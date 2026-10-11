use aip_execution::integrations::{Broker, EndpointConfig};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    server::{listen_with_authenticator_and_deployment, listen_with_managed, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::Builder::new().name("aip-test-schema-cleanup".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        if let Ok(cleanup) = cleanup {
            let _ = cleanup.join();
        }
    }
}

struct Auth {
    schema: String,
    iat: i64,
}
impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let grant = DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: 1, issued_at: self.iat, min_iat: 0 };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.iat + 600)
        })
    }
    fn verify_durable<'a>(
        &'a self,
        g: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.map_err(|_| AuthError::Unavailable)?;
            let row = db
                .query_opt(
                    &format!("SELECT enabled,min_iat FROM {}.aip_principals WHERE issuer=$1 AND subject=$2 AND actor_id=$3", self.schema),
                    &[&g.issuer, &g.subject, &g.actor_id],
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            if row.is_some_and(|r| r.get::<_, bool>(0) && r.get::<_, i64>(1) == g.min_iat) {
                Ok(())
            } else {
                Err(AuthError::BadSignature)
            }
        })
    }
}
async fn post(addr: std::net::SocketAddr, path: &str, token: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    stream.write_all(format!("POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    serde_json::from_slice(out.splitn(2, |_| false).next().unwrap())
        .unwrap_or_else(|_| serde_json::from_str(String::from_utf8(out).unwrap().split_once("\r\n\r\n").unwrap().1).unwrap())
}
async fn wait(addr: std::net::SocketAddr, fp: &str, id: &str) -> Value {
    wait_for(addr, fp, id, 100).await
}
async fn wait_for(addr: std::net::SocketAddr, fp: &str, id: &str, attempts: usize) -> Value {
    for _ in 0..attempts {
        let s = post(addr, "/jobs/status", "owner", fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&s["run"]["state"].as_str().unwrap_or("")) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job timeout");
}

async fn mock_endpoint(broken: bool) -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let call_count = calls.clone();
    let captured = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let call_count = call_count.clone();
            let captured = captured.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 2048];
                let mut body_len = 0usize;
                let mut header_end = None;
                loop {
                    let Ok(n) = socket.read(&mut chunk).await else { return };
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let head = String::from_utf8_lossy(&bytes[..pos]);
                            body_len = head
                                .lines()
                                .find_map(|line| {
                                    line.split_once(':')
                                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                        }
                    }
                    if header_end.is_some_and(|pos| bytes.len() >= pos + body_len) {
                        break;
                    }
                }
                call_count.fetch_add(1, Ordering::SeqCst);
                captured.lock().unwrap().push(String::from_utf8_lossy(&bytes).into_owned());
                if !broken {
                    let body = r#"{"text":"provider-response"}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            });
        }
    });
    (format!("http://{address}"), calls, requests)
}

async fn mock_gated_endpoint() -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let call_count = calls.clone();
    let captured = requests.clone();
    let request_seen = seen.clone();
    let response_release = release.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let call_count = call_count.clone();
            let captured = captured.clone();
            let request_seen = request_seen.clone();
            let response_release = response_release.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 2048];
                let mut body_len = 0usize;
                let mut header_end = None;
                loop {
                    let Ok(n) = socket.read(&mut chunk).await else { return };
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let head = String::from_utf8_lossy(&bytes[..pos]);
                            body_len = head
                                .lines()
                                .find_map(|line| {
                                    line.split_once(':')
                                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                        }
                    }
                    if header_end.is_some_and(|pos| bytes.len() >= pos + body_len) {
                        break;
                    }
                }
                call_count.fetch_add(1, Ordering::SeqCst);
                captured.lock().unwrap().push(String::from_utf8_lossy(&bytes).into_owned());
                request_seen.notify_one();
                response_release.notified().await;
                let body = r#"{"text":"provider-response"}"#;
                let response =
                    format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}"), calls, requests, seen, release)
}

