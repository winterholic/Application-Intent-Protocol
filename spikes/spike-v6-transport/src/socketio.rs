//! Socket.IO-compatible transport for the FaceReview `watch_frame` event (WBS 7.2.6, D-P-WATCH-TRANSPORT).
//!
//! Only what the original client uses is spoken: Engine.IO v4 over a WebSocket (no HTTP long polling, no upgrade),
//! the main namespace `/`, the CONNECT packet with an `auth` object, plain (non-binary) EVENT packets with an
//! acknowledgement, and DISCONNECT. See <https://socket.io/docs/v4/engine-io-protocol/> and
//! <https://socket.io/docs/v4/socket-io-protocol/>.
//!
//! This slice is the transport boundary: handshake, authentication, payload bounds and disconnect meaning. A frame that
//! passes validation is not analysed or stored yet; the acknowledgement says so (`WIRE.NOT_AVAILABLE`).

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio_tungstenite::{
    tungstenite::{
        protocol::{frame::coding::CloseCode, CloseFrame, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

/// Largest `auth.token`, the same bound the first-frame authentication of `/events/socket` uses.
pub(crate) const MAX_AUTH_TOKEN_LENGTH: usize = 4096;

#[derive(Clone, Debug)]
pub struct SocketIoOptions {
    /// Largest Engine.IO packet (one WebSocket message). A webcam frame is a base64 image, so this is well above the
    /// 16 KiB of an event subscription; it is also advertised to the client as `maxPayload`.
    pub max_packet_bytes: usize,
    pub ping_interval: Duration,
    pub ping_timeout: Duration,
    /// How long a connection may stay without a completed CONNECT (including the token check).
    pub connect_timeout: Duration,
    pub max_lifetime: Duration,
}

impl Default for SocketIoOptions {
    fn default() -> Self {
        Self {
            max_packet_bytes: 512 * 1024,
            ping_interval: Duration::from_secs(25),
            ping_timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(5),
            max_lifetime: Duration::from_secs(6 * 60 * 60),
        }
    }
}

impl SocketIoOptions {
    pub fn validate(&self) -> std::io::Result<()> {
        let second = Duration::from_secs(1);
        if !(1024..=8 * 1024 * 1024).contains(&self.max_packet_bytes)
            || !(second..=Duration::from_secs(10 * 60)).contains(&self.ping_interval)
            || !(second..=Duration::from_secs(10 * 60)).contains(&self.ping_timeout)
            || !(second..=Duration::from_secs(60)).contains(&self.connect_timeout)
            || !(second..=Duration::from_secs(24 * 60 * 60)).contains(&self.max_lifetime)
        {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid socketio limits"));
        }
        Ok(())
    }
}

/// True for the request target of an Engine.IO v4 WebSocket-only connection: `/socket.io/?EIO=4&transport=websocket`.
/// Long polling (`transport=polling`), a session id and any other parameter are not served.
pub(crate) fn is_websocket_target(target: &str) -> bool {
    let Some(query) = target.strip_prefix("/socket.io/?") else { return false };
    let (mut version, mut transport, mut stamp) = (false, false, false);
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("EIO", "4")) if !version => version = true,
            Some(("transport", "websocket")) if !transport => transport = true,
            Some(("t", value))
                if !stamp && !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
            {
                stamp = true
            }
            _ => return false,
        }
    }
    version && transport
}

/// True when the request target addresses the Socket.IO endpoint at all, whatever its query.
pub(crate) fn is_endpoint(target: &str) -> bool {
    target == "/socket.io/" || target.starts_with("/socket.io/?")
}

pub(crate) type AdmitFuture = std::pin::Pin<Box<dyn Future<Output = Result<Duration, &'static str>> + Send + 'static>>;

#[derive(Debug, PartialEq)]
pub(crate) enum Packet {
    Connect { namespace: String, auth: Option<Value> },
    Disconnect,
    Event { namespace: String, ack: Option<u64>, args: Vec<Value> },
}

/// Splits `[/namespace,]rest`. A namespace without a payload has no trailing comma (`0/admin`).
fn split_namespace(text: &str) -> (String, &str) {
    if !text.starts_with('/') {
        return ("/".to_owned(), text);
    }
    match text.split_once(',') {
        Some((namespace, rest)) => (namespace.to_owned(), rest),
        None => (text.to_owned(), ""),
    }
}

