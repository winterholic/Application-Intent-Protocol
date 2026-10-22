use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
    socketio::SocketIoOptions,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, protocol::frame::coding::CloseCode, Message};

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
        let cleanup = std::thread::Builder::new().name("aip-socketio-cleanup".into()).spawn(move || {
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

async fn start_server(socketio: Option<SocketIoOptions>, connections: usize) -> (spike_v6_transport::server::Server, SchemaCleanup) {
    let schema = format!("aip_socketio_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','watcher',1)"
    ))
    .await
    .unwrap();
    let issued_at: i64 = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    drop(db);
    let mut infrastructure = Infrastructure::default();
    infrastructure.realtime.max_connections = connections;
    infrastructure.socketio = socketio;
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
    (server, cleanup)
}

fn options() -> SocketIoOptions {
    SocketIoOptions { max_packet_bytes: 8 * 1024, connect_timeout: Duration::from_secs(2), ..Default::default() }
}

const TARGET: &str = "/socket.io/?EIO=4&transport=websocket";

fn request(address: SocketAddr, target: &str, origin: bool) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{address}{target}").into_client_request().unwrap();
    if origin {
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
    }
    request
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_text(socket: &mut Socket) -> String {
    match tokio::time::timeout(Duration::from_secs(5), socket.next()).await.expect("answer in time") {
        Some(Ok(Message::Text(text))) => text.to_string(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn next_close(socket: &mut Socket) -> Option<CloseCode> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next()).await.expect("close in time") {
            Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code),
            Some(Ok(Message::Text(_))) => continue,
            _ => return None,
        }
    }
}

