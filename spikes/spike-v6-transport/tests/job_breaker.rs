//! An open circuit breaker rejects a durable HTTP effect before sending, so no unknown effect is left behind.
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
    let mut stream = TcpStream::connect(addr).await.unwrap();
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
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().split_once("\r\n\r\n").unwrap().1).unwrap()
}

async fn settle(addr: std::net::SocketAddr, fp: &str, id: &str) -> Value {
    // Backoff between attempts is 1s then 2s, so allow well beyond three attempts with worker start-up.
    for _ in 0..400 {
        let status = post(addr, "/jobs/status", fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&status["run"]["state"].as_str().unwrap_or("")) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_circuit_rejects_without_sending_and_leaves_no_unknown_effect() {
    use aip_execution::integrations::{Broker, CircuitBreaker, EndpointConfig};
    let schema = format!("aip_job_breaker_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-breaker.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);

    let provider = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = provider.local_addr().unwrap();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = provider.accept().await {
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).await;
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: 2\r\n\r\n{}")
                .await;
        }
    });
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        // A fresh broker per language starts with a closed circuit.
        let broker = Broker::new(vec![EndpointConfig {
            name: "crm".into(),
            url: format!("http://127.0.0.1:{}/crm", address.port()),
            method: "POST".into(),
            timeout: Duration::from_secs(2),
            max_request_bytes: 4096,
            max_response_bytes: 4096,
            secret_header: None,
            resolved: vec![address],
            allow_loopback: true,
            allow_private_networks: false,
            circuit_breaker: Some(CircuitBreaker { failure_threshold: 1, open_for: Duration::from_secs(60) }),
            oauth: None,
        }])
        .unwrap();
        let before = requests.load(std::sync::atomic::Ordering::SeqCst);
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            spike_v2_read::DB_URL.into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-retry").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            Some(Arc::new(broker)),
            false,
            Infrastructure::default(),
        )
        .await
        .unwrap();
        let addr = server.address;
        let label = format!("{lang:?}").to_lowercase();
        let queued = post(addr, "/jobs/enqueue", &fp, json!({"operation":"callout","input":{},"key":format!("callout-{label}"),"delayMs":0})).await;
        let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("{lang:?} enqueue: {queued}")).to_string();
        let status = settle(addr, &fp, &id).await;
        assert_eq!(status["run"]["state"], "SUCCEEDED", "{lang:?}: {status}");
        let result = post(addr, "/jobs/result", &fp, json!({"id":id})).await;
        assert_eq!(result["output"], json!({"first":503,"second":"WIRE.CIRCUIT_OPEN","replayed":503}), "{lang:?}: {result}");
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst) - before, 1, "{lang:?}: the open circuit must not reach the provider");
        let effects: Vec<(String, String)> = db
            .query(&format!("SELECT effect_key,state FROM {schema}.aip_effects WHERE run_id=$1::text::uuid"), &[&id])
            .await
            .unwrap()
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();
        assert_eq!(effects.len(), 1, "{lang:?}: a rejected call must not leave a PENDING or UNKNOWN effect: {effects:?}");
        assert_eq!(effects[0].1, "SUCCEEDED", "{lang:?}: {effects:?}");
        server.shutdown().await.unwrap();
    }

    // An OAuth token endpoint that refuses to issue a token: the API call is never sent and no effect remains.
    let broker = Broker::new(vec![EndpointConfig {
        name: "crm".into(),
        url: format!("http://127.0.0.1:{}/crm", address.port()),
        method: "POST".into(),
        timeout: Duration::from_secs(2),
        max_request_bytes: 4096,
        max_response_bytes: 4096,
        secret_header: None,
        resolved: vec![address],
        allow_loopback: true,
        allow_private_networks: false,
        circuit_breaker: None,
        oauth: Some(aip_execution::integrations::OAuthConfig {
            token_url: format!("http://127.0.0.1:{}/token", address.port()),
            resolved: vec![address],
            client_id: "client".into(),
            client_secret: "secret".into(),
            scope: None,
            basic_auth: true,
        }),
    }])
    .unwrap();
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-retry").into() }),
        ServerOptions::default(),
        false,
        auth.clone(),
        Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
        Some(Arc::new(broker)),
        false,
        Infrastructure::default(),
    )
    .await
    .unwrap();
    let before = requests.load(std::sync::atomic::Ordering::SeqCst);
    let queued = post(server.address, "/jobs/enqueue", &fp, json!({"operation":"callout","input":{},"key":"callout-oauth","delayMs":0})).await;
    let id = queued["run"]["id"].as_str().unwrap().to_string();
    let status = settle(server.address, &fp, &id).await;
    assert_eq!(status["run"]["state"], "FAILED", "{status}");
    // Only the token endpoint was contacted.
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst) - before, 1);
    let effects: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_effects WHERE run_id=$1::text::uuid"), &[&id]).await.unwrap().get(0);
    assert_eq!(effects, 0, "a call refused for want of a token must not leave an effect");
    server.shutdown().await.unwrap();
}
