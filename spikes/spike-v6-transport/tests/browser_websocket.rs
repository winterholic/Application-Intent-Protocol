use aip_execution::events::Store as EventStore;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Auth {
    schema: String,
    issued_at: i64,
}

impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let grant = DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: 1, issued_at: self.issued_at, min_iat: 0 };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.issued_at + 600)
        })
    }

    fn verify_durable<'a>(
        &'a self,
        grant: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.map_err(|_| AuthError::Unavailable)?;
            let row = db
                .query_opt(
                    &format!("SELECT enabled,min_iat FROM {}.aip_principals WHERE issuer=$1 AND subject=$2 AND actor_id=$3", self.schema),
                    &[&grant.issuer, &grant.subject, &grant.actor_id],
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            if row.is_some_and(|row| row.get::<_, bool>(0) && row.get::<_, i64>(1) == grant.min_iat) {
                Ok(())
            } else {
                Err(AuthError::BadSignature)
            }
        })
    }
}

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::Builder::new().name("aip-browser-ws-cleanup".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        if let Ok(cleanup) = cleanup {
            let _ = cleanup.join();
        }
    }
}

async fn start_server() -> (spike_v6_transport::server::Server, String, SchemaCleanup) {
    let schema = format!("aip_browser_ws_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let mut db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','browser-owner',1)"
    ))
    .await
    .unwrap();
    let issued_at: i64 = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let owner = digest(&json!(["test", "browser-owner", 1, 0]));
    EventStore::new(&schema).unwrap().publish(&mut db, &owner, "notifications", "event-1", json!({"message":"browser"})).await.unwrap();
    drop(db);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let mut infrastructure = Infrastructure::default();
    infrastructure.realtime.max_lifetime = Duration::from_secs(10);
    infrastructure.realtime.max_connections = 1;
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), issued_at }),
        Some(DeploymentFence::new(schema, digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();
    (server, fingerprint, cleanup)
}

fn browser_request(address: SocketAddr, origin: bool) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{address}/events/socket").into_client_request().unwrap();
    if origin {
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
    }
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_authenticates_in_first_frame_before_subscribing_and_origin_is_required() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, fingerprint, cleanup) = start_server().await;

    let no_origin = tokio_tungstenite::connect_async(browser_request(server.address, false)).await;
    assert!(no_origin.is_err(), "browser WebSocket handshake without Origin must be rejected");

    let mut disallowed_origin = browser_request(server.address, true);
    disallowed_origin.headers_mut().insert("origin", "https://untrusted.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(disallowed_origin).await.is_err(), "disallowed Origin must be rejected before upgrade");

    let (mut socket, _) = tokio_tungstenite::connect_async(browser_request(server.address, true)).await.unwrap();
    assert!(
        tokio_tungstenite::connect_async(browser_request(server.address, true)).await.is_err(),
        "a second live realtime socket must be rejected at the configured cap"
    );
    socket.send(Message::Text(json!({"type":"auth","token":"browser-owner"}).to_string().into())).await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&ack.into_text().unwrap()).unwrap(), json!({"type":"authenticated"}));

    socket
        .send(Message::Text(json!({"type":"subscribe","stream":"notifications","after":"0","contractFingerprint":fingerprint}).to_string().into()))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
    let event: serde_json::Value = serde_json::from_str(&event.into_text().unwrap()).unwrap();
    assert_eq!(event["type"], "event");
    assert_eq!(event["cursor"], "1");
    assert_eq!(event["payload"]["message"], "browser");

    let mut db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    let owner = digest(&json!(["test", "browser-owner", 1, 0]));
    let bulk_payload = json!({"body":"x".repeat(60 * 1024)});
    let events = EventStore::new(&cleanup.0).unwrap();
    for index in 0..128 {
        events.publish(&mut db, &owner, "notifications", &format!("bulk-{index}"), bulk_payload.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(socket); // A reader that stops draining must not keep its connection permit after disconnect.

    let mut replacement = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok((socket, _)) = tokio_tungstenite::connect_async(browser_request(server.address, true)).await {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("realtime slot should return after a slow consumer disconnects");
    replacement.send(Message::Text(json!({"type":"auth","token":"browser-owner"}).to_string().into())).await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(5), replacement.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&ack.into_text().unwrap()).unwrap(), json!({"type":"authenticated"}));

    db.execute(&format!("UPDATE {}.aip_principals SET enabled=false WHERE issuer='test' AND subject='browser-owner'", cleanup.0), &[]).await.unwrap();
    replacement
        .send(Message::Text(json!({"type":"subscribe","stream":"notifications","after":"1","contractFingerprint":fingerprint}).to_string().into()))
        .await
        .unwrap();
    let revoked = tokio::time::timeout(Duration::from_secs(5), replacement.next()).await.unwrap().unwrap().unwrap();
    let revoked: serde_json::Value = serde_json::from_str(&revoked.into_text().unwrap()).unwrap();
    assert_eq!(revoked["type"], "error");
    assert_eq!(revoked["code"], "WIRE.UNAUTHENTICATED");
    let _ = tokio::time::timeout(Duration::from_secs(2), replacement.next()).await;
    drop(db);
    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_auth_frame_is_bounded_and_invalid_auth_never_reaches_subscription() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, fingerprint, _cleanup) = start_server().await;
    let (mut missing_auth, _) = tokio_tungstenite::connect_async(browser_request(server.address, true)).await.unwrap();
    missing_auth
        .send(Message::Text(json!({"type":"subscribe","stream":"notifications","after":"0","contractFingerprint":fingerprint}).to_string().into()))
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(3), missing_auth.next()).await.unwrap().unwrap().unwrap();
    let first: serde_json::Value = serde_json::from_str(&first.into_text().unwrap()).unwrap();
    assert_eq!(first["type"], "error");
    assert_eq!(first["code"], "WIRE.UNAUTHENTICATED");

    let (mut socket, _) = tokio_tungstenite::connect_async(browser_request(server.address, true)).await.unwrap();
    socket.send(Message::Text(json!({"type":"auth","token":"revoked-or-unknown"}).to_string().into())).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
    let first: serde_json::Value = serde_json::from_str(&first.into_text().unwrap()).unwrap();
    assert_eq!(first["type"], "error");
    assert_eq!(first["code"], "WIRE.UNAUTHENTICATED");
    let next = tokio::time::timeout(Duration::from_secs(2), socket.next()).await.unwrap();
    assert!(matches!(next, Some(Ok(Message::Close(_))) | None));

    let (mut oversized, _) = tokio_tungstenite::connect_async(browser_request(server.address, true)).await.unwrap();
    oversized.send(Message::Text(format!("{{\"type\":\"auth\",\"token\":\"{}\"}}", "x".repeat(20 * 1024)).into())).await.unwrap();
    let oversized_reply = tokio::time::timeout(Duration::from_secs(3), oversized.next()).await.unwrap();
    assert!(matches!(oversized_reply, None | Some(Err(_)) | Some(Ok(Message::Close(_)))), "oversized first frame must close: {oversized_reply:?}");
    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthenticated_browser_sockets_do_not_consume_authenticated_realtime_slots() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, _fingerprint, _cleanup) = start_server().await;
    // max_connections = 1: 인증 프레임을 보내지 않는 익명 소켓이 정식 슬롯을 쥐면 인증된 사용자가 SERVER_BUSY를 받는다.
    let (_anonymous, _) = tokio_tungstenite::connect_async(browser_request(server.address, true)).await.unwrap();
    let mut authenticated = browser_request(server.address, true);
    authenticated.headers_mut().insert("authorization", "Bearer browser-owner".parse().unwrap());
    let result = tokio_tungstenite::connect_async(authenticated).await;
    assert!(result.is_ok(), "an authenticated socket must not be starved by anonymous pre-auth sockets: {:?}", result.err());
    server.shutdown().await.unwrap();
}