fn frame_event(ack: u32, extra: serde_json::Value) -> String {
    let mut payload = json!({
        "video_view_log_id": "11111111-1111-4111-8111-111111111111",
        "youtube_running_time": 1.5,
        "frame_data": "data:image/jpeg;base64,AAAA",
        "duration": 60,
        "user_id": "claimed-by-the-client",
        "video_id": "22222222-2222-4222-8222-222222222222"
    });
    for (key, value) in extra.as_object().unwrap() {
        payload[key] = value.clone();
    }
    format!("42{ack}{}", json!(["watch_frame", payload]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socketio_endpoint_is_not_served_unless_it_is_configured() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, _cleanup) = start_server(None, 1).await;
    assert!(tokio_tungstenite::connect_async(request(server.address, TARGET, true)).await.is_err());
    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_upgrade_accepts_only_a_browser_websocket_only_engine_io_v4_request() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, _cleanup) = start_server(Some(options()), 1).await;
    assert!(tokio_tungstenite::connect_async(request(server.address, TARGET, false)).await.is_err(), "Origin is required");
    let mut foreign = request(server.address, TARGET, true);
    foreign.headers_mut().insert("origin", "https://untrusted.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(foreign).await.is_err(), "a disallowed Origin is refused before the upgrade");
    for target in [
        "/socket.io/?EIO=4&transport=polling",
        "/socket.io/?EIO=3&transport=websocket",
        "/socket.io/?EIO=4&transport=websocket&sid=abc",
        "/socket.io/",
    ] {
        assert!(tokio_tungstenite::connect_async(request(server.address, target, true)).await.is_err(), "{target}");
    }
    let mut with_header_token = request(server.address, TARGET, true);
    with_header_token.headers_mut().insert("authorization", "Bearer watcher".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(with_header_token).await.is_err(), "the token belongs in the CONNECT packet");

    // A browser offers permessage-deflate; the server declines it instead of refusing the connection.
    let mut browser = request(server.address, TARGET, true);
    browser.headers_mut().insert("sec-websocket-extensions", "permessage-deflate; client_max_window_bits".parse().unwrap());
    let (mut socket, response) = tokio_tungstenite::connect_async(browser).await.expect("an extension offer is not a refusal");
    assert!(response.headers().get("sec-websocket-extensions").is_none(), "no extension is accepted");
    assert!(next_text(&mut socket).await.starts_with("0{"));
    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_authenticates_with_the_real_principal_check_and_a_frame_needs_a_relational_deployment() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, _cleanup) = start_server(Some(options()), 1).await;

    // Unknown subject: the product principal check refuses it and the connection ends.
    let (mut unknown, _) = tokio_tungstenite::connect_async(request(server.address, TARGET, true)).await.unwrap();
    assert!(next_text(&mut unknown).await.starts_with("0{"));
    unknown.send(Message::Text(r#"40{"token":"nobody"}"#.into())).await.unwrap();
    let refusal = next_text(&mut unknown).await;
    assert!(refusal.starts_with("44") && refusal.contains("\"WIRE.UNAUTHENTICATED\""), "{refusal}");
    assert_eq!(next_close(&mut unknown).await, Some(CloseCode::Policy));

    let (mut socket, _) = tokio_tungstenite::connect_async(request(server.address, TARGET, true)).await.unwrap();
    assert!(
        tokio_tungstenite::connect_async(request(server.address, TARGET, true)).await.is_err(),
        "a second live socket is refused at the configured connection cap"
    );
    let open: serde_json::Value = serde_json::from_str(&next_text(&mut socket).await[1..]).unwrap();
    assert_eq!((open["upgrades"].clone(), open["maxPayload"].clone()), (json!([]), json!(8 * 1024)));
    socket.send(Message::Text(r#"40{"token":"watcher"}"#.into())).await.unwrap();
    let connected = next_text(&mut socket).await;
    assert!(connected.starts_with("40{\"sid\":\""), "{connected}");

    socket.send(Message::Text(frame_event(1, json!({})).into())).await.unwrap();
    let reply = next_text(&mut socket).await;
    let body: serde_json::Value = serde_json::from_str(reply.strip_prefix("431").expect("ack 1")).unwrap();
    assert_eq!((body[0]["status"].clone(), body[0]["code"].clone()), (json!("error"), json!("WIRE.ADAPTER_UNSUPPORTED")));
    assert!(body[0].get("response").is_none());

    socket.send(Message::Text(frame_event(2, json!({"video_id": ""})).into())).await.unwrap();
    let reply = next_text(&mut socket).await;
    assert_eq!(reply, r#"432[{"code":"WIRE.BAD_REQUEST","message":"Missing required fields","status":"error"}]"#);

    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_and_oversized_packets_close_only_the_transport_and_return_the_slot() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, _cleanup) = start_server(Some(options()), 1).await;
    let connect = |address| async move {
        let (mut socket, _) = tokio_tungstenite::connect_async(request(address, TARGET, true)).await.unwrap();
        next_text(&mut socket).await;
        socket.send(Message::Text(r#"40{"token":"watcher"}"#.into())).await.unwrap();
        assert!(next_text(&mut socket).await.starts_with("40{"));
        socket
    };
    let mut socket = connect(server.address).await;
    socket.send(Message::Text("41".into())).await.unwrap();
    assert_eq!(next_close(&mut socket).await, Some(CloseCode::Normal));
    drop(socket);

    // The cap is 1: the next connection only succeeds because the slot came back after the disconnect.
    let mut socket = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok((socket, _)) = tokio_tungstenite::connect_async(request(server.address, TARGET, true)).await {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("slot returns after a disconnect");
    next_text(&mut socket).await;
    socket.send(Message::Text(r#"40{"token":"watcher"}"#.into())).await.unwrap();
    assert!(next_text(&mut socket).await.starts_with("40{"));
    let _ = socket.send(Message::Text(frame_event(1, json!({"frame_data": "A".repeat(9000)})).into())).await;
    assert_eq!(next_close(&mut socket).await, Some(CloseCode::Size));
    drop(socket);

    // Dropping the TCP connection without any packet also frees the slot.
    let socket = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok((socket, _)) = tokio_tungstenite::connect_async(request(server.address, TARGET, true)).await {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("slot returns after an oversized packet");
    drop(socket);
    server.shutdown().await.unwrap();
}

/// The real `socket.io-client` 4.8.3 (the version the FaceReview player locks) against this server. Install it first:
/// `npm ci --ignore-scripts` in `tests/socketio-client`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the pinned socket.io-client (npm ci in tests/socketio-client) and Node; run with -- --ignored"]
async fn the_real_socketio_client_connects_emits_watch_frame_and_disconnects() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let (server, _cleanup) = start_server(Some(options()), 2).await;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/socketio-client");
    let package = format!("{dir}/node_modules/socket.io-client/package.json");
    assert!(std::path::Path::new(&package).exists(), "run npm ci --ignore-scripts in {dir}");
    let version: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&package).unwrap()).unwrap();
    assert_eq!(version["version"], "4.8.3");
    let origin = format!("http://{}", server.address);
    let output = tokio::process::Command::new("node")
        .args(["check.mjs", &server.address.to_string(), "watcher", &origin])
        .current_dir(dir)
        .output()
        .await
        .expect("node runs");
    assert!(output.status.success(), "node failed: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let last = output.stdout.split(|b| *b == b'\n').rev().find(|line| !line.is_empty()).unwrap();
    let seen: serde_json::Value = serde_json::from_slice(last).unwrap();
    println!("socket.io-client observed: {seen}");
    assert_eq!(seen["refused"], json!({"connected": false, "message": "WIRE.UNAUTHENTICATED", "data": {"code": "WIRE.UNAUTHENTICATED"}}));
    assert_eq!(seen["connected"]["id"].as_str().map(str::len), Some(20));
    assert_eq!((seen["valid"]["status"].clone(), seen["valid"]["code"].clone()), (json!("error"), json!("WIRE.ADAPTER_UNSUPPORTED")));
    assert!(seen["valid"].get("response").is_none());
    assert_eq!(seen["missing"], json!({"status": "error", "code": "WIRE.BAD_REQUEST", "message": "Missing required fields"}));
    assert_eq!(seen["unknownEvent"]["message"], "Unknown event");
    assert_eq!(seen["disconnectReason"], "io client disconnect");
    server.shutdown().await.unwrap();
}
