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
const MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct RealTimeOptions {
    pub enabled: bool,
    pub max_connections: usize,
    pub max_lifetime: Duration,
    pub heartbeat_interval: Duration,
    pub poll_interval: Duration,
}

impl Default for RealTimeOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_connections: 32,
            max_lifetime: Duration::from_secs(15 * 60),
            heartbeat_interval: Duration::from_secs(15),
            poll_interval: Duration::from_millis(250),
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
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid realtime limits"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Subscription {
    pub stream: String,
    pub after: String,
    #[serde(rename = "contractFingerprint")]
    pub contract_fingerprint: String,
}

impl Subscription {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_SUBSCRIPTION_BYTES {
            return Err("PAYLOAD_TOO_LARGE");
        }
        let subscription: Self = serde_json::from_slice(bytes).map_err(|_| "BAD_REQUEST")?;
        if subscription.stream.is_empty()
            || subscription.stream.len() > MAX_STREAM_LENGTH
            || subscription.stream.chars().any(char::is_control)
            || subscription.after.is_empty()
            || subscription.after.len() > MAX_CURSOR_LENGTH
            || subscription.after.chars().any(char::is_control)
            || subscription.contract_fingerprint.len() != 64
            || !subscription.contract_fingerprint.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("BAD_REQUEST");
        }
        Ok(subscription)
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

pub(crate) async fn serve_sse<W, F>(writer: &mut W, subscription: Subscription, options: &RealTimeOptions, mut poll: F) -> io::Result<()>
where
    W: AsyncWriteExt + Unpin,
    F: FnMut(&Subscription, &str) -> PollFuture,
{
    let start = tokio::time::Instant::now();
    let deadline = start + options.max_lifetime;
    let mut cursor = subscription.after.clone();
    let mut polling = tokio::time::interval_at(start, options.poll_interval);
    polling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval_at(start + options.heartbeat_interval, options.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                write_sse(writer, ": close: lifetime limit\n\n").await?;
                return Ok(());
            }
            _ = polling.tick() => {
                match tokio::time::timeout_at(deadline, poll(&subscription, &cursor)).await {
                    Ok(Ok(batch)) => {
                        for event in batch.events {
                            let frame = sse_event(&event);
                            write_sse(writer, &frame).await?;
                            cursor = event.cursor;
                        }
                        if batch.cursor != cursor { cursor = batch.cursor; }
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
            _ = heartbeat.tick() => write_sse(writer, ": keepalive\n\n").await?,
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
    let mut poll = |subscription: &Subscription, after: &str| -> PollFuture {
        let (subscription, after, token, fingerprint, facts, runtime) =
            (subscription.clone(), after.to_owned(), token.clone(), expected_fingerprint.clone(), facts.clone(), runtime.clone());
        Box::pin(async move { poll_events(runtime, facts, wire, token, fingerprint, subscription, after).await })
    };
    serve_sse(writer, subscription, &options, &mut poll).await
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

pub(crate) async fn serve_ws<S, F>(io: S, options: &RealTimeOptions, mut poll: F) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnMut(&Subscription, &str) -> PollFuture,
{
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_SUBSCRIPTION_BYTES);
    config.max_frame_size = Some(MAX_SUBSCRIPTION_BYTES);
    config.max_write_buffer_size = 64 * 1024;
    config.write_buffer_size = 4 * 1024;
    let mut socket = WebSocketStream::from_raw_socket(io, tokio_tungstenite::tungstenite::protocol::Role::Server, Some(config)).await;
    let deadline = tokio::time::Instant::now() + options.max_lifetime;
    let subscription = match tokio::time::timeout_at(deadline, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match Subscription::parse(text.as_bytes()) {
            Ok(subscription) => subscription,
            Err(code) => {
                let _ = socket.send(Message::Text(json!({"type":"error","code":code}).to_string().into())).await;
                socket.send(Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: code.into() }))).await?;
                return Ok(());
            }
        },
        Ok(Some(Ok(Message::Close(frame)))) => {
            socket.send(Message::Close(frame)).await?;
            return Ok(());
        }
        Ok(Some(Err(error))) => return Err(error),
        _ => {
            socket.send(Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: "subscription required".into() }))).await?;
            return Ok(());
        }
    };
    run_ws(&mut socket, subscription, options, &mut poll).await
}