/// Decodes the Socket.IO packet inside an Engine.IO `message`. Binary packet types and the ones only a server sends are errors.
pub(crate) fn parse_packet(text: &str) -> Result<Packet, &'static str> {
    let Some(kind) = text.bytes().next() else { return Err("empty packet") };
    let rest = &text[1..];
    match kind {
        b'0' => {
            let (namespace, payload) = split_namespace(rest);
            let auth = if payload.is_empty() {
                None
            } else {
                let value: Value = serde_json::from_str(payload).map_err(|_| "CONNECT payload is not JSON")?;
                if !value.is_object() {
                    return Err("CONNECT auth is not an object");
                }
                Some(value)
            };
            Ok(Packet::Connect { namespace, auth })
        }
        b'1' if rest.is_empty() => Ok(Packet::Disconnect),
        b'2' => {
            let (namespace, rest) = split_namespace(rest);
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let ack = if digits == 0 { None } else { Some(rest[..digits].parse::<u64>().map_err(|_| "acknowledgement id out of range")?) };
            let payload = &rest[digits..];
            if !(payload.starts_with('[') && payload.ends_with(']')) {
                return Err("EVENT payload is not an array");
            }
            let Ok(Value::Array(args)) = serde_json::from_str::<Value>(payload) else { return Err("EVENT payload is not an array") };
            if !args.first().is_some_and(Value::is_string) {
                return Err("EVENT has no event name");
            }
            Ok(Packet::Event { namespace, ack, args })
        }
        _ => Err("packet type not accepted from a client"),
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct WatchFrame {
    pub video_view_log_id: String,
    pub video_id: String,
    pub youtube_running_time: f64,
    pub duration: f64,
    pub frame_data: String,
}

const MAX_ID_LENGTH: usize = 128;
/// About eleven days: far beyond any video, small enough that the number still means something.
const MAX_SECONDS: f64 = 1_000_000.0;

fn bounded_id(value: &Value) -> bool {
    value.as_str().is_some_and(|text| text.len() <= MAX_ID_LENGTH && !text.chars().any(char::is_control))
}

fn seconds(value: &Value) -> Option<f64> {
    value.as_f64().filter(|seconds| seconds.is_finite() && (0.0..=MAX_SECONDS).contains(seconds))
}

/// The same required fields as the original `watch_frame`. `user_id` is not one of them: the server takes the user from
/// the authenticated principal, never from the payload. Messages follow the original's wording where it has one.
pub(crate) fn parse_watch_frame(payload: Option<&Value>) -> Result<WatchFrame, String> {
    const MISSING: &str = "Missing required fields";
    let object = payload.and_then(Value::as_object).ok_or_else(|| MISSING.to_owned())?;
    let present = |name: &str| object.get(name).is_some_and(|value| !value.is_null() && value.as_str() != Some(""));
    if !["video_view_log_id", "video_id", "youtube_running_time", "frame_data"].iter().all(|name| present(name)) {
        return Err(MISSING.to_owned());
    }
    let invalid = |name: &str| format!("Invalid field: {name}");
    if !bounded_id(&object["video_view_log_id"]) {
        return Err(invalid("video_view_log_id"));
    }
    if !bounded_id(&object["video_id"]) {
        return Err(invalid("video_id"));
    }
    let youtube_running_time = seconds(&object["youtube_running_time"]).ok_or_else(|| invalid("youtube_running_time"))?;
    let duration = match object.get("duration") {
        None | Some(Value::Null) => 0.0,
        Some(value) => seconds(value).ok_or_else(|| invalid("duration"))?,
    };
    let frame_data = object["frame_data"].as_str().ok_or_else(|| invalid("frame_data"))?.to_owned();
    Ok(WatchFrame {
        video_view_log_id: object["video_view_log_id"].as_str().unwrap_or_default().to_owned(),
        video_id: object["video_id"].as_str().unwrap_or_default().to_owned(),
        youtube_running_time,
        duration,
        frame_data,
    })
}

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

fn session_id() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 15];
    getrandom::getrandom(&mut bytes).expect("난수");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

type WsError = tokio_tungstenite::tungstenite::Error;

async fn send_text<S>(socket: &mut WebSocketStream<S>, text: String) -> Result<(), WsError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Text(text.into())))
        .await
        .map_err(|_| WsError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "slow Socket.IO client")))?
}

/// Ends the transport. The result of the close frame is ignored: the peer may already be gone.
async fn close<S>(socket: &mut WebSocketStream<S>, code: CloseCode, reason: &str)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = CloseFrame { code, reason: reason.to_owned().into() };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Close(Some(frame)))).await;
}

fn connect_error(code: &str) -> String {
    format!("44{}", json!({"message": code, "data": {"code": code}}))
}

