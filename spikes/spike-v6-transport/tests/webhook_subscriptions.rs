//! The worked policy for outbound webhook subscribers, written with the primitives the product already has: a
//! subscription row counts consecutive failed deliveries, a success resets the count, and the subscription is
//! disabled after three failures in a row or at once when its destination is rejected.
use aip_execution::integrations::{standard_webhook_signature, Broker, WebhookSenderConfig};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const SECRET: &[u8] = b"subscriber-signing-secret";

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        let _ = cleanup.join();
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

async fn post(addr: std::net::SocketAddr, path: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer owner\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("send");
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.expect("response in time").expect("read");
    serde_json::from_str(String::from_utf8(out).expect("utf-8").split_once("\r\n\r\n").expect("head and body").1).expect("json body")
}

async fn run_fanout(addr: std::net::SocketAddr, fp: &str, event: &str) -> Value {
    let queued = post(addr, "/jobs/enqueue", fp, json!({"operation":"fanout","input":{"event":event},"key":event,"delayMs":0})).await;
    let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("enqueue: {queued}")).to_string();
    for _ in 0..400 {
        let status = post(addr, "/jobs/status", fp, json!({"id":id})).await;
        let state = status["run"]["state"].as_str().unwrap_or("");
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&state) {
            assert_eq!(state, "SUCCEEDED", "{status}");
            return post(addr, "/jobs/result", fp, json!({"id":id})).await["output"].clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_is_disabled_after_consecutive_failures_and_a_success_resets_the_count() {
    let schema = format!("aip_webhook_subs_{}", std::process::id());
    sqlgen::try_set_schema(&schema).expect("schema");
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/webhook-subscriptions.aip"), Form::A).expect("definition").execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.expect("init");
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.expect("db");
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .expect("seed");
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.expect("clock").get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);

    // A subscriber that verifies the signature and answers 204, or 500 on /fail.
    let receiver = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = receiver.local_addr().expect("address").port();
    let accepted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = receiver.accept().await {
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                raw.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = head
                        .lines()
                        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse().unwrap_or(0)))
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&raw).to_string();
            let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
            let header = |name: &str| {
                head.lines().find_map(|line| line.split_once(": ").filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.to_string()))
            };
            let valid = match (header("webhook-id"), header("webhook-timestamp").and_then(|t| t.parse::<u64>().ok()), header("webhook-signature")) {
                (Some(id), Some(timestamp), Some(signature)) => {
                    signature == format!("v1,{}", standard_webhook_signature(SECRET, &id, timestamp, body.as_bytes()))
                }
                _ => false,
            };
            let target = head.lines().next().unwrap_or("").to_owned();
            let status = if target.starts_with("POST /fail") || !valid { "500 Internal Server Error" } else { "204 No Content" };
            if status.starts_with("204") {
                log.lock().expect("log").push(target);
            }
            let _ = stream.write_all(format!("HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n").as_bytes()).await;
        }
    });

    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let label = format!("{lang:?}").to_lowercase();
        db.batch_execute(&format!(
            "DELETE FROM {schema}.subscription; \
             INSERT INTO {schema}.subscription(id,member_id,url,active,failures) VALUES \
              (1,1,'http://127.0.0.1:{port}/ok/1',true,0), \
              (2,1,'http://127.0.0.1:{port}/fail',true,0), \
              (3,1,'https://10.1.2.3/hook',true,0), \
              (4,1,'http://127.0.0.1:{port}/ok/4',true,2)"
        ))
        .await
        .expect("subscriptions");
        let broker = Broker::new(vec![])
            .expect("broker")
            .with_webhooks(vec![WebhookSenderConfig {
                name: "subscribers".into(),
                secret: SECRET.to_vec(),
                timeout: Duration::from_secs(2),
                max_request_bytes: 4096,
                allow_loopback: true,
            }])
            .expect("webhook sender");
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().expect("address"),
            spike_v2_read::DB_URL.into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/webhook-subscriptions").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).expect("fence")),
            Some(Arc::new(broker)),
            false,
            Infrastructure::default(),
        )
        .await
        .expect("server");
        let state = || async {
            db.query(&format!("SELECT id,active,failures FROM {schema}.subscription ORDER BY id"), &[])
                .await
                .expect("state")
                .iter()
                .map(|row| (row.get::<_, i64>(0), row.get::<_, bool>(1), row.get::<_, i64>(2)))
                .collect::<Vec<_>>()
        };
        let before = accepted.lock().expect("log").len();

        // Run 1: a private destination is disabled at once; a failing one starts a streak; a success resets an old streak.
        let first = run_fanout(server.address, &fp, &format!("e1-{label}")).await;
        assert_eq!(first, json!({"delivered":2,"failed":2,"disabled":1}), "{lang:?}");
        assert_eq!(state().await, vec![(1, true, 0), (2, true, 1), (3, false, 1), (4, true, 0)], "{lang:?}");
        // Run 2: the streak grows but the subscription stays.
        let second = run_fanout(server.address, &fp, &format!("e2-{label}")).await;
        assert_eq!(second, json!({"delivered":2,"failed":1,"disabled":0}), "{lang:?}");
        assert_eq!(state().await[1], (2, true, 2), "{lang:?}");
        // Run 3: the third failure in a row disables it.
        let third = run_fanout(server.address, &fp, &format!("e3-{label}")).await;
        assert_eq!(third, json!({"delivered":2,"failed":1,"disabled":1}), "{lang:?}");
        assert_eq!(state().await[1], (2, false, 3), "{lang:?}");
        // Run 4: disabled subscriptions are not tried again, healthy ones keep receiving.
        let fourth = run_fanout(server.address, &fp, &format!("e4-{label}")).await;
        assert_eq!(fourth, json!({"delivered":2,"failed":0,"disabled":0}), "{lang:?}");

        let delivered = accepted.lock().expect("log")[before..].to_vec();
        assert_eq!(delivered.len(), 8, "{lang:?}: two healthy subscribers over four runs: {delivered:?}");
        assert!(delivered.iter().all(|target| target.contains("/ok/")), "{lang:?}: nothing reached the failing or private ones: {delivered:?}");
        server.shutdown().await.expect("shutdown");
    }
}
