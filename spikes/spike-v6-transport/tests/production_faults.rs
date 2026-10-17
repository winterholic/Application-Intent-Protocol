use aip_execution::{
    messages::{ddl as messages_ddl, MessageStore, NewMessage},
    rate_limit::{Decision, Policy, RateLimiter},
};
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, Session},
    infrastructure::{Infrastructure, ResourceLimits},
    server::{listen_with_infrastructure, ServerOptions},
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);

fn schema(prefix: &str) -> String {
    format!("aip_{prefix}_{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed))
}

async fn connect() -> Client {
    let (client, driver) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = driver.await;
    });
    client
}

struct RejectAuth;
impl Authenticator for RejectAuth {
    fn verify_session<'a>(&'a self, _token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
}

async fn session_request(address: std::net::SocketAddr) -> Vec<u8> {
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST /session HTTP/1.1\r\nhost: {address}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    match tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut response)).await.unwrap() {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(error) => panic!("read session response: {error}"),
    }
    response
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_rate_checks_across_connections_never_exceed_the_shared_limit() {
    let schema = schema("rate_fault");
    let setup = connect().await;
    setup.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
    let limiter = RateLimiter::new(&schema).unwrap();
    limiter.install(&setup).await.unwrap();
    let policy = Policy { limit: 4, window: Duration::from_secs(60) };

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let limiter = limiter.clone();
        tasks.spawn(async move {
            let mut client = connect().await;
            limiter.check(&mut client, "principal:one", "/read", policy).await
        });
    }
    let mut allowed = 0;
    let mut limited = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap().unwrap() {
            Decision::Allowed => allowed += 1,
            Decision::Limited { retry_after_ms } => {
                assert!((1..=60_000).contains(&retry_after_ms));
                limited += 1;
            }
        }
    }
    assert_eq!((allowed, limited), (4, 16));
    setup.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn lease_race_uses_independent_clients_and_expired_owner_cannot_ack_replacement() {
    let schema = schema("lease_fault");
    let setup = connect().await;
    setup.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
    setup.batch_execute(&messages_ddl(&schema).unwrap()).await.unwrap();
    let store = MessageStore::new(&schema).unwrap();
    let created = store
        .enqueue(
            &setup,
            NewMessage {
                owner: "issuer|subject".into(),
                queue: "work".into(),
                key: "one".into(),
                payload: json!({"id":1}),
                max_deliveries: 3,
                ttl_ms: 60_000,
                delay_ms: 0,
            },
        )
        .await
        .unwrap();

    let (left, right) = (connect().await, connect().await);
    let left_store = store.clone();
    let right_store = store.clone();
    let (left, right) = tokio::join!(
        tokio::spawn(async move { left_store.claim(&left, "issuer|subject", "work", "worker-left", 100).await }),
        tokio::spawn(async move { right_store.claim(&right, "issuer|subject", "work", "worker-right", 100).await }),
    );
    let left = left.unwrap().unwrap();
    let right = right.unwrap().unwrap();
    assert_eq!(left.is_some() as usize + right.is_some() as usize, 1);
    let stale = left.or(right).unwrap();
    assert_eq!(stale.id, created.id);

    setup
        .execute(
            &format!("UPDATE {schema}.aip_messages SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1::text::uuid"),
            &[&created.id],
        )
        .await
        .unwrap();
    let replacement = store.claim(&setup, "issuer|subject", "work", "worker-next", 100).await.unwrap().unwrap();
    assert!(replacement.epoch > stale.epoch);
    assert!(!store.ack(&setup, &stale).await.unwrap(), "stale lease must not acknowledge its successor");
    assert!(store.ack(&setup, &replacement).await.unwrap());
    setup.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnecting_partial_request_releases_the_only_admission_slot() {
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    let infrastructure = Infrastructure { resource_limits: ResourceLimits { connections: 1, ..ResourceLimits::default() }, ..Default::default() };
    let server = listen_with_infrastructure(
        facts,
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        DB.into(),
        None,
        ServerOptions::default(),
        false,
        std::sync::Arc::new(RejectAuth),
        None,
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();

    let mut stalled = TcpStream::connect(server.address).await.unwrap();
    stalled.write_all(format!("POST /session HTTP/1.1\r\nhost: {}\r\n", server.address).as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let rejected = session_request(server.address).await;
    assert!(rejected.is_empty(), "a second request must be rejected while the only connection slot is occupied");

    drop(stalled);
    // The permit returns when the server notices the closed peer, which takes a moment under load; the claim is that it
    // does return, not that it returns before the very next packet.
    let mut recovered = Vec::new();
    for _ in 0..40 {
        recovered = session_request(server.address).await;
        if recovered.starts_with(b"HTTP/1.1 200 OK\r\n") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        recovered.starts_with(b"HTTP/1.1 200 OK\r\n"),
        "connection permit should return after peer disconnect: {}",
        String::from_utf8_lossy(&recovered)
    );
    server.shutdown().await.unwrap();
}