async fn pending_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Runs one Socket.IO connection on an already upgraded WebSocket until either side ends it.
///
/// `authenticate` checks the `auth.token` of the CONNECT packet and answers how long the token still lives. It runs once:
/// the connection ends when that time is over, and a disconnect does nothing but close the transport.
pub(crate) async fn serve<S, A>(io: S, options: &SocketIoOptions, mut authenticate: A) -> Result<(), WsError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    A: FnMut(&str) -> AdmitFuture,
{
    use tokio::time::Instant;
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(options.max_packet_bytes);
    config.max_frame_size = Some(options.max_packet_bytes);
    config.max_write_buffer_size = options.max_packet_bytes.max(64 * 1024);
    config.write_buffer_size = 4 * 1024;
    let mut socket = WebSocketStream::from_raw_socket(io, tokio_tungstenite::tungstenite::protocol::Role::Server, Some(config)).await;
    let started = Instant::now();
    let open = json!({
        "sid": session_id(),
        "upgrades": [],
        "pingInterval": options.ping_interval.as_millis() as u64,
        "pingTimeout": options.ping_timeout.as_millis() as u64,
        "maxPayload": options.max_packet_bytes,
    });
    send_text(&mut socket, format!("0{open}")).await?;

    let mut connect_by = Some(started + options.connect_timeout);
    let life_by = started + options.max_lifetime;
    let mut token_by: Option<Instant> = None;
    let mut ping_at = Some(started + options.ping_interval);
    let mut pong_by: Option<Instant> = None;
    loop {
        let message = tokio::select! {
            message = socket.next() => message,
            _ = pending_until(connect_by) => {
                close(&mut socket, CloseCode::Policy, "CONNECT required").await;
                return Ok(());
            }
            _ = tokio::time::sleep_until(life_by) => {
                close(&mut socket, CloseCode::Policy, "connection lifetime").await;
                return Ok(());
            }
            _ = pending_until(token_by) => {
                close(&mut socket, CloseCode::Policy, "WIRE.TOKEN_EXPIRED").await;
                return Ok(());
            }
            _ = pending_until(pong_by) => {
                close(&mut socket, CloseCode::Policy, "ping timeout").await;
                return Ok(());
            }
            _ = pending_until(ping_at) => {
                send_text(&mut socket, "2".to_owned()).await?;
                ping_at = None;
                pong_by = Some(Instant::now() + options.ping_timeout);
                continue;
            }
        };
        let text = match message {
            None => return Ok(()),
            Some(Err(WsError::Capacity(_))) => {
                close(&mut socket, CloseCode::Size, "packet too large").await;
                return Ok(());
            }
            Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => return Ok(()),
            Some(Err(error)) => return Err(error),
            Some(Ok(Message::Text(text))) => text.to_string(),
            Some(Ok(Message::Close(frame))) => {
                let _ = socket.send(Message::Close(frame)).await;
                return Ok(());
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(_)) => {
                close(&mut socket, CloseCode::Unsupported, "binary packets are not supported").await;
                return Ok(());
            }
        };
        match text.as_bytes().first() {
            Some(b'3') if text == "3" => {
                if pong_by.take().is_some() {
                    ping_at = Some(Instant::now() + options.ping_interval);
                }
                continue;
            }
            Some(b'1') if text == "1" => {
                close(&mut socket, CloseCode::Normal, "").await;
                return Ok(());
            }
            Some(b'4') => {}
            _ => {
                close(&mut socket, CloseCode::Protocol, "unexpected Engine.IO packet").await;
                return Ok(());
            }
        }
        let packet = match parse_packet(&text[1..]) {
            Ok(packet) => packet,
            Err(reason) => {
                close(&mut socket, CloseCode::Protocol, reason).await;
                return Ok(());
            }
        };
        match (packet, connect_by) {
            (Packet::Connect { namespace, auth }, Some(connect_limit)) => {
                if namespace != "/" {
                    send_text(&mut socket, format!("44{namespace},{}", json!({"message":"Invalid namespace"}))).await?;
                    close(&mut socket, CloseCode::Policy, "Invalid namespace").await;
                    return Ok(());
                }
                let token = auth
                    .as_ref()
                    .and_then(|auth| auth.get("token"))
                    .and_then(Value::as_str)
                    .filter(|token| !token.is_empty() && token.len() <= MAX_AUTH_TOKEN_LENGTH);
                let Some(token) = token else {
                    send_text(&mut socket, connect_error("WIRE.UNAUTHENTICATED")).await?;
                    close(&mut socket, CloseCode::Policy, "WIRE.UNAUTHENTICATED").await;
                    return Ok(());
                };
                let verdict = match tokio::time::timeout_at(connect_limit, authenticate(token)).await {
                    Ok(verdict) => verdict,
                    Err(_) => Err("WIRE.DB_UNAVAILABLE"),
                };
                match verdict {
                    Ok(remaining) => {
                        token_by = Some(Instant::now() + remaining);
                        send_text(&mut socket, format!("40{}", json!({"sid": session_id()}))).await?;
                    }
                    Err(code) => {
                        send_text(&mut socket, connect_error(code)).await?;
                        close(&mut socket, CloseCode::Policy, code).await;
                        return Ok(());
                    }
                }
                connect_by = None;
            }
            // Before the first CONNECT only that packet may come.
            (Packet::Event { .. } | Packet::Disconnect, Some(_)) => {
                close(&mut socket, CloseCode::Policy, "CONNECT required").await;
                return Ok(());
            }
            (Packet::Connect { .. }, None) => {
                close(&mut socket, CloseCode::Protocol, "already connected").await;
                return Ok(());
            }
            (Packet::Disconnect, None) => {
                close(&mut socket, CloseCode::Normal, "").await;
                return Ok(());
            }
            (Packet::Event { namespace, ack, args }, None) => {
                if namespace != "/" {
                    close(&mut socket, CloseCode::Protocol, "namespace not connected").await;
                    return Ok(());
                }
                // Like a Socket.IO server: an event nobody asked an answer for is dropped, whatever its name.
                let Some(ack) = ack else { continue };
                let body = if args[0] == "watch_frame" {
                    match parse_watch_frame(args.get(1)) {
                        // Analysis and storage are later slices; this one never invents an emotion.
                        Ok(_) => json!({"status":"error","code":"WIRE.NOT_AVAILABLE","message":"Frame analysis is not available yet"}),
                        Err(message) => json!({"status":"error","code":"WIRE.BAD_REQUEST","message":message}),
                    }
                } else {
                    json!({"status":"error","code":"WIRE.BAD_REQUEST","message":"Unknown event"})
                };
                send_text(&mut socket, format!("43{ack}[{body}]")).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio::io::DuplexStream;
    use tokio_tungstenite::tungstenite::protocol::Role;

    type Client = WebSocketStream<DuplexStream>;

    fn options() -> SocketIoOptions {
        SocketIoOptions {
            max_packet_bytes: 4096,
            ping_interval: Duration::from_secs(30),
            ping_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_millis(300),
            max_lifetime: Duration::from_secs(30),
        }
    }

    struct Harness {
        client: Client,
        server: tokio::task::JoinHandle<Result<(), tokio_tungstenite::tungstenite::Error>>,
        tokens: Arc<Mutex<Vec<String>>>,
        calls: Arc<AtomicUsize>,
    }

    /// `verdict` decides the answer for every token: `Ok(remaining)` or a wire code.
    async fn connect(options: SocketIoOptions, verdict: Result<Duration, &'static str>) -> Harness {
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let tokens = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let (seen, count) = (tokens.clone(), calls.clone());
        let server = tokio::spawn(async move {
            serve(server_io, &options, move |token: &str| {
                seen.lock().unwrap().push(token.to_owned());
                count.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { verdict })
            })
            .await
        });
        let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        Harness { client, server, tokens, calls }
    }

    async fn next_text(client: &mut Client) -> String {
        match tokio::time::timeout(Duration::from_secs(5), client.next()).await.expect("server answers in time") {
            Some(Ok(Message::Text(text))) => text.to_string(),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }

    async fn next_close(client: &mut Client) -> Option<CloseFrame> {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), client.next()).await.expect("server closes in time") {
                Some(Ok(Message::Close(frame))) => return frame,
                Some(Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Err(_)) | None => return None,
                Some(Ok(other)) => panic!("unexpected frame {other:?}"),
            }
        }
    }

    async fn send(client: &mut Client, text: &str) {
        client.send(Message::Text(text.to_owned().into())).await.unwrap();
    }

    async fn open(harness: &mut Harness) -> Value {
        let text = next_text(&mut harness.client).await;
        assert!(text.starts_with('0'), "Engine.IO open packet first: {text}");
        serde_json::from_str(&text[1..]).unwrap()
    }

    async fn connected(options: SocketIoOptions) -> Harness {
        let mut harness = connect(options, Ok(Duration::from_secs(60))).await;
        open(&mut harness).await;
        send(&mut harness.client, r#"40{"token":"good"}"#).await;
        let reply = next_text(&mut harness.client).await;
        assert!(reply.starts_with("40{\"sid\":\""), "{reply}");
        harness
    }

    fn frame(extra: Value) -> Value {
        let mut base = json!({
            "video_view_log_id": "11111111-1111-4111-8111-111111111111",
            "youtube_running_time": 12.5,
            "frame_data": "data:image/jpeg;base64,AAAA",
            "duration": 120,
            "user_id": "client-claimed",
            "video_id": "22222222-2222-4222-8222-222222222222"
        });
        for (key, value) in extra.as_object().unwrap() {
            if value.is_null() && key.starts_with('-') {
                base.as_object_mut().unwrap().remove(&key[1..]);
            } else {
                base[key] = value.clone();
            }
        }
        base
    }

    #[test]
    fn only_the_websocket_only_engine_io_v4_target_is_served() {
        for good in
            ["/socket.io/?EIO=4&transport=websocket", "/socket.io/?transport=websocket&EIO=4", "/socket.io/?EIO=4&transport=websocket&t=Ab-_9"]
        {
            assert!(is_websocket_target(good), "{good}");
        }
        for bad in [
            "/socket.io/",
            "/socket.io/?",
            "/socket.io/?EIO=4",
            "/socket.io/?EIO=4&transport=polling",
            "/socket.io/?EIO=3&transport=websocket",
            "/socket.io/?EIO=4&transport=websocket&sid=abc",
            "/socket.io/?EIO=4&EIO=4&transport=websocket",
            "/socket.io/?EIO=4&transport=websocket&t=",
            "/socket.io/?EIO=4&transport=websocket&t=a%20b",
            "/socket.io?EIO=4&transport=websocket",
            "/socket.io/x?EIO=4&transport=websocket",
            "/events/socket",
        ] {
            assert!(!is_websocket_target(bad), "{bad}");
        }
        assert!(is_endpoint("/socket.io/") && is_endpoint("/socket.io/?EIO=3") && !is_endpoint("/socket.io") && !is_endpoint("/socket.io/x"));
    }

    #[test]
    fn options_are_bounded() {
        assert!(SocketIoOptions::default().validate().is_ok());
        for bad in [
            SocketIoOptions { max_packet_bytes: 10, ..Default::default() },
            SocketIoOptions { max_packet_bytes: 64 * 1024 * 1024, ..Default::default() },
            SocketIoOptions { ping_interval: Duration::ZERO, ..Default::default() },
            SocketIoOptions { ping_timeout: Duration::ZERO, ..Default::default() },
            SocketIoOptions { connect_timeout: Duration::from_secs(120), ..Default::default() },
            SocketIoOptions { max_lifetime: Duration::from_secs(48 * 60 * 60), ..Default::default() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn packets_decode_the_forms_the_original_client_sends() {
        assert_eq!(parse_packet("0").unwrap(), Packet::Connect { namespace: "/".into(), auth: None });
        assert_eq!(parse_packet(r#"0{"token":"t"}"#).unwrap(), Packet::Connect { namespace: "/".into(), auth: Some(json!({"token":"t"})) });
        assert_eq!(parse_packet("0/admin,").unwrap(), Packet::Connect { namespace: "/admin".into(), auth: None });
        assert_eq!(
            parse_packet(r#"0/admin,{"token":"t"}"#).unwrap(),
            Packet::Connect { namespace: "/admin".into(), auth: Some(json!({"token":"t"})) }
        );
        assert_eq!(parse_packet("1").unwrap(), Packet::Disconnect);
        assert_eq!(
            parse_packet(r#"2["watch_frame",{"a":1}]"#).unwrap(),
            Packet::Event { namespace: "/".into(), ack: None, args: vec![json!("watch_frame"), json!({"a":1})] }
        );
        assert_eq!(
            parse_packet(r#"212["watch_frame",{}]"#).unwrap(),
            Packet::Event { namespace: "/".into(), ack: Some(12), args: vec![json!("watch_frame"), json!({})] }
        );
        assert_eq!(parse_packet(r#"2/x,3["e"]"#).unwrap(), Packet::Event { namespace: "/x".into(), ack: Some(3), args: vec![json!("e")] });
    }

    #[test]
    fn packets_reject_what_this_transport_does_not_speak() {
        for bad in [
            "",
            "3",
            "31[]",
            "4",
            "5",
            "51-[\"e\"]",
            "6",
            "7",
            "x",
            "2",
            "2[]",
            "2{}",
            "2\"e\"",
            "2[1]",
            "2[\"e\"",
            "2 [\"e\"]",
            "0[1]",
            "0\"x\"",
            "099999999999999999999999[\"e\"]",
            "2/x",
            "2/x,[\"e\"] ",
            "1x",
        ] {
            assert!(parse_packet(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(parse_packet("299999999999999999999[\"e\"]").is_err(), "an acknowledgement id beyond u64 is rejected");
    }

    #[test]
    fn watch_frames_need_the_fields_the_original_requires_and_ignore_the_client_user_id() {
        let ok = parse_watch_frame(Some(&frame(json!({})))).unwrap();
        assert_eq!(
            ok,
            WatchFrame {
                video_view_log_id: "11111111-1111-4111-8111-111111111111".into(),
                video_id: "22222222-2222-4222-8222-222222222222".into(),
                youtube_running_time: 12.5,
                duration: 120.0,
                frame_data: "data:image/jpeg;base64,AAAA".into(),
            }
        );
        // No user_id at all is fine: the server takes the user from the authenticated principal.
        assert!(parse_watch_frame(Some(&frame(json!({"-user_id": null})))).is_ok());
        // A running time of 0 counts as present (the original checks `is not None`), a missing duration is 0.
        assert_eq!(parse_watch_frame(Some(&frame(json!({"youtube_running_time": 0, "-duration": null})))).unwrap().duration, 0.0);
        for missing in ["video_view_log_id", "video_id", "youtube_running_time", "frame_data"] {
            let error = parse_watch_frame(Some(&frame(json!({ format!("-{missing}"): null })))).unwrap_err();
            assert_eq!(error, "Missing required fields", "{missing}");
        }
        for empty in [("video_view_log_id", json!("")), ("video_id", json!("")), ("frame_data", json!("")), ("youtube_running_time", Value::Null)] {
            assert_eq!(parse_watch_frame(Some(&frame(json!({ empty.0: empty.1 })))).unwrap_err(), "Missing required fields", "{}", empty.0);
        }
        assert_eq!(parse_watch_frame(None).unwrap_err(), "Missing required fields");
        assert_eq!(parse_watch_frame(Some(&json!("x"))).unwrap_err(), "Missing required fields");
        assert_eq!(parse_watch_frame(Some(&json!([1]))).unwrap_err(), "Missing required fields");
    }

    #[test]
    fn watch_frames_reject_wrong_types_and_out_of_range_numbers() {
        for (field, value) in [
            ("video_view_log_id", json!(5)),
            ("video_view_log_id", json!("a".repeat(129))),
            ("video_view_log_id", json!("bad\u{0}id")),
            ("video_id", json!(["x"])),
            ("video_id", json!("a".repeat(129))),
            ("frame_data", json!(7)),
            ("youtube_running_time", json!("1.0")),
            ("youtube_running_time", json!(-0.5)),
            ("youtube_running_time", json!(1.0e9)),
            ("duration", json!("120")),
            ("duration", json!(-1)),
            ("duration", json!(1.0e9)),
        ] {
            let error = parse_watch_frame(Some(&frame(json!({ field: value.clone() })))).unwrap_err();
            assert_eq!(error, format!("Invalid field: {field}"), "{field}={value}");
        }
    }

    #[tokio::test]
    async fn the_open_packet_advertises_the_engine_io_limits_and_no_upgrades() {
        let mut harness = connect(options(), Ok(Duration::from_secs(60))).await;
        let info = open(&mut harness).await;
        assert_eq!(info["upgrades"], json!([]));
        assert_eq!(info["pingInterval"], 30_000);
        assert_eq!(info["pingTimeout"], 30_000);
        assert_eq!(info["maxPayload"], 4096);
        let sid = info["sid"].as_str().unwrap();
        assert_eq!(sid.len(), 20);
        assert!(sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{sid}");
        // Two connections never share a session id.
        let mut other = connect(options(), Ok(Duration::from_secs(60))).await;
        assert_ne!(open(&mut other).await["sid"], info["sid"]);
    }

    #[tokio::test]
    async fn no_connect_packet_before_the_deadline_closes_the_connection_without_any_authentication() {
        let mut harness = connect(options(), Ok(Duration::from_secs(60))).await;
        open(&mut harness).await;
        let close = next_close(&mut harness.client).await.expect("a close frame with a reason");
        assert_eq!(close.code, CloseCode::Policy);
        harness.server.await.unwrap().unwrap();
        assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn connect_with_a_valid_token_answers_with_a_socket_id_and_checks_exactly_that_token() {
        let mut harness = connect(options(), Ok(Duration::from_secs(60))).await;
        let engine_sid = open(&mut harness).await["sid"].clone();
        send(&mut harness.client, r#"40{"token":"good-token"}"#).await;
        let reply = next_text(&mut harness.client).await;
        let body: Value = serde_json::from_str(reply.strip_prefix("40").expect("CONNECT reply")).unwrap();
        let sid = body["sid"].as_str().unwrap();
        assert_eq!(sid.len(), 20);
        assert_ne!(body["sid"], engine_sid, "the Socket.IO sid is not the Engine.IO sid");
        assert_eq!(*harness.tokens.lock().unwrap(), vec!["good-token".to_owned()]);
    }

    #[tokio::test]
    async fn connect_without_a_usable_token_is_refused_with_connect_error_and_never_reaches_the_verifier() {
        let long = format!(r#"0{{"token":"{}"}}"#, "t".repeat(MAX_AUTH_TOKEN_LENGTH + 1));
        for packet in [
            "40".to_owned(),
            "40{}".to_owned(),
            r#"40{"token":""}"#.to_owned(),
            r#"40{"token":5}"#.to_owned(),
            r#"40{"token":null}"#.to_owned(),
            format!("4{long}"),
        ] {
            let mut harness = connect(SocketIoOptions { max_packet_bytes: 16 * 1024, ..options() }, Ok(Duration::from_secs(60))).await;
            open(&mut harness).await;
            send(&mut harness.client, &packet).await;
            let reply = next_text(&mut harness.client).await;
            let body: Value =
                serde_json::from_str(reply.strip_prefix("44").unwrap_or_else(|| panic!("CONNECT_ERROR for {packet:.40}, got {reply}"))).unwrap();
            assert_eq!(body["message"], "WIRE.UNAUTHENTICATED", "{packet:.40}");
            assert_eq!(body["data"]["code"], "WIRE.UNAUTHENTICATED");
            next_close(&mut harness.client).await;
            harness.server.await.unwrap().unwrap();
            assert_eq!(harness.calls.load(Ordering::SeqCst), 0, "{packet:.40}");
        }
    }

    #[tokio::test]
    async fn a_refused_token_gets_the_verifiers_code_and_the_connection_ends() {
        for code in ["WIRE.UNAUTHENTICATED", "WIRE.TOKEN_EXPIRED", "WIRE.SERVER_BUSY", "WIRE.RATE_LIMITED", "WIRE.DB_UNAVAILABLE"] {
            let mut harness = connect(options(), Err(code)).await;
            open(&mut harness).await;
            send(&mut harness.client, r#"40{"token":"bad"}"#).await;
            let reply = next_text(&mut harness.client).await;
            let body: Value = serde_json::from_str(reply.strip_prefix("44").expect("CONNECT_ERROR")).unwrap();
            assert_eq!((body["message"].as_str(), body["data"]["code"].as_str()), (Some(code), Some(code)));
            assert!(next_close(&mut harness.client).await.is_some());
            harness.server.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn a_namespace_other_than_the_main_one_is_refused() {
        let mut harness = connect(options(), Ok(Duration::from_secs(60))).await;
        open(&mut harness).await;
        send(&mut harness.client, r#"40/admin,{"token":"good"}"#).await;
        assert_eq!(next_text(&mut harness.client).await, r#"44/admin,{"message":"Invalid namespace"}"#);
        next_close(&mut harness.client).await;
        assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_event_before_connect_closes_the_connection_without_an_answer() {
        let mut harness = connect(options(), Ok(Duration::from_secs(60))).await;
        open(&mut harness).await;
        send(&mut harness.client, &format!("421{}", json!(["watch_frame", frame(json!({}))]))).await;
        let close = next_close(&mut harness.client).await.expect("close frame");
        assert_eq!(close.code, CloseCode::Policy);
        assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_valid_frame_is_acknowledged_as_not_yet_processed_and_never_as_an_emotion() {
        let mut harness = connected(options()).await;
        send(&mut harness.client, &format!("427{}", json!(["watch_frame", frame(json!({}))]))).await;
        let reply = next_text(&mut harness.client).await;
        let body: Value = serde_json::from_str(reply.strip_prefix("437").expect("ACK with id 7")).unwrap();
        assert_eq!(body[0]["status"], "error");
        assert_eq!(body[0]["code"], "WIRE.NOT_AVAILABLE");
        assert!(body[0].get("response").is_none(), "no invented analysis result: {body}");
        // The token was checked once, at CONNECT.
        assert_eq!(harness.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_invalid_frame_gets_a_status_error_acknowledgement_in_the_shape_the_client_reads() {
        let mut harness = connected(options()).await;
        send(&mut harness.client, &format!("429{}", json!(["watch_frame", frame(json!({"-video_id": null}))]))).await;
        let reply = next_text(&mut harness.client).await;
        let body: Value = serde_json::from_str(reply.strip_prefix("439").unwrap()).unwrap();
        assert_eq!(body, json!([{"status":"error","code":"WIRE.BAD_REQUEST","message":"Missing required fields"}]));
        // The connection stays usable after a bad frame.
        send(&mut harness.client, &format!("4210{}", json!(["watch_frame", frame(json!({}))]))).await;
        assert!(next_text(&mut harness.client).await.starts_with("4310"));
    }

    #[tokio::test]
    async fn events_without_an_acknowledgement_id_are_dropped_silently_and_unknown_events_with_one_get_an_error() {
        let mut harness = connected(options()).await;
        send(&mut harness.client, &format!("42{}", json!(["watch_frame", frame(json!({}))]))).await;
        send(&mut harness.client, r#"42["no_such_event",{}]"#).await;
        send(&mut harness.client, r#"423["no_such_event",{}]"#).await;
        let reply = next_text(&mut harness.client).await;
        let body: Value = serde_json::from_str(reply.strip_prefix("433").expect("only the acknowledged unknown event answers")).unwrap();
        assert_eq!(body, json!([{"status":"error","code":"WIRE.BAD_REQUEST","message":"Unknown event"}]));
    }

    #[tokio::test]
    async fn a_packet_over_the_limit_ends_the_connection_with_message_too_big_and_no_answer() {
        let mut harness = connected(options()).await;
        let big = frame(json!({"frame_data": "A".repeat(5000)}));
        // The send itself may fail once the server has already closed; the close code is what matters.
        let _ = harness.client.send(Message::Text(format!("421{}", json!(["watch_frame", big])).into())).await;
        let close = next_close(&mut harness.client).await.expect("close frame");
        assert_eq!(close.code, CloseCode::Size);
        let _ = harness.server.await;
    }

    #[tokio::test]
    async fn binary_frames_and_binary_packets_are_not_spoken_here() {
        let mut harness = connected(options()).await;
        harness.client.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
        assert_eq!(next_close(&mut harness.client).await.expect("close").code, CloseCode::Unsupported);
        let mut harness = connected(options()).await;
        send(&mut harness.client, r#"451-["watch_frame",{"_placeholder":true,"num":0}]"#).await;
        assert_eq!(next_close(&mut harness.client).await.expect("close").code, CloseCode::Protocol);
    }

    #[tokio::test]
    async fn a_second_connect_or_a_server_only_packet_is_a_protocol_error() {
        for packet in [r#"40{"token":"good"}"#, "43[]", r#"44{"message":"x"}"#, "3probe", "5", "6"] {
            let mut harness = connected(options()).await;
            send(&mut harness.client, packet).await;
            let close = next_close(&mut harness.client).await.expect("close");
            assert_eq!(close.code, CloseCode::Protocol, "{packet}");
        }
    }

    #[tokio::test]
    async fn disconnect_closes_only_the_transport_and_changes_nothing_else() {
        let mut harness = connected(options()).await;
        send(&mut harness.client, "41").await;
        assert_eq!(next_close(&mut harness.client).await.expect("close").code, CloseCode::Normal);
        harness.server.await.unwrap().unwrap();
        assert_eq!(harness.calls.load(Ordering::SeqCst), 1, "no re-authentication, no other work, on disconnect");
    }

    #[tokio::test]
    async fn the_engine_io_close_packet_and_a_vanished_client_end_the_server_side() {
        let mut harness = connected(options()).await;
        send(&mut harness.client, "1").await;
        next_close(&mut harness.client).await;
        harness.server.await.unwrap().unwrap();

        let harness = connected(options()).await;
        let Harness { client, server, .. } = harness;
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server notices the dropped transport").unwrap().ok();
    }

    #[tokio::test]
    async fn the_server_pings_and_a_client_that_never_answers_is_closed_after_the_ping_timeout() {
        let quick = SocketIoOptions { ping_interval: Duration::from_millis(50), ping_timeout: Duration::from_millis(150), ..options() };
        let mut harness = connected(quick.clone()).await;
        assert_eq!(next_text(&mut harness.client).await, "2");
        send(&mut harness.client, "3").await;
        assert_eq!(next_text(&mut harness.client).await, "2", "an answered ping keeps the connection alive");
        // This one is not answered.
        let close = next_close(&mut harness.client).await.expect("close frame");
        assert_eq!(close.code, CloseCode::Policy);
        harness.server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_connection_ends_when_its_token_would_have_expired() {
        let mut harness = connect(options(), Ok(Duration::from_millis(200))).await;
        open(&mut harness).await;
        send(&mut harness.client, r#"40{"token":"short"}"#).await;
        assert!(next_text(&mut harness.client).await.starts_with("40{"));
        let close = next_close(&mut harness.client).await.expect("close frame");
        assert_eq!(close.code, CloseCode::Policy);
        assert_eq!(close.reason.as_str(), "WIRE.TOKEN_EXPIRED");
    }

    #[tokio::test]
    async fn a_verifier_that_never_answers_is_cut_off_by_the_connect_timeout() {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move { serve(server_io, &options(), |_: &str| -> AdmitFuture { Box::pin(std::future::pending()) }).await });
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        next_text(&mut client).await;
        send(&mut client, r#"40{"token":"x"}"#).await;
        let reply = next_text(&mut client).await;
        assert!(reply.starts_with("44") && reply.contains("WIRE.DB_UNAVAILABLE"), "{reply}");
        server.await.unwrap().unwrap();
    }
}
