//! Bounded SSE and WebSocket event delivery over the durable event cursor.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{future::Future, io, sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{
    tungstenite::{
        protocol::{frame::coding::CloseCode, CloseFrame, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

const MAX_SUBSCRIPTION_BYTES: usize = 16 * 1024;
const MAX_STREAM_LENGTH: usize = 128;
const MAX_CURSOR_LENGTH: usize = 256;
const MAX_AUTH_TOKEN_LENGTH: usize = 4096;
const MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const WS_AUTH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct RealTimeOptions {
    pub enabled: bool,
    pub max_connections: usize,
    pub max_lifetime: Duration,
    pub heartbeat_interval: Duration,
    pub poll_interval: Duration,
    /// How often a data subscription re-runs its read when no commit in this process woke it. A commit of this
    /// process wakes it at once; this interval is the safety net for writes the process never saw (another instance, a script).
    pub query_poll_interval: Duration,
}

impl Default for RealTimeOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_connections: 32,
            max_lifetime: Duration::from_secs(15 * 60),
            heartbeat_interval: Duration::from_secs(15),
            poll_interval: Duration::from_millis(250),
            query_poll_interval: Duration::from_secs(5),
        }
    }
}

impl RealTimeOptions {
    pub fn validate(&self) -> io::Result<()> {
        if self.max_connections == 0
            || self.max_connections > 1024
            || self.max_lifetime.is_zero()
            || self.max_lifetime > MAX_LIFETIME
            || self.heartbeat_interval.is_zero()
            || self.poll_interval.is_zero()
            || self.poll_interval > self.heartbeat_interval
            || self.query_poll_interval.is_zero()
            || self.query_poll_interval > Duration::from_secs(60 * 60)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid realtime limits"));
        }
        Ok(())
    }
}

/// What a client subscribes to: the events of a stream from a cursor, or the result of a read (`query`).
///
/// A query subscription follows the PoC `subscribe` (D-P24): the whole result is sent first (`snapshot`), then again
/// (`changed`) whenever a re-run as the subscriber gives a different one, so it has no cursor to resume from, and a
/// reconnect starts with a new snapshot. The re-run is the full check of any read: the contract, the principal, the row
/// and field policies. A subscriber who lost access gets an error and the stream ends.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Subscription {
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub query: Option<Value>,
    #[serde(rename = "contractFingerprint")]
    pub contract_fingerprint: String,
}

/// The `after` of a query subscription before anything was sent; it is never a digest of a result.
const QUERY_START: &str = "0";

