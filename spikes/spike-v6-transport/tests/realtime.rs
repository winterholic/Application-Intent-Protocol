use aip_execution::events::Store as EventStore;
use serde_json::json;
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::realtime::RealTimeOptions;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[test]
fn realtime_defaults_are_enabled_bounded_and_usable() {
    let options = RealTimeOptions::default();

    assert!(options.enabled);
    assert_eq!(options.max_connections, 32);
    assert_eq!(options.max_lifetime, Duration::from_secs(15 * 60));
    assert_eq!(options.heartbeat_interval, Duration::from_secs(15));
    assert_eq!(options.poll_interval, Duration::from_millis(250));
    options.validate().unwrap();
}

#[test]
fn realtime_rejects_unbounded_or_invalid_limits() {
    let options = RealTimeOptions { max_connections: 0, ..Default::default() };
    assert!(options.validate().is_err());

    let options = RealTimeOptions { max_lifetime: Duration::ZERO, ..Default::default() };
    assert!(options.validate().is_err());

    let options = RealTimeOptions { heartbeat_interval: Duration::ZERO, ..Default::default() };
    assert!(options.validate().is_err());

    let options = RealTimeOptions { max_connections: 1025, ..Default::default() };
    assert!(options.validate().is_err());
}

static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        grant: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.map_err(|_| AuthError::Unavailable)?;
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
        let cleanup = std::thread::Builder::new().name("aip-realtime-schema-cleanup".into()).spawn(move || {
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

async fn sse_event(address: SocketAddr, fingerprint: &str, after: &str) -> String {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let body = json!({"stream":"notifications","after":after,"contractFingerprint":fingerprint}).to_string();
    socket.write_all(format!(
        "POST /events/subscribe HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer owner\r\naccept: text/event-stream\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()
    ).as_bytes()).await.unwrap();
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await.unwrap().unwrap();
    assert!(line.starts_with("HTTP/1.1 200"), "{line}");
    let mut headers = Vec::new();
    loop {
        line.clear();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await.unwrap().unwrap();
        if line == "\r\n" {
            break;
        }
        headers.push(line.trim_end().to_owned());
    }
    assert!(headers.iter().any(|header| header.eq_ignore_ascii_case("connection: keep-alive")), "{headers:?}");
    assert!(headers.iter().any(|header| header.eq_ignore_ascii_case("vary: Origin")), "{headers:?}");
    assert!(headers.iter().any(|header| header.to_ascii_lowercase().starts_with("x-request-id: req_")), "an event stream is traceable: {headers:?}");
    let mut frame = String::new();
    loop {
        line.clear();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await.unwrap().unwrap();
        frame.push_str(&line);
        if line == "\n" || line == "\r\n" {
            break;
        }
    }
    frame
}

fn subscription(fingerprint: &str, after: &str) -> String {
    json!({"stream":"notifications","after":after,"contractFingerprint":fingerprint}).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_and_websocket_replay_the_same_owner_scoped_event() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_realtime_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let mut db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1),('test','other',1)")).await.unwrap();
    let iat: i64 = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let owner = digest(&json!(["test", "owner", 1, 0]));
    EventStore::new(&schema).unwrap().publish(&mut db, &owner, "notifications", "event-1", json!({"message":"hello"})).await.unwrap();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let mut infrastructure = Infrastructure::default();
    infrastructure.realtime.max_lifetime = Duration::from_secs(2);
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        auth,
        Some(DeploymentFence::new(schema.clone(), digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();

    let sse = sse_event(server.address, &fingerprint, "0").await;
    assert!(sse.contains("id: 1\n"), "{sse}");
    assert!(sse.contains("data: {\"message\":\"hello\"}"), "{sse}");

    let mut request = format!("ws://{}/events/socket", server.address).into_client_request().unwrap();
    request.headers_mut().insert("authorization", "Bearer owner".parse().unwrap());
    request.headers_mut().insert("origin", format!("http://{}", server.address).parse().unwrap());
    let (mut websocket, upgrade) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert!(
        upgrade.headers().get("x-request-id").and_then(|value| value.to_str().ok()).is_some_and(|id| id.starts_with("req_")),
        "the upgrade reply is traceable: {:?}",
        upgrade.headers()
    );
    use futures_util::{SinkExt, StreamExt};
    websocket.send(tokio_tungstenite::tungstenite::Message::Text(subscription(&fingerprint, "0").into())).await.unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), websocket.next()).await.unwrap().unwrap().unwrap();
    let text = message.into_text().unwrap();
    assert!(text.contains("\"cursor\":\"1\""), "{text}");
    assert!(text.contains("\"message\":\"hello\""), "{text}");

    let mut other_request = format!("ws://{}/events/socket", server.address).into_client_request().unwrap();
    other_request.headers_mut().insert("authorization", "Bearer other".parse().unwrap());
    other_request.headers_mut().insert("origin", format!("http://{}", server.address).parse().unwrap());
    let (mut other_socket, _) = tokio_tungstenite::connect_async(other_request).await.unwrap();
    other_socket.send(tokio_tungstenite::tungstenite::Message::Text(subscription(&fingerprint, "0").into())).await.unwrap();
    let other_message = tokio::time::timeout(Duration::from_millis(400), other_socket.next()).await;
    assert!(other_message.is_err(), "another owner's event must not be delivered: {other_message:?}");
    drop(other_socket);

    EventStore::new(&schema).unwrap().prune(&mut db, &owner, "notifications", "1").await.unwrap();
    let expired = sse_event(server.address, &fingerprint, "0").await;
    assert!(expired.contains("event: error\n"), "{expired}");
    assert!(expired.contains("CURSOR_EXPIRED"), "{expired}");

    db.execute(&format!("UPDATE {schema}.aip_principals SET enabled=false WHERE issuer='test' AND subject='owner'"), &[]).await.unwrap();
    let revoked = tokio::time::timeout(Duration::from_secs(5), websocket.next()).await.unwrap().unwrap().unwrap();
    let revoked = revoked.into_text().unwrap();
    assert!(revoked.contains("UNAUTHENTICATED"), "{revoked}");

    let mut lifetime_request = format!("ws://{}/events/socket", server.address).into_client_request().unwrap();
    lifetime_request.headers_mut().insert("authorization", "Bearer other".parse().unwrap());
    lifetime_request.headers_mut().insert("origin", format!("http://{}", server.address).parse().unwrap());
    let (mut lifetime_socket, _) = tokio_tungstenite::connect_async(lifetime_request).await.unwrap();
    lifetime_socket.send(tokio_tungstenite::tungstenite::Message::Text(subscription(&fingerprint, "0").into())).await.unwrap();
    let close = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) = lifetime_socket.next().await {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(close.unwrap().reason, "lifetime limit");

    server.shutdown().await.unwrap();
}
