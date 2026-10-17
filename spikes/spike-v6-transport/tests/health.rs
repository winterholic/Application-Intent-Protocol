//! Liveness answers without dependencies; readiness follows the database and the deployment fence.
use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    server::{listen_with_apply_contract, listen_with_managed, DeploymentFence, ServerOptions},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

// A process may use one schema; both tests use the same name, one after the other.
static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        let _ = cleanup.join();
    }
}

struct NoSessions;
impl Authenticator for NoSessions {
    fn verify_session<'a>(&'a self, _: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
    fn verify_durable<'a>(
        &'a self,
        _: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
}

async fn request(addr: std::net::SocketAddr, method: &str, path: &str) -> (u16, String, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head.to_ascii_lowercase(), serde_json::from_str(body).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readiness_tracks_database_and_deployment_while_liveness_stays_up() {
    let _serial = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_health_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let server = listen_with_managed(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(NoSessions),
        Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
        None,
        false,
    )
    .await
    .unwrap();
    let addr = server.address;
    let (status, head, body) = request(addr, "GET", "/health").await;
    assert_eq!((status, body["status"].as_str()), (200, Some("live")));
    assert!(head.contains("cache-control: no-store"));
    let (status, _, body) = request(addr, "GET", "/ready").await;
    assert_eq!((status, body["status"].as_str()), (200, Some("ready")), "{body}");
    let (status, _, _) = request(addr, "HEAD", "/ready").await;
    assert_eq!(status, 200);
    let (status, head, _) = request(addr, "POST", "/health").await;
    assert_eq!(status, 405);
    assert!(head.contains("allow: get, head"));

    // After another deployment migrates the schema, this instance must leave the load balancer.
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!("UPDATE {schema}.aip_migrate_meta SET facts_digest='{}'", "0".repeat(64))).await.unwrap();
    let (status, _, body) = request(addr, "GET", "/ready").await;
    assert_eq!((status, body["code"].as_str()), (503, Some("WIRE.DEPLOYMENT_CHANGED")), "{body}");
    let (status, _, _) = request(addr, "GET", "/health").await;
    assert_eq!(status, 200);
    server.shutdown().await.unwrap();

    // An unreachable database keeps the process live but not ready, without leaking connection details.
    let offline = listen_with_apply_contract(
        facts,
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1".into(),
    )
    .await
    .unwrap();
    let (status, _, _) = request(offline.address, "GET", "/health").await;
    assert_eq!(status, 200);
    let (status, _, body) = request(offline.address, "GET", "/ready").await;
    assert_eq!((status, body["code"].as_str()), (503, Some("WIRE.DB_UNAVAILABLE")), "{body}");
    assert!(!body.to_string().contains("127.0.0.1"));
    offline.shutdown().await.unwrap();
}

/// O-23: `/ready` is unauthenticated and runs one database check at a time. While a check is stuck, further probes must
/// get the previous answer immediately instead of queueing behind it, or a flood of probes starves the load balancer's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_does_not_queue_behind_a_stuck_readiness_check() {
    let _serial = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_health_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let server = listen_with_managed(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(NoSessions),
        Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
        None,
        false,
    )
    .await
    .unwrap();
    let addr = server.address;
    assert_eq!(request(addr, "GET", "/ready").await.0, 200, "a first answer exists to fall back on");

    // The check reads aip_migrate_meta; an exclusive lock held by another session makes that read wait.
    let blocker = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    blocker.batch_execute(&format!("BEGIN; LOCK TABLE {schema}.aip_migrate_meta IN ACCESS EXCLUSIVE MODE")).await.unwrap();
    let stuck = tokio::spawn(async move { request(addr, "GET", "/ready").await });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let started = std::time::Instant::now();
    let (status, _, body) = request(addr, "GET", "/ready").await;
    let waited = started.elapsed();
    assert!(waited < Duration::from_millis(800), "the probe queued behind the stuck check for {waited:?}");
    assert_eq!((status, body["status"].as_str()), (200, Some("ready")), "the previous answer is served meanwhile: {body}");

    // The stuck check itself reports the database as unready once its own deadline passes.
    let (status, _, _) = stuck.await.unwrap();
    assert_eq!(status, 503);
    blocker.batch_execute("COMMIT").await.unwrap();
    assert_eq!(request(addr, "GET", "/ready").await.0, 200, "once the database answers again the probe is fresh");
    server.shutdown().await.unwrap();
}