impl Subscription {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_SUBSCRIPTION_BYTES {
            return Err("WIRE.PAYLOAD_TOO_LARGE");
        }
        let subscription: Self = serde_json::from_slice(bytes).map_err(|_| "WIRE.BAD_REQUEST")?;
        if subscription.contract_fingerprint.len() != 64
            || !subscription.contract_fingerprint.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("WIRE.BAD_REQUEST");
        }
        match (&subscription.stream, &subscription.after, &subscription.query) {
            (Some(stream), Some(after), None) => {
                if stream.is_empty()
                    || stream.len() > MAX_STREAM_LENGTH
                    || stream.chars().any(char::is_control)
                    || after.is_empty()
                    || after.len() > MAX_CURSOR_LENGTH
                    || after.chars().any(char::is_control)
                {
                    return Err("WIRE.BAD_REQUEST");
                }
            }
            // Exactly one of the two kinds, and a query has no cursor.
            (None, None, Some(query)) if query.is_object() => {}
            _ => return Err("WIRE.BAD_REQUEST"),
        }
        Ok(subscription)
    }

    /// Where polling starts: the cursor of an event stream, or "nothing sent yet" for a query.
    fn start(&self) -> String {
        self.after.clone().unwrap_or_else(|| QUERY_START.to_owned())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RealtimeEvent {
    pub cursor: String,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct EventBatch {
    pub events: Vec<RealtimeEvent>,
    pub cursor: String,
}

pub(crate) type PollError = &'static str;
type PollFuture = std::pin::Pin<Box<dyn Future<Output = Result<EventBatch, PollError>> + Send + 'static>>;
type AuthFuture = std::pin::Pin<Box<dyn Future<Output = Result<(), PollError>> + Send + 'static>>;

pub(crate) async fn send_sse_headers(writer: &mut (impl AsyncWriteExt + Unpin), origin: Option<&str>) -> io::Result<()> {
    let origin = origin.map(|origin| format!("access-control-allow-origin: {origin}\r\n")).unwrap_or_default();
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-cache, no-transform\r\nconnection: keep-alive\r\nvary: Origin\r\n{origin}x-accel-buffering: no\r\n\r\n"
    );
    tokio::time::timeout(WRITE_TIMEOUT, async {
        writer.write_all(headers.as_bytes()).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "slow realtime client"))?
}

/// Changes closer together than this are answered by one re-run of a data subscription.
const QUERY_DEBOUNCE: Duration = Duration::from_millis(100);

/// Resolves when a commit of this process signals a change, for a data subscription; never for an event stream.
async fn wake_signal(wake: &mut Option<tokio::sync::watch::Receiver<u64>>) {
    if let Some(receiver) = wake.as_mut() {
        if receiver.changed().await.is_ok() {
            // A burst of commits costs one re-run: wait a moment, then mark everything seen so far as answered.
            tokio::time::sleep(QUERY_DEBOUNCE).await;
            receiver.borrow_and_update();
            return;
        }
    }
    std::future::pending::<()>().await
}

pub(crate) async fn serve_sse<W, F>(
    writer: &mut W,
    subscription: Subscription,
    options: &RealTimeOptions,
    changes: tokio::sync::watch::Receiver<u64>,
    mut poll: F,
) -> io::Result<()>
where
    W: AsyncWriteExt + Unpin,
    F: FnMut(&Subscription, &str) -> PollFuture,
{
    let start = tokio::time::Instant::now();
    let deadline = start + options.max_lifetime;
    let mut cursor = subscription.start();
    let query = subscription.query.is_some();
    let mut wake = query.then_some(changes);
    let mut polling = tokio::time::interval_at(start, if query { options.query_poll_interval } else { options.poll_interval });
    polling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval_at(start + options.heartbeat_interval, options.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let due = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                write_sse(writer, ": close: lifetime limit\n\n").await?;
                return Ok(());
            }
            _ = polling.tick() => true,
            _ = wake_signal(&mut wake) => true,
            _ = heartbeat.tick() => {
                write_sse(writer, ": keepalive\n\n").await?;
                false
            }
        };
        if !due {
            continue;
        }
        match tokio::time::timeout_at(deadline, poll(&subscription, &cursor)).await {
            Ok(Ok(batch)) => {
                for event in batch.events {
                    let frame = sse_event(&event);
                    write_sse(writer, &frame).await?;
                    cursor = event.cursor;
                }
                if batch.cursor != cursor {
                    cursor = batch.cursor;
                }
            }
            Ok(Err(code)) => {
                let frame = format!("event: error\ndata: {}\n\n", json!({"code":code}));
                let _ = write_sse(writer, &frame).await;
                return Ok(());
            }
            Err(_) => {
                write_sse(writer, ": close: lifetime limit\n\n").await?;
                return Ok(());
            }
        }
    }
}

pub(crate) async fn stream_sse<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    subscription: Subscription,
    token: String,
    expected_fingerprint: String,
    facts: Arc<Value>,
    runtime: Arc<crate::server::Runtime>,
    wire: spike_v2_read::id_wire::IdWire,
) -> io::Result<()> {
    let options = runtime.infrastructure.realtime.clone();
    let changes = runtime.infrastructure.data_changes.subscribe();
    let mut poll = |subscription: &Subscription, after: &str| -> PollFuture {
        let (subscription, after, token, fingerprint, facts, runtime) =
            (subscription.clone(), after.to_owned(), token.clone(), expected_fingerprint.clone(), facts.clone(), runtime.clone());
        Box::pin(async move { poll_events(runtime, facts, wire, token, fingerprint, subscription, after).await })
    };
    serve_sse(writer, subscription, &options, changes, &mut poll).await
}

fn sse_event(event: &RealtimeEvent) -> String {
    let mut frame = format!("id: {}\nevent: message\n", event.cursor);
    let data = event.payload.to_string();
    for line in data.lines() {
        frame.push_str("data: ");
        frame.push_str(line);
        frame.push('\n');
    }
    frame.push('\n');
    frame
}

fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty() && cursor.len() <= 19 && cursor.bytes().all(|byte| byte.is_ascii_digit()) && cursor.parse::<u64>().is_ok()
}

async fn write_sse(writer: &mut (impl AsyncWriteExt + Unpin), frame: &str) -> io::Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, async {
        writer.write_all(frame.as_bytes()).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "slow realtime client"))?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserAuthFrame {
    #[serde(rename = "type")]
    kind: String,
    token: String,
}

pub(crate) async fn serve_ws<S, A, F>(
    io: S,
    options: &RealTimeOptions,
    changes: tokio::sync::watch::Receiver<u64>,
    header_token: Option<String>,
    mut authenticate: A,
    mut poll: F,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    A: FnMut(&str) -> AuthFuture,
    F: FnMut(&Subscription, &str, &str) -> PollFuture,
{
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_SUBSCRIPTION_BYTES);
    config.max_frame_size = Some(MAX_SUBSCRIPTION_BYTES);
    config.max_write_buffer_size = 64 * 1024;
    config.write_buffer_size = 4 * 1024;
    let mut socket = WebSocketStream::from_raw_socket(io, tokio_tungstenite::tungstenite::protocol::Role::Server, Some(config)).await;
    let start = tokio::time::Instant::now();
    let deadline = start + options.max_lifetime;
    let browser_deadline = (start + WS_AUTH_TIMEOUT).min(deadline);
    let browser_mode = header_token.is_none();
    let token = if let Some(token) = header_token {
        token
    } else {
        let Some(frame) = read_ws_text(&mut socket, browser_deadline).await? else {
            close_ws(&mut socket, "authentication required").await?;
            return Ok(());
        };
        let auth: BrowserAuthFrame = match serde_json::from_str::<BrowserAuthFrame>(&frame) {
            Ok(auth) if auth.kind == "auth" && !auth.token.is_empty() && auth.token.len() <= MAX_AUTH_TOKEN_LENGTH => auth,
            _ => {
                send_ws_error(&mut socket, "WIRE.UNAUTHENTICATED").await?;
                return Ok(());
            }
        };
        match tokio::time::timeout_at(browser_deadline, authenticate(&auth.token)).await {
            Ok(Ok(())) => {}
            Ok(Err(code)) => {
                send_ws_error(&mut socket, code).await?;
                return Ok(());
            }
            Err(_) => {
                send_ws_error(&mut socket, "WIRE.DB_UNAVAILABLE").await?;
                return Ok(());
            }
        }
        send_ws(&mut socket, Message::Text(json!({"type":"authenticated"}).to_string().into())).await?;
        auth.token
    };

    let subscription_text = match read_ws_text(&mut socket, if browser_mode { browser_deadline } else { deadline }).await? {
        Some(text) => text,
        None => {
            close_ws(&mut socket, "subscription required").await?;
            return Ok(());
        }
    };
    let subscription = if browser_mode { parse_browser_subscription(&subscription_text) } else { Subscription::parse(subscription_text.as_bytes()) };
    let subscription = match subscription {
        Ok(subscription) => subscription,
        Err(code) => {
            send_ws_error(&mut socket, code).await?;
            return Ok(());
        }
    };
    run_ws(&mut socket, subscription, &token, options, changes, deadline, &mut poll).await
}

/// 토큰 없는 브라우저 WebSocket의 슬롯 상태. 인증 전에는 `realtime_preauth`만 쓰고, 인증이 끝나면 `realtime_slots`로 옮긴다.
/// 연결이 어떻게 끝나든(취소 포함) Drop이 쥔 슬롯을 돌려준다.
pub(crate) struct BrowserSlots {
    runtime: Arc<crate::server::Runtime>,
    preauth: std::sync::atomic::AtomicBool,
    main: std::sync::atomic::AtomicBool,
    /// The principal's share of the realtime slots, taken together with the main slot once the auth frame names a principal.
    principal: std::sync::Mutex<Option<crate::slots::PrincipalPermit>>,
}

impl BrowserSlots {
    pub(crate) fn acquire_preauth(runtime: &Arc<crate::server::Runtime>) -> Option<Arc<Self>> {
        runtime.realtime_preauth.try_acquire().ok()?.forget();
        Some(Arc::new(Self { runtime: runtime.clone(), preauth: true.into(), main: false.into(), principal: Default::default() }))
    }