fn endpoint(name: &str, url: String, address: SocketAddr) -> EndpointConfig {
    EndpointConfig {
        name: name.into(),
        url: format!("{url}/{name}"),
        method: "POST".into(),
        timeout: Duration::from_secs(2),
        max_request_bytes: 4096,
        max_response_bytes: 4096,
        secret_header: None,
        resolved: vec![address],
        allow_loopback: true,
        allow_private_networks: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_http_jobs_broker_requests_and_never_replay_unknown_effects_in_both_languages() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_durable_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _schema_cleanup = SchemaCleanup(schema.clone());
    let source = format!(
        "{}\n{}",
        include_str!("fixtures/jobs.aip"),
        r#"
operation job http_echo {
  input { text: Text(1..1000) }
  output { length: Int; status: Int }
  allow actor != null
  access http.echo
  effect managed
  deadline 10s
  implementation "jobs.httpEcho"
}
operation job http_broken {
  input { text: Text(1..1000) }
  output { ok: Bool }
  allow actor != null
  access http.broken
  effect managed
  deadline 10s
  implementation "jobs.httpBroken"
}
operation job http_broken_caught {
  input { text: Text(1..1000) }
  output { ok: Bool }
  allow actor != null
  access http.broken
  effect managed
  deadline 10s
  implementation "jobs.httpBrokenCaught"
}
operation job http_echo_replay {
  input { text: Text(1..1000) }
  output { length: Int; status: Int }
  allow actor != null
  access http.echo, jobs.length
  effect managed
  deadline 10s
  implementation "jobs.httpEchoReplay"
}
operation job http_hold {
  input { text: Text(1..1000) }
  output { length: Int; status: Int }
  allow actor != null
  access http.hold
  effect managed
  deadline 10s
  implementation "jobs.httpHold"
}"#
    );
    let facts = load_str(&source, Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let mut unknown_errors = Vec::new();
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let (echo_url, echo_count, echo_requests) = mock_endpoint(false).await;
        let (broken_url, broken_count, broken_requests) = mock_endpoint(true).await;
        let (hold_url, hold_count, hold_requests, hold_seen, hold_release) = mock_gated_endpoint().await;
        let echo_addr: SocketAddr = echo_url.trim_start_matches("http://").parse().unwrap();
        let broken_addr: SocketAddr = broken_url.trim_start_matches("http://").parse().unwrap();
        let hold_addr: SocketAddr = hold_url.trim_start_matches("http://").parse().unwrap();
        let broker = Arc::new(
            Broker::new(vec![
                endpoint("echo", echo_url, echo_addr),
                endpoint("broken", broken_url, broken_addr),
                endpoint("hold", hold_url, hold_addr),
            ])
            .unwrap(),
        );
        let start = || {
            listen_with_managed(
                facts.clone(),
                IdWire::SafeNumber,
                "127.0.0.1:0".parse().unwrap(),
                spike_v2_read::DB_URL.into(),
                Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
                ServerOptions::default(),
                false,
                auth.clone(),
                Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
                Some(broker.clone()),
                false,
            )
        };
        let server = start().await.unwrap();
        let echo = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"http_echo","input":{"text":"hello"},"key":format!("echo-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(echo["ok"], true, "{echo}");
        let echo_id = echo["run"]["id"].as_str().unwrap();
        assert_eq!(wait(server.address, &fp, echo_id).await["run"]["state"], "SUCCEEDED");
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":echo_id})).await["output"], json!({"length":17,"status":200}));
        let req = echo_requests.lock().unwrap().join("\n");
        assert!(req.to_ascii_lowercase().contains("idempotency-key:"), "{req}");
        assert!(!req.contains("owner"), "provider request must not contain raw session token: {req}");
        assert_eq!(echo_count.load(Ordering::SeqCst), 1);
        let replay = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"http_echo_replay","input":{"text":"hello"},"key":format!("echo-replay-{lang:?}"),"delayMs":0}),
        )
        .await;
        let replay_id = replay["run"]["id"].as_str().unwrap();
        assert_eq!(wait(server.address, &fp, replay_id).await["run"]["state"], "SUCCEEDED");
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":replay_id})).await["output"], json!({"length":17,"status":200}));
        assert_eq!(echo_count.load(Ordering::SeqCst), 2, "reusing a stable HTTP key after a different effect must not issue a second request");
        let effect_count: i64 =
            db.query_one(&format!("SELECT count(*) FROM {schema}.aip_effects WHERE run_id::text=$1"), &[&replay_id]).await.unwrap().get(0);
        assert_eq!(effect_count, 2, "two stable effect keys should create two journal rows despite the repeated HTTP call");
        let broken = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"http_broken","input":{"text":"hello"},"key":format!("broken-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(broken["ok"], true, "{broken}");
        let broken_id = broken["run"]["id"].as_str().unwrap();
        let failed = wait(server.address, &fp, broken_id).await;
        assert_eq!(failed["run"]["state"], "FAILED", "{failed}");
        if failed["run"]["error"] != "EFFECT_UNKNOWN" {
            unknown_errors.push(format!("{lang:?}: {}", failed["run"]["error"]));
        }
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":broken_id})).await["code"], "JOB_FAILED");
        assert_eq!(
            post(
                server.address,
                "/jobs/enqueue",
                "owner",
                &fp,
                json!({"operation":"http_broken","input":{"text":"hello"},"key":format!("broken-{lang:?}"),"delayMs":0})
            )
            .await["run"]["id"],
            broken_id
        );
        assert_eq!(post(server.address, "/jobs/status", "owner", &fp, json!({"id":broken_id})).await["run"]["state"], "FAILED");
        assert_eq!(echo_count.load(Ordering::SeqCst), 2);
        assert_eq!(broken_count.load(Ordering::SeqCst), 1);
        let state: String =
            db.query_one(&format!("SELECT state FROM {schema}.aip_effects WHERE run_id::text=$1"), &[&broken_id]).await.unwrap().get(0);
        assert_eq!(state, "UNKNOWN");
        let req = broken_requests.lock().unwrap().join("\n");
        assert!(req.to_ascii_lowercase().contains("idempotency-key:"), "{req}");
        assert!(!req.contains("owner"), "provider request must not contain raw session token: {req}");
        let caught = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"http_broken_caught","input":{"text":"hello"},"key":format!("broken-caught-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(caught["ok"], true, "{caught}");
        let caught_id = caught["run"]["id"].as_str().unwrap();
        let caught_state = wait(server.address, &fp, caught_id).await;
        assert_eq!(caught_state["run"]["state"], "FAILED", "caught EFFECT_UNKNOWN must not let the job report success: {caught_state}");
        assert_eq!(caught_state["run"]["error"], "EFFECT_UNKNOWN", "{caught_state}");
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":caught_id})).await["code"], "JOB_FAILED");
        let caught_effect: String =
            db.query_one(&format!("SELECT state FROM {schema}.aip_effects WHERE run_id::text=$1"), &[&caught_id]).await.unwrap().get(0);
        assert_eq!(caught_effect, "UNKNOWN");
        assert_eq!(
            post(
                server.address,
                "/jobs/enqueue",
                "owner",
                &fp,
                json!({"operation":"http_broken_caught","input":{"text":"hello"},"key":format!("broken-caught-{lang:?}"),"delayMs":0})
            )
            .await["run"]["id"],
            caught_id
        );
        assert_eq!(broken_count.load(Ordering::SeqCst), 2, "a caught unknown outcome must not replay the external request");
        let req = broken_requests.lock().unwrap().join("\n");
        assert!(req.lines().all(|line| !line.contains("owner")), "provider request must not contain raw session token: {req}");
        let held = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"http_hold","input":{"text":"hold"},"key":format!("hold-{lang:?}"),"delayMs":0}),
        )
        .await;
        let held_id = held["run"]["id"].as_str().unwrap();
        tokio::time::timeout(Duration::from_secs(5), hold_seen.notified()).await.unwrap();
        assert_eq!(post(server.address, "/jobs/cancel", "owner", &fp, json!({"id":held_id})).await["ok"], true);
        hold_release.notify_one();
        let held_state = wait(server.address, &fp, held_id).await;
        assert_eq!(held_state["run"]["state"], "FAILED", "an in-flight external effect cannot be reported as safely cancelled: {held_state}");
        assert_eq!(held_state["run"]["error"], "CANCELLATION_UNCONFIRMED", "{held_state}");
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":held_id})).await["code"], "JOB_FAILED");
        assert_eq!(hold_count.load(Ordering::SeqCst), 1);
        let req = hold_requests.lock().unwrap().join("\n");
        assert!(req.to_ascii_lowercase().contains("idempotency-key:"), "{req}");
        assert!(!req.contains("owner"), "provider request must not contain raw session token: {req}");
        server.shutdown().await.unwrap();
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(unknown_errors.is_empty(), "broker outcome uncertainty must remain visible as EFFECT_UNKNOWN: {unknown_errors:?}");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_jobs_survive_restart_cancel_and_isolate_files_events_in_both_languages() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_durable_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _schema_cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1),('test','other',1)")).await.unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let start = || {
            listen_with_authenticator_and_deployment(
                facts.clone(),
                IdWire::SafeNumber,
                "127.0.0.1:0".parse().unwrap(),
                spike_v2_read::DB_URL.into(),
                Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
                ServerOptions::default(),
                false,
                auth.clone(),
                Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            )
        };
        let server = start().await.unwrap();
        assert_eq!(
            post(
                server.address,
                "/files/put",
                "owner",
                &fp,
                json!({"bucket":"reports","name":"upload.txt","mediaType":"text/plain","base64":"eA=="})
            )
            .await["code"],
            "NOT_EXPOSED"
        );
        let body = json!({"operation":"count","input":{"text":"가😀A"},"key":format!("count-{lang:?}"),"delayMs":500});
        let submitted = post(server.address, "/jobs/enqueue", "owner", &fp, body.clone()).await;
        assert_eq!(submitted["ok"], true, "{submitted}");
        let id = submitted["run"]["id"].as_str().unwrap();
        assert_eq!(post(server.address, "/jobs/enqueue", "owner", &fp, body.clone()).await["run"]["id"], id);
        assert_eq!(post(server.address, "/jobs/status", "other", &fp, json!({"id":id})).await["code"], "NOT_FOUND");
        server.shutdown().await.unwrap();
        let server = start().await.unwrap();
        assert_eq!(wait(server.address, &fp, id).await["run"]["state"], "SUCCEEDED");
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":id})).await["output"], json!({"length":3}));
        db.execute(&format!("UPDATE {schema}.aip_principals SET enabled=false WHERE issuer='test' AND subject='owner'"), &[]).await.unwrap();
        let revoked = post(server.address, "/jobs/result", "owner", &fp, json!({"id":id})).await;
        assert_eq!(revoked["code"], "UNAUTHENTICATED", "terminal results must check the principal's current authorization: {revoked}");
        db.execute(&format!("UPDATE {schema}.aip_principals SET enabled=true WHERE issuer='test' AND subject='owner'"), &[]).await.unwrap();
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":id})).await["output"], json!({"length":3}));
        let slow =
            post(server.address, "/jobs/enqueue", "owner", &fp, json!({"operation":"slow","input":{},"key":format!("slow-{lang:?}"),"delayMs":0}))
                .await;
        let slow_id = slow["run"]["id"].as_str().unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(post(server.address, "/jobs/cancel", "owner", &fp, json!({"id":slow_id})).await["ok"], true);
        assert_eq!(wait(server.address, &fp, slow_id).await["run"]["state"], "CANCELLED");
        let report = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"report","input":{"text":"report"},"key":format!("report-{lang:?}"),"delayMs":0}),
        )
        .await;
        let state = wait(server.address, &fp, report["run"]["id"].as_str().unwrap()).await;
        assert_eq!(state["run"]["state"], "SUCCEEDED", "{state}");
        let result = post(server.address, "/jobs/result", "owner", &fp, json!({"id":report["run"]["id"]})).await;
        let fid = &result["output"]["file"];
        assert_eq!(post(server.address, "/files/get", "owner", &fp, json!({"id":fid})).await["base64"], "cmVwb3J0");
        assert_eq!(post(server.address, "/files/get", "other", &fp, json!({"id":fid})).await["code"], "NOT_FOUND");
        let events = post(server.address, "/events/read", "owner", &fp, json!({"stream":"notifications","after":"0","limit":100})).await;
        assert!(!events["events"].as_array().unwrap().is_empty(), "{events}");
        assert!(post(server.address, "/events/read", "other", &fp, json!({"stream":"notifications","after":"0","limit":100})).await["events"]
            .as_array()
            .unwrap()
            .is_empty());
        server.shutdown().await.unwrap();
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_jobs_share_one_capability_budget_in_both_languages() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_durable_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _schema_cleanup = SchemaCleanup(schema.clone());
    let source = format!(
        "{}\n{}",
        include_str!("fixtures/jobs.aip"),
        r#"
operation job budgetRoot {
  input {}
  output { length: Int }
  allow actor != null
  access jobs.budgetChild, events.notifications
  effect managed
  deadline 120s
  implementation "jobs.budgetRoot"
}

operation job budgetChild {
  input {}
  output { length: Int }
  allow actor != null
  access events.notifications
  effect managed
  deadline 120s
  implementation "jobs.budgetChild"
}"#
    );
    let facts = load_str(&source, Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let mut budget_violations = Vec::new();
    for (language_index, lang) in [WorkerLang::Node, WorkerLang::Python].into_iter().enumerate() {
        let server = listen_with_managed(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            spike_v2_read::DB_URL.into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            None,
            false,
        )
        .await
        .unwrap();
        let job = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"budgetRoot","input":{},"key":format!("nested-budget-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(job["ok"], true, "{job}");
        let id = job["run"]["id"].as_str().unwrap();
        let failed = wait_for(server.address, &fp, id, 2400).await;
        if failed["run"]["state"] != "FAILED" || failed["run"]["error"] != "COST_EXCEEDED" {
            let connections: i64 = db.query_one("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()", &[]).await.unwrap().get(0);
            let stored = db
                .query_one(
                    &format!(
                        "SELECT state,error,extract(epoch from attempt_deadline-clock_timestamp())::float8 FROM {schema}.aip_jobs WHERE id::text=$1"
                    ),
                    &[&id],
                )
                .await
                .unwrap();
            let effect_states: Vec<(String, i64)> = db
                .query(&format!("SELECT state,count(*) FROM {schema}.aip_effects WHERE run_id::text=$1 GROUP BY state ORDER BY state"), &[&id])
                .await
                .unwrap()
                .into_iter()
                .map(|row| (row.get(0), row.get(1)))
                .collect();
            budget_violations.push(format!("{lang:?}: run={failed}; stored_state={}, stored_error={}, seconds_to_deadline={:?}, postgres_connections={connections}, effects_by_state={effect_states:?}",stored.get::<_,String>(0),stored.get::<_,Option<String>>(1).unwrap_or_default(),stored.get::<_,Option<f64>>(2),));
        }
        let event_owner = spike_v1_fixture::digest(&json!(["test", "owner", 1, 0]));
        let event_count: i64 = db
            .query_one(&format!("SELECT count(*) FROM {schema}.aip_events WHERE owner=$1 AND stream='notifications'"), &[&event_owner])
            .await
            .unwrap()
            .get(0);
        let expected_count = 63 * (language_index + 1);
        if event_count != expected_count as i64 {
            budget_violations.push(format!("{lang:?}: expected {expected_count} cumulative committed nested events, found {event_count}"));
        }
        server.shutdown().await.unwrap();
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(budget_violations.is_empty(), "nested job calls must share a 64-call budget: {budget_violations:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn known_child_failure_can_be_caught_and_replayed_by_parent_in_both_languages() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_durable_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _schema_cleanup = SchemaCleanup(schema.clone());
    let source = format!(
        "{}\n{}",
        include_str!("fixtures/jobs.aip"),
        r#"
operation job knownFailChild {
  input {}
  output { length: Int }
  allow actor != null
  effect managed
  deadline 10s
  implementation "jobs.knownFailChild"
}
operation job catchKnownChildFailure {
  input {}
  output { length: Int }
  allow actor != null
  access jobs.knownFailChild
  effect managed
  deadline 10s
  implementation "jobs.catchKnownChildFailure"
}"#
    );
    let facts = load_str(&source, Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let server = listen_with_managed(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            spike_v2_read::DB_URL.into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            None,
            false,
        )
        .await
        .unwrap();
        let submitted = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"catchKnownChildFailure","input":{},"key":format!("catch-known-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(submitted["ok"], true, "{submitted}");
        let parent_id = submitted["run"]["id"].as_str().unwrap();
        let terminal = wait(server.address, &fp, parent_id).await;
        assert_eq!(terminal["run"]["state"], "SUCCEEDED", "the parent should recover from a known child failure: {terminal}");
        assert_eq!(post(server.address, "/jobs/result", "owner", &fp, json!({"id":parent_id})).await["output"], json!({"length":2}));
        let failed_child_entries: i64 = db
            .query_one(&format!("SELECT count(*) FROM {schema}.aip_effects WHERE run_id::text=$1 AND state='FAILED'"), &[&parent_id])
            .await
            .unwrap()
            .get(0);
        assert_eq!(failed_child_entries, 1, "repeating the same child key must replay one failed child journal entry");
        let pending_entries: i64 = db
            .query_one(&format!("SELECT count(*) FROM {schema}.aip_effects WHERE run_id::text=$1 AND state='PENDING'"), &[&parent_id])
            .await
            .unwrap()
            .get(0);
        assert_eq!(pending_entries, 0, "known failure recovery must not leave a pending effect journal entry");
        server.shutdown().await.unwrap();
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn messages_share_atomic_job_effects_and_reject_foreign_acknowledgements() {
    let _lock = SCHEMA_LOCK.lock().await;
    let schema_name = format!("aip_durable_{}", std::process::id());
    let schema = schema_name.as_str();
    let _cleanup = SchemaCleanup(schema.into());
    sqlgen::set_schema(schema);
    let source=format!("{}\noperation job dispatch {{ input {{ text: Text }} output {{ length: Int }} allow actor != null access messages.notifications effect managed deadline 5s implementation \"jobs.message\" }}",include_str!("fixtures/jobs.aip"));
    let facts = load_str(&source, Form::A).unwrap().execution;
    aip_migrate::init(spike_v2_read::DB_URL, schema, &facts).await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1),('test','other',1)")).await.unwrap();
    let iat: i64 = db.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let infra = spike_v6_transport::infrastructure::Infrastructure { messaging: true, client_consume: true, ..Default::default() };
        let server = spike_v6_transport::server::listen_with_infrastructure(
            facts.clone(),
            IdWire::DecimalString,
            "127.0.0.1:0".parse().unwrap(),
            spike_v2_read::DB_URL.into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
            ServerOptions::default(),
            false,
            Arc::new(Auth { schema: schema.into(), iat }),
            Some(DeploymentFence::new(schema.into(), spike_v1_fixture::digest(&facts)).unwrap()),
            None,
            false,
            infra,
        )
        .await
        .unwrap();
        let direct_publish = post(
            server.address,
            "/messages/enqueue",
            "owner",
            &fp,
            json!({"queue":"notifications","key":"client-forbidden","payload":{"text":"injected"}}),
        )
        .await;
        assert_eq!(direct_publish["code"], "NOT_EXPOSED", "job queue declaration must not grant client publishing: {direct_publish}");
        let run = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"dispatch","input":{"text":"가😀A"},"key":format!("messages-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(run["ok"], true, "{run}");
        let done = wait(server.address, &fp, run["run"]["id"].as_str().unwrap()).await;
        assert_eq!(done["run"]["state"], "SUCCEEDED", "{done}");
        let claimed = post(server.address, "/messages/claim", "owner", &fp, json!({"queue":"notifications","worker":"mailer"})).await;
        assert_eq!(claimed["ok"], true, "{claimed}");
        assert_eq!(claimed["lease"]["payload"]["text"], "가😀A");
        let lease = claimed["lease"].clone();
        let foreign = post(server.address, "/messages/ack", "other", &fp, json!({"queue":"notifications","lease":lease})).await;
        assert_eq!(foreign["acknowledged"], false, "{foreign}");
        let ack = post(server.address, "/messages/ack", "owner", &fp, json!({"queue":"notifications","lease":lease})).await;
        assert_eq!(ack["acknowledged"], true, "{ack}");
        let empty = post(server.address, "/messages/claim", "owner", &fp, json!({"queue":"notifications","worker":"mailer"})).await;
        assert!(empty["lease"].is_null(), "{empty}");
        let sdk_run = post(
            server.address,
            "/jobs/enqueue",
            "owner",
            &fp,
            json!({"operation":"dispatch","input":{"text":"SDK 실서버"},"key":format!("messages-sdk-{lang:?}"),"delayMs":0}),
        )
        .await;
        assert_eq!(sdk_run["ok"], true, "{sdk_run}");
        let done = wait(server.address, &fp, sdk_run["run"]["id"].as_str().unwrap()).await;
        assert_eq!(done["run"]["state"], "SUCCEEDED", "{done}");
        let script = r#"
            import assert from 'node:assert/strict';
            const {createInfrastructureClient} = await import(process.argv[1]);
            const [base, fingerprint] = process.argv.slice(2);
            const client = createInfrastructureClient(async (path, body) => {
                const response = await fetch(base + path, {method:'POST', headers:{
                    authorization:'Bearer owner', 'x-aip-contract':fingerprint,
                    'content-type':'application/json'
                }, body:JSON.stringify(body)});
                return response.json();
            }, fingerprint);
            const lease = await client.messages.claim({queue:'notifications', worker:'sdk-worker'});
            assert.equal(lease.payload.text, 'SDK 실서버');
            assert.match(lease.leaseUntil, /Z$/u);
            assert.equal(await client.messages.ack({queue:'notifications', lease}), true);
            assert.deepEqual(await client.messages.dead({queue:'notifications', cursor:lease.id}), {items:[],nextCursor:null});
            console.log('SDK messaging HTTP contract verified');
        "#;
        let sdk_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("client/infrastructure.ts");
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new("node")
                .args(["--input-type=module", "-e", script])
                .arg(sdk_path)
                .arg(format!("http://{}", server.address))
                .arg(&fp)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(output.status.success(), "SDK HTTP regression: {}", String::from_utf8_lossy(&output.stderr));
        server.shutdown().await.unwrap();
    }
}
