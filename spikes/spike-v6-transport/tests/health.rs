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
    let schema = format!("aip_health_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let server = listen_with_managed(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
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
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!("UPDATE {schema}.aip_migrate_meta SET facts_digest='{}'", "0".repeat(64))).await.unwrap();
    let (status, _, body) = request(addr, "GET", "/ready").await;
    assert_eq!((status, body["code"].as_str()), (503, Some("DEPLOYMENT_CHANGED")), "{body}");
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
    assert_eq!((status, body["code"].as_str()), (503, Some("DB_UNAVAILABLE")), "{body}");
    assert!(!body.to_string().contains("127.0.0.1"));
    offline.shutdown().await.unwrap();
}