pub(crate) async fn stream_ws<S>(
    io: S,
    token: String,
    expected_fingerprint: String,
    facts: Arc<Value>,
    runtime: Arc<crate::server::Runtime>,
    wire: spike_v2_read::id_wire::IdWire,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let options = runtime.infrastructure.realtime.clone();
    let mut poll = |subscription: &Subscription, after: &str| -> PollFuture {
        let (subscription, after, token, fingerprint, facts, runtime) =
            (subscription.clone(), after.to_owned(), token.clone(), expected_fingerprint.clone(), facts.clone(), runtime.clone());
        Box::pin(async move { poll_events(runtime, facts, wire, token, fingerprint, subscription, after).await })
    };
    serve_ws(io, &options, &mut poll).await
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
        return Err("CONTRACT_MISMATCH");
    }
    let provider = runtime.authenticator.as_ref().ok_or("UNAUTHENTICATED")?;
    let session = match tokio::time::timeout(crate::server::DB_PREFLIGHT_TIMEOUT, provider.verify_session(&token)).await {
        Ok(Ok(session)) if session.remaining_ms() > 0 => session,
        Ok(Ok(_)) | Ok(Err(crate::auth::AuthError::Expired)) => return Err("TOKEN_EXPIRED"),
        Ok(Err(crate::auth::AuthError::Unavailable)) | Err(_) => return Err("DB_UNAVAILABLE"),
        Ok(Err(_)) => return Err("UNAUTHENTICATED"),
    };
    let response = crate::durable::read_events(&runtime, &facts, wire, &session, &subscription.stream, &after, 100).await;
    if response["ok"] != true {
        return Err(match response["code"].as_str() {
            Some("CURSOR_EXPIRED") => "CURSOR_EXPIRED",
            Some("BAD_VALUE") => "BAD_VALUE",
            Some("TOKEN_EXPIRED") => "TOKEN_EXPIRED",
            Some("DEPLOYMENT_CHANGED") => "DEPLOYMENT_CHANGED",
            Some("UNAUTHENTICATED") => "UNAUTHENTICATED",
            Some("NOT_EXPOSED") => "NOT_EXPOSED",
            Some("NOT_AVAILABLE") => "NOT_AVAILABLE",
            Some("CONTRACT_MISMATCH") => "CONTRACT_MISMATCH",
            _ => "DB_UNAVAILABLE",
        });
    }
    let Some(cursor) = response["cursor"].as_str().filter(|cursor| valid_cursor(cursor)) else { return Err("DB_UNAVAILABLE") };
    let events = response["events"]
        .as_array()
        .ok_or("DB_UNAVAILABLE")?
        .iter()
        .map(|event| {
            let cursor = event["cursor"].as_str()?;
            valid_cursor(cursor).then(|| RealtimeEvent { cursor: cursor.to_owned(), payload: event["payload"].clone() })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("DB_UNAVAILABLE")?;
    Ok(EventBatch { events, cursor: cursor.to_owned() })
}

async fn run_ws<S, F>(
    socket: &mut WebSocketStream<S>,
    subscription: Subscription,
    options: &RealTimeOptions,
    poll: &mut F,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnMut(&Subscription, &str) -> PollFuture,
{
    let start = tokio::time::Instant::now();
    let deadline = start + options.max_lifetime;
    let mut cursor = subscription.after.clone();
    let mut polling = tokio::time::interval_at(start, options.poll_interval);
    polling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval_at(start + options.heartbeat_interval, options.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_pong = start;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "lifetime limit".into() }))).await?;
                return Ok(());
            }
            incoming = socket.next() => match incoming {
                None => return Ok(()),
                Some(Err(error)) => return Err(error),
                Some(Ok(Message::Close(frame))) => { send_ws(socket, Message::Close(frame)).await?; return Ok(()); }
                Some(Ok(Message::Ping(payload))) => send_ws(socket, Message::Pong(payload)).await?,
                Some(Ok(Message::Pong(_))) => last_pong = tokio::time::Instant::now(),
                Some(Ok(_)) => {
                    send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Policy, reason: "one subscription per connection".into() }))).await?;
                    return Ok(());
                }
            },
            _ = polling.tick() => {
                match tokio::time::timeout_at(deadline, poll(&subscription, &cursor)).await {
                    Ok(Ok(batch)) => {
                        for event in batch.events {
                            let text = json!({"type":"event", "cursor":event.cursor, "payload":event.payload}).to_string();
                            send_ws(socket, Message::Text(text.into())).await?;
                            cursor = event.cursor;
                        }
                        if batch.cursor != cursor { cursor = batch.cursor; }
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
            _ = heartbeat.tick() => {
                if tokio::time::Instant::now().duration_since(last_pong) > options.heartbeat_interval.saturating_mul(2) {
                    send_ws(socket, Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "heartbeat timeout".into() }))).await?;
                    return Ok(());
                }
                send_ws(socket, Message::Ping(Vec::new().into())).await?;
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