    fn promote(&self, principal: &str) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        // The principal's share first, so a refused principal does not briefly hold a server-wide slot.
        let Some(share) = self.runtime.principal_slots.realtime.try_acquire(principal) else { return false };
        let Ok(permit) = self.runtime.realtime_slots.try_acquire() else { return false };
        permit.forget();
        *self.principal.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(share);
        self.main.store(true, SeqCst);
        if self.preauth.swap(false, SeqCst) {
            self.runtime.realtime_preauth.add_permits(1);
        }
        true
    }
}

impl Drop for BrowserSlots {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::SeqCst;
        if self.preauth.swap(false, SeqCst) {
            self.runtime.realtime_preauth.add_permits(1);
        }
        if self.main.swap(false, SeqCst) {
            self.runtime.realtime_slots.add_permits(1);
        }
        self.principal.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    }
}

/// The check a browser WebSocket's token goes through before it may stream: a live product principal, the per-principal
/// request limit, then the connection slots. Answers how many milliseconds the token still lives.
pub(crate) async fn admit_browser_token(runtime: &Arc<crate::server::Runtime>, slots: Option<&BrowserSlots>, token: &str) -> Result<u64, PollError> {
    let provider = runtime.authenticator.as_ref().ok_or("WIRE.UNAUTHENTICATED")?;
    match tokio::time::timeout(crate::server::DB_PREFLIGHT_TIMEOUT, provider.verify_session(token)).await {
        Ok(Ok(session)) if session.remaining_ms() > 0 && session.durable.is_some() => {
            if let Some(admission) = runtime.admission.as_ref() {
                let Some(pool) = runtime.db_pool.as_ref() else { return Err("WIRE.DB_UNAVAILABLE") };
                let principal = crate::admission::principal(Some(&session));
                match admission
                    .check(pool, &principal, crate::admission::Route::Realtime, tokio::time::Instant::now() + crate::server::DB_PREFLIGHT_TIMEOUT)
                    .await
                {
                    Ok(crate::admission::Outcome::Allowed) => {}
                    Ok(crate::admission::Outcome::Limited { .. }) => return Err("WIRE.RATE_LIMITED"),
                    Err(_) => return Err("WIRE.DB_UNAVAILABLE"),
                }
            }
            if let Some(slots) = slots {
                if !slots.promote(&crate::admission::principal(Some(&session))) {
                    return Err("WIRE.SERVER_BUSY");
                }
            }
            Ok(session.remaining_ms())
        }
        Ok(Ok(_)) | Ok(Err(crate::auth::AuthError::Expired)) => Err("WIRE.TOKEN_EXPIRED"),
        Ok(Err(crate::auth::AuthError::Unavailable)) | Err(_) => Err("WIRE.DB_UNAVAILABLE"),
        Ok(Err(_)) => Err("WIRE.UNAUTHENTICATED"),
    }
}

pub(crate) async fn stream_ws<S>(
    io: S,
    token: Option<String>,
    slots: Option<Arc<BrowserSlots>>,
    expected_fingerprint: String,
    facts: Arc<Value>,
    runtime: Arc<crate::server::Runtime>,
    wire: spike_v2_read::id_wire::IdWire,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let options = runtime.infrastructure.realtime.clone();
    let auth_runtime = runtime.clone();
    let mut authenticate = move |token: &str| -> AuthFuture {
        let (token, runtime, slots) = (token.to_owned(), auth_runtime.clone(), slots.clone());
        Box::pin(async move { admit_browser_token(&runtime, slots.as_deref(), &token).await.map(|_| ()) })
    };
    let mut poll = |subscription: &Subscription, after: &str, token: &str| -> PollFuture {
        let (subscription, after, token, fingerprint, facts, runtime) =
            (subscription.clone(), after.to_owned(), token.to_owned(), expected_fingerprint.clone(), facts.clone(), runtime.clone());
        Box::pin(async move { poll_events(runtime, facts, wire, token, fingerprint, subscription, after).await })
    };
    let changes = runtime.infrastructure.data_changes.subscribe();
    serve_ws(io, &options, changes, token, &mut authenticate, &mut poll).await
}

