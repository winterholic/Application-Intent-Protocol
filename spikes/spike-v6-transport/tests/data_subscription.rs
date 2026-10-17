//! A caller can keep a read open and hear when its result changes (H04). The model is the PoC `subscribe` (D-P24):
//! the whole result is sent first, then again whenever a re-run as the subscriber gives a different one, and the
//! re-run is the full check of any read, so a subscriber who lost access gets an error and nothing more.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    realtime::RealTimeOptions,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, Server, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

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

/// A token is the subject of a bound principal; the actor is whatever that principal is bound to.
struct Auth {
    schema: String,
    iat: i64,
}
impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.map_err(|_| AuthError::Unavailable)?;
            let row = db
                .query_opt(&format!("SELECT actor_id,min_iat FROM {}.aip_principals WHERE issuer='test' AND subject=$1", self.schema), &[&token])
                .await
                .map_err(|_| AuthError::Unavailable)?
                .ok_or(AuthError::BadSignature)?;
            let grant =
                DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: row.get(0), issued_at: self.iat, min_iat: row.get(1) };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.iat + 600)
        })
    }
    fn verify_durable<'a>(
        &'a self,
        g: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.map_err(|_| AuthError::Unavailable)?;
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

struct Fixture {
    schema: String,
    db: spike_v2_read::OwnedConnection,
    facts: Value,
    fingerprint: String,
    auth: Arc<Auth>,
    _cleanup: SchemaCleanup,
}

async fn fixture() -> Fixture {
    fixture_of(include_str!("fixtures/data-subscription.aip")).await
}

async fn fixture_of(definition: &str) -> Fixture {
    let schema = format!("aip_data_sub_{}", std::process::id());
    sqlgen::try_set_schema(&schema).expect("schema");
    let cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(definition, Form::A).expect("definition").execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.expect("init");
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.expect("db");
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); \
         INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1),('test','other',2); \
         INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(11,1,false),(12,1,false),(98,2,false),(99,2,false)"
    ))
    .await
    .expect("seed");
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.expect("clock").get(0);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    Fixture { auth: Arc::new(Auth { schema: schema.clone(), iat }), schema, db, facts, fingerprint, _cleanup: cleanup }
}

async fn start(fixture: &Fixture, query_poll_interval: Duration) -> Server {
    start_with(fixture, query_poll_interval, None).await
}

async fn start_with(fixture: &Fixture, query_poll_interval: Duration, extensions: Option<ReadExtensions>) -> Server {
    let realtime = RealTimeOptions { query_poll_interval, ..RealTimeOptions::default() };
    listen_with_infrastructure(
        fixture.facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().expect("address"),
        aip_test_support::test_database_url!().into(),
        extensions,
        ServerOptions::default(),
        false,
        fixture.auth.clone(),
        Some(DeploymentFence::new(fixture.schema.clone(), digest(&fixture.facts)).expect("fence")),
        None,
        false,
        Infrastructure { realtime, ..Infrastructure::default() },
    )
    .await
    .expect("server")
}

async fn post(addr: SocketAddr, path: &str, fingerprint: &str, token: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("send");
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.expect("in time").expect("read");
    serde_json::from_str(String::from_utf8(out).expect("utf-8").split_once("\r\n\r\n").expect("head and body").1).expect("json")
}

fn query() -> Value {
    json!({"read":"Order","select":["id","paid"],"sort":[{"field":"id"}]})
}

fn pay(addr: SocketAddr, fingerprint: &str, token: &str, id: i64, key: &str) -> impl std::future::Future<Output = Value> {
    let (fingerprint, token, key) = (fingerprint.to_owned(), token.to_owned(), key.to_owned());
    async move { post(addr, "/apply", &fingerprint, &token, json!({"key":key,"request":{"apply":"Order.pay","target":{"ids":[id]}}})).await }
}

/// An open `POST /events/subscribe` and the frames read from it.
struct Sse {
    reader: BufReader<TcpStream>,
}