async fn read_ws_text<S>(
    socket: &mut WebSocketStream<S>,
    deadline: tokio::time::Instant,
) -> Result<Option<String>, tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) if text.len() <= MAX_SUBSCRIPTION_BYTES => return Ok(Some(text.to_string())),
            Ok(Some(Ok(Message::Close(frame)))) => {
                socket.send(Message::Close(frame)).await?;
                return Ok(None);
            }
            Ok(Some(Ok(Message::Ping(payload)))) => send_ws(socket, Message::Pong(payload)).await?,
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => return Ok(None),
            Ok(Some(Err(error))) => return Err(error),
        }
    }
}

fn parse_browser_subscription(text: &str) -> Result<Subscription, &'static str> {
    if text.len() > MAX_SUBSCRIPTION_BYTES {
        return Err("WIRE.PAYLOAD_TOO_LARGE");
    }
    let mut value: Value = serde_json::from_str(text).map_err(|_| "WIRE.BAD_REQUEST")?;
    if value["type"] != "subscribe" {
        return Err("WIRE.BAD_REQUEST");
    }
    value.as_object_mut().ok_or("WIRE.BAD_REQUEST")?.remove("type");
    Subscription::parse(value.to_string().as_bytes())
}

async fn close_ws<S>(socket: &mut WebSocketStream<S>, reason: &'static str) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: reason.into() }))).await
}

async fn send_ws_error<S>(socket: &mut WebSocketStream<S>, code: &'static str) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let text = json!({"type":"error","code":code}).to_string();
    let _ = send_ws(socket, Message::Text(text.into())).await;
    close_ws(socket, code).await
}

async fn poll_events(
    runtime: Arc<crate::server::Runtime>,
    facts: Arc<Value>,
    wire: spike_v2_read::id_wire::IdWire,
    token: String,
    expected_fingerprint: String,
    subscription: Subscription,
    after: String,
) -> Result<EventBatch, PollError> {
    if subscription.contract_fingerprint != expected_fingerprint {
        return Err("WIRE.CONTRACT_MISMATCH");
    }
    let provider = runtime.authenticator.as_ref().ok_or("WIRE.UNAUTHENTICATED")?;
    let session = match tokio::time::timeout(crate::server::DB_PREFLIGHT_TIMEOUT, provider.verify_session(&token)).await {
        Ok(Ok(session)) if session.remaining_ms() > 0 => session,
        Ok(Ok(_)) | Ok(Err(crate::auth::AuthError::Expired)) => return Err("WIRE.TOKEN_EXPIRED"),
        Ok(Err(crate::auth::AuthError::Unavailable)) | Err(_) => return Err("WIRE.DB_UNAVAILABLE"),
        Ok(Err(_)) => return Err("WIRE.UNAUTHENTICATED"),
    };
    if let Some(query) = &subscription.query {
        return poll_query(&runtime, &facts, wire, &session, query, &after).await;
    }
    let stream = subscription.stream.as_deref().ok_or("WIRE.BAD_REQUEST")?;
    let response = crate::durable::read_events(&runtime, &facts, wire, &session, stream, &after, 100).await;
    if response["ok"] != true {
        return Err(match response["code"].as_str() {
            Some("WIRE.CURSOR_EXPIRED") => "WIRE.CURSOR_EXPIRED",
            Some("WIRE.BAD_VALUE") => "WIRE.BAD_VALUE",
            Some("WIRE.TOKEN_EXPIRED") => "WIRE.TOKEN_EXPIRED",
            Some("WIRE.DEPLOYMENT_CHANGED") => "WIRE.DEPLOYMENT_CHANGED",
            Some("WIRE.UNAUTHENTICATED") => "WIRE.UNAUTHENTICATED",
            Some("WIRE.NOT_EXPOSED") => "WIRE.NOT_EXPOSED",
            Some("WIRE.NOT_AVAILABLE") => "WIRE.NOT_AVAILABLE",
            Some("WIRE.CONTRACT_MISMATCH") => "WIRE.CONTRACT_MISMATCH",
            _ => "WIRE.DB_UNAVAILABLE",
        });
    }
    let Some(cursor) = response["cursor"].as_str().filter(|cursor| valid_cursor(cursor)) else { return Err("WIRE.DB_UNAVAILABLE") };
    let events = response["events"]
        .as_array()
        .ok_or("WIRE.DB_UNAVAILABLE")?
        .iter()
        .map(|event| {
            let cursor = event["cursor"].as_str()?;
            valid_cursor(cursor).then(|| RealtimeEvent { cursor: cursor.to_owned(), payload: event["payload"].clone() })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("WIRE.DB_UNAVAILABLE")?;
    Ok(EventBatch { events, cursor: cursor.to_owned() })
}

/// One run of a data subscription: the result as the subscriber sees it now. It is sent when it differs from the last one
/// sent (`after` is the digest of that one, or `QUERY_START`), as the whole result, so a client never applies a diff.
async fn poll_query(
    runtime: &Arc<crate::server::Runtime>,
    facts: &Value,
    wire: spike_v2_read::id_wire::IdWire,
    session: &crate::auth::Session,
    query: &Value,
    after: &str,
) -> Result<EventBatch, PollError> {
    let rows = crate::durable::read_as_subscriber(runtime, facts, wire, session, query).await?;
    let digest = spike_v1_fixture::digest(&rows);
    if digest == after {
        return Ok(EventBatch { events: Vec::new(), cursor: after.to_owned() });
    }
    let kind = if after == QUERY_START { "snapshot" } else { "changed" };
    Ok(EventBatch { events: vec![RealtimeEvent { cursor: digest.clone(), payload: json!({"type":kind,"rows":rows}) }], cursor: digest })
}

async fn run_ws<S, F>(
    socket: &mut WebSocketStream<S>,
    subscription: Subscription,
    token: &str,
    options: &RealTimeOptions,
    changes: tokio::sync::watch::Receiver<u64>,
    deadline: tokio::time::Instant,
    poll: &mut F,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnMut(&Subscription, &str, &str) -> PollFuture,
{
    let start = tokio::time::Instant::now();
    let mut cursor = subscription.start();
    let query = subscription.query.is_some();
    let mut wake = query.then_some(changes);
    let mut polling = tokio::time::interval_at(start, if query { options.query_poll_interval } else { options.poll_interval });
    polling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval_at(start + options.heartbeat_interval, options.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_pong = start;
    loop {
        let due = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "lifetime limit".into() }))).await?;
                return Ok(());
            }
            incoming = socket.next() => match incoming {
                None => return Ok(()),
                Some(Err(error)) => return Err(error),
                Some(Ok(Message::Close(frame))) => { send_ws(socket, Message::Close(frame)).await?; return Ok(()); }
                Some(Ok(Message::Ping(payload))) => { send_ws(socket, Message::Pong(payload)).await?; false }
                Some(Ok(Message::Pong(_))) => { last_pong = tokio::time::Instant::now(); false }
                Some(Ok(_)) => {
                    send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: "one subscription per connection".into() }))).await?;
                    return Ok(());
                }
            },
            _ = polling.tick() => true,
            _ = wake_signal(&mut wake) => true,
            _ = heartbeat.tick() => {
                if tokio::time::Instant::now().duration_since(last_pong) > options.heartbeat_interval.saturating_mul(2) {
                    send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "heartbeat timeout".into() }))).await?;
                    return Ok(());
                }
                send_ws(socket, Message::Ping(Vec::new().into())).await?;
                false
            }
        };
        if !due {
            continue;
        }
        match tokio::time::timeout_at(deadline, poll(&subscription, &cursor, token)).await {
            Ok(Ok(batch)) => {
                for event in batch.events {
                    let text = json!({"type":"event", "cursor":event.cursor, "payload":event.payload}).to_string();
                    send_ws(socket, Message::Text(text.into())).await?;
                    cursor = event.cursor;
                }
                if batch.cursor != cursor {
                    cursor = batch.cursor;
                }
            }
            Ok(Err(code)) => {
                let text = json!({"type":"error", "code":code}).to_string();
                let _ = send_ws(socket, Message::Text(text.into())).await;
                send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: code.into() }))).await?;
                return Ok(());
            }
            Err(_) => {
                send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "lifetime limit".into() }))).await?;
                return Ok(());
            }
        }
    }
}

async fn send_ws<S>(socket: &mut WebSocketStream<S>, message: Message) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(WRITE_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| tokio_tungstenite::tungstenite::Error::Io(io::Error::new(io::ErrorKind::TimedOut, "slow realtime client")))?
}