impl Sse {
    /// `Err` is the JSON reply of a request the server refused before it opened a stream.
    async fn open(addr: SocketAddr, token: &str, subscription: Value) -> Result<Self, Value> {
        let mut socket = TcpStream::connect(addr).await.expect("connect");
        let body = subscription.to_string();
        socket
            .write_all(
                format!(
                    "POST /events/subscribe HTTP/1.1\r\nhost: {addr}\r\nauthorization: Bearer {token}\r\naccept: text/event-stream\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .expect("send");
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("status line");
        assert!(line.starts_with("HTTP/1.1 200"), "{line}");
        let mut streaming = false;
        loop {
            line.clear();
            reader.read_line(&mut line).await.expect("header");
            if line == "\r\n" {
                break;
            }
            streaming |= line.to_ascii_lowercase().starts_with("content-type: text/event-stream");
        }
        if !streaming {
            let mut body = String::new();
            tokio::time::timeout(Duration::from_secs(5), reader.read_to_string(&mut body)).await.expect("in time").expect("body");
            return Err(serde_json::from_str(&body).expect("a JSON refusal"));
        }
        Ok(Self { reader })
    }

    /// The next `message` or `error` event as (event, data), or `None` when nothing arrives in `within`.
    async fn next(&mut self, within: Duration) -> Option<(String, Value)> {
        let deadline = tokio::time::Instant::now() + within;
        let (mut event, mut data) = (String::new(), String::new());
        loop {
            let mut line = String::new();
            match tokio::time::timeout_at(deadline, self.reader.read_line(&mut line)).await {
                Err(_) | Ok(Ok(0)) | Ok(Err(_)) => return None,
                Ok(Ok(_)) => {}
            }
            let line = line.trim_end();
            if let Some(name) = line.strip_prefix("event: ") {
                event = name.to_owned();
            } else if let Some(text) = line.strip_prefix("data: ") {
                data.push_str(text);
            } else if line.is_empty() && !event.is_empty() {
                return Some((event, serde_json::from_str(&data).expect("event data is JSON")));
            }
        }
    }
}

/// The next text frame of the socket, or `None` when nothing arrives in `within`.
async fn next_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    within: Duration,
) -> Option<Value> {
    loop {
        match tokio::time::timeout(within, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => return Some(serde_json::from_str::<Value>(&text).expect("json frame")),
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

fn rows(frame: &Option<(String, Value)>) -> (String, Value) {
    let (event, data) = frame.clone().expect("a frame arrived");
    assert_eq!(event, "message", "{data}");
    (data["type"].as_str().unwrap_or("").to_owned(), data["rows"].clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscriber_gets_a_snapshot_then_a_changed_result_after_a_write_and_nothing_for_invisible_or_identical_results() {
    let _lock = SCHEMA_LOCK.lock().await;
    let f = fixture().await;
    // The safety-net poll is far away, so a frame within a second can only come from the commit that caused it.
    let server = start(&f, Duration::from_secs(30)).await;
    let addr = server.address;

    let mut owner = Sse::open(addr, "owner", json!({"query":query(),"contractFingerprint":f.fingerprint})).await.expect("a stream");
    let (kind, snapshot) = rows(&owner.next(Duration::from_secs(5)).await);
    assert_eq!(kind, "snapshot");
    // Only the rows the subscriber may read: order 99 belongs to another member.
    assert_eq!(snapshot, json!([{"id":11,"paid":false},{"id":12,"paid":false}]));

    let started = tokio::time::Instant::now();
    let applied = pay(addr, &f.fingerprint, "owner", 11, "pay-11").await;
    assert_eq!(applied["changed"], json!([11]), "{applied}");
    let (kind, changed) = rows(&owner.next(Duration::from_secs(5)).await);
    assert_eq!((kind.as_str(), changed), ("changed", json!([{"id":11,"paid":true},{"id":12,"paid":false}])));
    assert!(started.elapsed() < Duration::from_secs(5), "the commit woke the subscription instead of waiting for the poll");

    // Another member's write wakes the re-run, but the subscriber's result is the same, so nothing is sent.
    let other = pay(addr, &f.fingerprint, "other", 99, "pay-99").await;
    assert_eq!(other["changed"], json!([99]), "{other}");
    assert!(owner.next(Duration::from_millis(900)).await.is_none(), "an unchanged result is not sent again");

    // A write the server never saw (another instance, a script) is found by the safety-net poll of a second server.
    let polling = start(&f, Duration::from_secs(1)).await;
    let mut watcher = Sse::open(polling.address, "owner", json!({"query":query(),"contractFingerprint":f.fingerprint})).await.expect("a stream");
    assert_eq!(rows(&watcher.next(Duration::from_secs(5)).await).0, "snapshot");
    f.db.batch_execute(&format!("UPDATE {}.\"order\" SET paid=true WHERE id=12", f.schema)).await.expect("external write");
    let (kind, changed) = rows(&watcher.next(Duration::from_secs(5)).await);
    assert_eq!((kind.as_str(), changed), ("changed", json!([{"id":11,"paid":true},{"id":12,"paid":true}])));

    // Access is checked again on every re-run: a revoked principal gets an error and the stream ends.
    f.db.batch_execute(&format!("UPDATE {}.aip_principals SET enabled=false WHERE subject='owner'", f.schema)).await.expect("revoke");
    let _ = pay(addr, &f.fingerprint, "other", 98, "pay-98").await;
    let (event, data) = owner.next(Duration::from_secs(5)).await.expect("an error frame");
    assert_eq!((event.as_str(), data["code"].as_str()), ("error", Some("WIRE.UNAUTHENTICATED")), "{data}");
    assert!(owner.next(Duration::from_secs(2)).await.is_none(), "nothing follows the error");
    polling.shutdown().await.expect("shutdown");
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_or_overreaching_data_subscription_is_refused_before_any_data_is_sent() {
    let _lock = SCHEMA_LOCK.lock().await;
    let f = fixture().await;
    let server = start(&f, Duration::from_secs(30)).await;
    let addr = server.address;
    let fp = &f.fingerprint;
    let refusals = [
        // Both a stream and a query, or neither.
        (json!({"stream":"notifications","after":"0","query":query(),"contractFingerprint":fp}), "WIRE.BAD_REQUEST"),
        (json!({"contractFingerprint":fp}), "WIRE.BAD_REQUEST"),
        // A query is not resumable: `after` is meaningless and must not pretend to be a cursor.
        (json!({"after":"7","query":query(),"contractFingerprint":fp}), "WIRE.BAD_REQUEST"),
        // A query outside the contract is refused the way a read is, with the allowed names.
        (json!({"query":{"read":"Order","select":["id"],"sort":[{"field":"paid"}]},"contractFingerprint":fp}), "WIRE.SORT_NOT_ALLOWED"),
        (json!({"query":{"read":"Nope","select":["id"]},"contractFingerprint":fp}), "WIRE.UNKNOWN_RESOURCE"),
        // The caller's contract must be the server's.
        (json!({"query":query(),"contractFingerprint":"0".repeat(64)}), "WIRE.CONTRACT_MISMATCH"),
    ];
    for (subscription, code) in refusals {
        // A request the server can refuse by reading it is answered with a JSON reply; one that only a run can refuse
        // (a query outside the contract) opens the stream and ends it with an error event.
        let answered = match Sse::open(addr, "owner", subscription.clone()).await {
            Err(reply) => reply["code"].as_str().map(str::to_owned),
            Ok(mut sse) => {
                let (event, data) = sse.next(Duration::from_secs(5)).await.unwrap_or_else(|| panic!("{subscription}: no frame"));
                assert_eq!(event, "error", "{subscription}: {data}");
                data["code"].as_str().map(str::to_owned)
            }
        };
        assert_eq!(answered.as_deref(), Some(code), "{subscription}");
    }
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_subscriber_gets_the_same_snapshot_and_changes() {
    let _lock = SCHEMA_LOCK.lock().await;
    let f = fixture().await;
    let server = start(&f, Duration::from_secs(30)).await;
    let addr = server.address;
    let mut request = format!("ws://{addr}/events/socket").into_client_request().expect("request");
    request.headers_mut().insert("authorization", "Bearer owner".parse().expect("header"));
    request.headers_mut().insert("origin", format!("http://{addr}").parse().expect("header"));
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.expect("upgrade");
    socket.send(Message::Text(json!({"query":query(),"contractFingerprint":f.fingerprint}).to_string().into())).await.expect("subscribe");
    let first = next_frame(&mut socket, Duration::from_secs(5)).await.expect("a frame");
    assert_eq!((first["type"].as_str(), first["payload"]["type"].as_str()), (Some("event"), Some("snapshot")), "{first}");
    assert_eq!(first["payload"]["rows"], json!([{"id":11,"paid":false},{"id":12,"paid":false}]));
    let _ = pay(addr, &f.fingerprint, "owner", 12, "pay-12").await;
    let second = next_frame(&mut socket, Duration::from_secs(5)).await.expect("a frame");
    assert_eq!(second["payload"]["type"], "changed", "{second}");
    assert_eq!(second["payload"]["rows"], json!([{"id":11,"paid":false},{"id":12,"paid":true}]));
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_made_by_a_durable_job_wakes_a_subscription_too() {
    let _lock = SCHEMA_LOCK.lock().await;
    let f = fixture_of(include_str!("fixtures/job-data.aip")).await;
    let server = start_with(
        &f,
        Duration::from_secs(30),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-data").into() }),
    )
    .await;
    let addr = server.address;
    let mut owner = Sse::open(addr, "owner", json!({"query":query(),"contractFingerprint":f.fingerprint})).await.expect("a stream");
    assert_eq!(rows(&owner.next(Duration::from_secs(5)).await).0, "snapshot");
    // The job pays every order the owner can see (11 and 12) with `ctx.data.apply`; the poll is 30 s away.
    let queued =
        post(addr, "/jobs/enqueue", &f.fingerprint, "owner", json!({"operation":"settle","input":{"other":99},"key":"settle","delayMs":0})).await;
    assert!(queued["run"]["id"].is_string(), "{queued}");
    let mut last = json!(null);
    for _ in 0..40 {
        let Some(frame) = owner.next(Duration::from_millis(500)).await else { continue };
        last = rows(&Some(frame)).1;
        if last == json!([{"id":11,"paid":true},{"id":12,"paid":true}]) {
            break;
        }
    }
    assert_eq!(last, json!([{"id":11,"paid":true},{"id":12,"paid":true}]), "the job's transitions reached the subscriber");
    server.shutdown().await.expect("shutdown");
}
