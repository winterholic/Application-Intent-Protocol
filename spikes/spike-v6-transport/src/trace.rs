//! Request correlation: every request gets an id, the id is returned in `x-request-id` on the JSON replies,
//! and one structured line per finished request goes to stderr with the same id. A caller that reports a
//! failure gives that id; the operator finds the line, and the line says which route, what outcome and how long.
//!
//! The line never carries a token, a principal, a query string or a body: only the route name (a fixed set),
//! the HTTP method, the status the reply was sent with, the AIP error code of a failed reply and the duration.
//! Set `AIP_REQUEST_LOG=off` to silence the lines; the header stays.
//!
//! The line is handed to a writer thread through a bounded queue and is dropped, with a count, when the queue
//! is full. Writing to stderr from the request task would let a slow or blocked log pipe stall a runtime worker
//! and delay the release of the connection permit; observability must never slow the request it observes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::time::Instant;

const QUEUE: usize = 4096;
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// The queue to the writer thread, or `None` when the log is switched off.
fn sink() -> Option<&'static SyncSender<String>> {
    static SINK: OnceLock<Option<SyncSender<String>>> = OnceLock::new();
    SINK.get_or_init(|| {
        if std::env::var("AIP_REQUEST_LOG").is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
            return None;
        }
        let (tx, rx) = sync_channel::<String>(QUEUE);
        std::thread::Builder::new()
            .name("aip-request-log".into())
            .spawn(move || {
                for line in rx {
                    let dropped = DROPPED.swap(0, Ordering::Relaxed);
                    if dropped > 0 {
                        eprintln!("{{\"event\":\"request_log_dropped\",\"count\":{dropped}}}");
                    }
                    eprintln!("{line}");
                }
            })
            .ok()?;
        Some(tx)
    })
    .as_ref()
}

/// Routes named in the log. Anything else (static mounts, file mounts, unknown paths) is `other`, so that a
/// path that holds an object key or a file name never reaches the log.
const ROUTES: &[&str] = &[
    "session",
    "read",
    "apply",
    "status",
    "extension",
    "operation",
    "capabilities",
    "jobs",
    "files",
    "events",
    "rag",
    "objects",
    "messages",
    "health",
    "ready",
    "metrics",
];

pub struct Trace {
    id: String,
    context: aip_execution::trace_context::TraceContext,
    /// `traceparent` headers seen. A request with more than one has an invalid trace context and starts a new trace.
    traceparents: u8,
    started: Instant,
    method: String,
    route: &'static str,
    status: Option<u16>,
    code: Option<String>,
}

impl Default for Trace {
    fn default() -> Self {
        Trace {
            id: generate_id(),
            context: aip_execution::trace_context::TraceContext::generate(),
            traceparents: 0,
            started: Instant::now(),
            method: String::new(),
            route: "other",
            status: None,
            code: None,
        }
    }
}

impl Trace {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Takes the id a trusted upstream (a reverse proxy, a server-side caller) sent, when it is a plausible
    /// id; otherwise the generated one stays. The value is echoed in a header and a log line, so only a short
    /// token of safe characters is accepted.
    pub fn accept_inbound(&mut self, value: &str) {
        if (8..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
            self.id = value.to_string();
        }
    }

    /// The W3C trace context of this request: the caller's when it sent one `traceparent`, otherwise a trace that
    /// starts here. Outbound calls of the request carry its trace id, and the log line names it.
    pub fn context(&self) -> &aip_execution::trace_context::TraceContext {
        &self.context
    }

    /// Takes an inbound `traceparent`. Only a well-formed single header counts: a malformed one is ignored and a
    /// second one discards both, as the W3C specification says, so the trace id in the log is always one the caller meant.
    pub fn accept_traceparent(&mut self, value: &str) {
        self.traceparents = self.traceparents.saturating_add(1);
        self.context = match (self.traceparents, aip_execution::trace_context::TraceContext::parse(value)) {
            (1, Some(inbound)) => inbound,
            _ => aip_execution::trace_context::TraceContext::generate(),
        };
    }

    pub fn begin(&mut self, method: &str, path: &str) {
        self.method = method.chars().filter(char::is_ascii_alphabetic).take(8).collect();
        self.route = route_of(path);
    }

    /// Remembers what the reply said: the status of its line (`"200 OK"`) and, for a failed reply, its `code`.
    pub fn record(&mut self, status_line: &str, body: &str) {
        self.status = status_line.split(' ').next().and_then(|s| s.parse().ok());
        self.code = None;
        // Replies are `{"ok":false,"code":...}` objects; reading the code of a large success body would cost
        // a second parse for nothing.
        if body.len() <= 64 * 1024 && body.contains("\"ok\":false") {
            self.code = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| {
                v["code"]
                    .as_str()
                    .map(|c| c.chars().filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_' || *c == '.').take(64).collect())
            });
        }
    }

    /// The line the operator reads. Field order is fixed so that lines compare and grep well.
    pub fn line(&self) -> String {
        let mut line = serde_json::Map::new();
        line.insert("event".into(), "request".into());
        line.insert("requestId".into(), self.id.clone().into());
        line.insert("traceId".into(), self.context.trace_id().into());
        line.insert("method".into(), self.method.clone().into());
        line.insert("route".into(), self.route.into());
        line.insert("status".into(), self.status.map_or(serde_json::Value::Null, Into::into));
        if let Some(code) = &self.code {
            line.insert("code".into(), code.clone().into());
        }
        line.insert("ms".into(), (self.started.elapsed().as_millis() as u64).into());
        serde_json::Value::Object(line).to_string()
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        crate::metrics::observe(self.route, self.status, self.code.as_deref(), self.started.elapsed());
        if let Some(tx) = sink() {
            if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = tx.try_send(self.line()) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// A writer that gives the replies of the routes with their own head writers (static files, object streams, managed
/// files, SSE, the WebSocket upgrade) the same correlation the JSON replies have: the first write of a reply is an
/// HTTP/1.1 head, so `x-request-id` is inserted after its status line, the status is remembered for the log line,
/// and a browser is allowed to read the header when the reply is a CORS one. Those writers do not know the request,
/// and this keeps them from having to.
pub struct HeadTap<'a, W> {
    inner: &'a mut W,
    trace: &'a mut Trace,
    seen: bool,
    pending: Vec<u8>,
    sent: usize,
    claimed: usize,
}

impl<'a, W> HeadTap<'a, W> {
    pub fn new(inner: &'a mut W, trace: &'a mut Trace) -> Self {
        HeadTap { inner, trace, seen: false, pending: Vec::new(), sent: 0, claimed: 0 }
    }

    /// The head with the id inserted, or `None` when the write is not the start of an HTTP/1.1 reply.
    fn rewrite(&mut self, buf: &[u8]) -> Option<Vec<u8>> {
        if !buf.starts_with(b"HTTP/1.1 ") {
            return None;
        }
        let status_end = buf.windows(2).position(|pair| pair == b"\r\n")?;
        let head_end = buf.windows(4).position(|quad| quad == b"\r\n\r\n");
        let head = String::from_utf8_lossy(&buf[..head_end.unwrap_or(buf.len())]).to_ascii_lowercase();
        let body = head_end.and_then(|end| std::str::from_utf8(&buf[end + 4..]).ok()).unwrap_or("");
        self.trace.record(&String::from_utf8_lossy(&buf[9..status_end]), body);
        let mut inserted = String::new();
        if !head.contains("\r\nx-request-id:") {
            inserted.push_str(&format!("x-request-id: {}\r\n", self.trace.id()));
        }
        if head.contains("\r\naccess-control-allow-origin:") {
            inserted.push_str("access-control-expose-headers: x-request-id\r\n");
        }
        let mut out = Vec::with_capacity(buf.len() + inserted.len());
        out.extend_from_slice(&buf[..status_end + 2]);
        out.extend_from_slice(inserted.as_bytes());
        out.extend_from_slice(&buf[status_end + 2..]);
        Some(out)
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> HeadTap<'_, W> {
    fn drain(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        while self.sent < self.pending.len() {
            match std::pin::Pin::new(&mut *self.inner).poll_write(cx, &self.pending[self.sent..]) {
                Poll::Ready(Ok(0)) => return Poll::Ready(Err(std::io::ErrorKind::WriteZero.into())),
                Poll::Ready(Ok(n)) => self.sent += n,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.pending.clear();
        self.sent = 0;
        Poll::Ready(Ok(()))
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for HeadTap<'_, W> {
    fn poll_write(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        use std::task::Poll;
        let this = self.get_mut();
        if !this.pending.is_empty() {
            match this.drain(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
            if this.claimed > 0 {
                // The head the caller offered was taken earlier and has now left in full.
                return Poll::Ready(Ok(std::mem::take(&mut this.claimed)));
            }
        }
        if !this.seen {
            this.seen = true;
            if let Some(rewritten) = this.rewrite(buf) {
                this.pending = rewritten;
                this.claimed = buf.len();
                // The caller is told its whole buffer was taken only once the longer bytes have left, so a writer
                // that never flushes cannot lose the tail of the head.
                return match this.drain(cx) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(std::mem::take(&mut this.claimed))),
                    Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                    Poll::Pending => Poll::Pending,
                };
            }
        }
        std::pin::Pin::new(&mut *this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match this.drain(cx) {
            std::task::Poll::Ready(Ok(())) => std::pin::Pin::new(&mut *this.inner).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match this.drain(cx) {
            std::task::Poll::Ready(Ok(())) => std::pin::Pin::new(&mut *this.inner).poll_shutdown(cx),
            other => other,
        }
    }
}

fn route_of(path: &str) -> &'static str {
    let segment = path.split(['?', '#']).next().unwrap_or("").trim_start_matches('/').split('/').next().unwrap_or("");
    ROUTES.iter().find(|r| **r == segment).copied().unwrap_or("other")
}

fn generate_id() -> String {
    let mut bytes = [0u8; 12];
    if getrandom::getrandom(&mut bytes).is_err() {
        // No entropy source: a counter and the clock still make the id unique inside one process.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        bytes[..8].copy_from_slice(&(t ^ n.rotate_left(32)).to_le_bytes());
        bytes[8..].copy_from_slice(&(n as u32).to_le_bytes());
    }
    let mut id = String::from("req_");
    for b in bytes {
        id.push_str(&format!("{b:02x}"));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_shaped() {
        let (a, b) = (generate_id(), generate_id());
        assert_ne!(a, b);
        assert!(a.starts_with("req_") && a.len() == 4 + 24 && a[4..].bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
    }

    #[test]
    fn an_inbound_id_is_taken_only_when_it_is_a_safe_token() {
        let mut t = Trace::default();
        let generated = t.id().to_string();
        for bad in ["short", "has space in it", "line\nbreak-123", "x".repeat(65).as_str(), "semi;colon-1234", "ünïcode-12345"] {
            t.accept_inbound(bad);
            assert_eq!(t.id(), generated, "{bad:?} must not be taken");
        }
        t.accept_inbound("proxy-7f3a9c.01_ab");
        assert_eq!(t.id(), "proxy-7f3a9c.01_ab");
    }

    #[test]
    fn the_route_is_a_fixed_name_and_never_a_path() {
        assert_eq!(route_of("/read"), "read");
        assert_eq!(route_of("/jobs/enqueue?x=1"), "jobs");
        assert_eq!(route_of("/objects/7b1c-secret-key"), "objects");
        assert_eq!(route_of("/reports/payroll-2026.pdf"), "other");
        assert_eq!(route_of(""), "other");
    }

    #[test]
    fn the_line_has_the_outcome_and_no_payload() {
        let mut t = Trace::default();
        t.begin("POST", "/apply?token=abc");
        t.record("200 OK", "{\"code\":\"WIRE.BAD_VALUE\",\"msg\":\"password=hunter2\",\"ok\":false}");
        let line = t.line();
        let v: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
        assert_eq!((v["event"].as_str(), v["method"].as_str(), v["route"].as_str()), (Some("request"), Some("POST"), Some("apply")));
        assert_eq!((v["status"].as_u64(), v["code"].as_str()), (Some(200), Some("WIRE.BAD_VALUE")));
        assert!(!line.contains("hunter2") && !line.contains("abc"), "{line}");
        t.record("200 OK", "{\"ok\":true}");
        assert!(t.line().contains("\"status\":200") && !t.line().contains("WIRE.BAD_VALUE"), "a later reply replaces the earlier outcome");
    }

    /// Writes `head` and then `chunks` through a tap over a tiny pipe (so the longer head needs several writes) and
    /// returns what the peer read and the log line.
    async fn through_tap(head: &str, chunks: &[&[u8]]) -> (String, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut client, mut server) = tokio::io::duplex(16);
        let mut trace = Trace::default();
        trace.begin("GET", "/assets/a.txt");
        let writer = async {
            let mut tap = HeadTap::new(&mut server, &mut trace);
            tap.write_all(head.as_bytes()).await.expect("write head");
            for chunk in chunks {
                tap.write_all(chunk).await.expect("write chunk");
            }
            tap.flush().await.expect("flush");
            drop(tap);
            drop(server);
        };
        let reader = async {
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.expect("read");
            String::from_utf8_lossy(&out).into_owned()
        };
        let ((), text) = tokio::join!(writer, reader);
        (text, trace.line())
    }

    #[tokio::test]
    async fn a_head_written_by_another_writer_gets_the_id_and_its_status_reaches_the_log() {
        let (text, line) = through_tap("HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n", &[b"hello"]).await;
        let id = text.lines().find_map(|l| l.strip_prefix("x-request-id: ")).expect("the header was inserted").to_string();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\nx-request-id: req_"), "{text}");
        assert!(text.ends_with("content-length: 5\r\n\r\nhello"), "the rest of the reply is untouched: {text}");
        let v: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
        assert_eq!((v["status"].as_u64(), v["requestId"].as_str()), (Some(200), Some(id.as_str())));
    }

    #[tokio::test]
    async fn a_cors_reply_exposes_the_id_and_a_json_failure_in_the_head_write_gives_its_code() {
        let head =
            "HTTP/1.1 404 Not Found\r\naccess-control-allow-origin: http://a.example\r\nvary: Origin\r\n\r\n{\"ok\":false,\"code\":\"WIRE.NOT_FOUND\"}";
        let (text, line) = through_tap(head, &[]).await;
        assert!(text.contains("\r\naccess-control-expose-headers: x-request-id\r\n"), "{text}");
        let v: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
        assert_eq!((v["status"].as_u64(), v["code"].as_str()), (Some(404), Some("WIRE.NOT_FOUND")));
        let (plain, _) = through_tap("HTTP/1.1 200 OK\r\n\r\n", &[]).await;
        assert!(!plain.contains("access-control-expose-headers"), "{plain}");
    }

    #[tokio::test]
    async fn bytes_that_are_not_a_head_pass_through_and_a_later_head_is_not_rewritten() {
        let (text, line) = through_tap("data: x\n\n", &[b"HTTP/1.1 200 OK\r\n\r\n"]).await;
        assert_eq!(text, "data: x\n\nHTTP/1.1 200 OK\r\n\r\n");
        let v: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
        assert!(v["status"].is_null(), "{line}");
    }

    #[test]
    fn the_log_line_names_the_callers_trace_and_a_second_header_discards_it() {
        let trace_id = "0af7651916cd43dd8448eb211c80319c";
        let mut t = Trace::default();
        t.begin("GET", "/read");
        let generated: serde_json::Value = serde_json::from_str(&t.line()).expect("one JSON object");
        let own = generated["traceId"].as_str().expect("a request without traceparent still has a trace id").to_owned();
        assert!(own.len() == 32 && own != trace_id && own.bytes().all(|b| b.is_ascii_hexdigit()), "{own}");

        let mut t = Trace::default();
        t.accept_traceparent(&format!("00-{trace_id}-b7ad6b7169203331-01"));
        let line: serde_json::Value = serde_json::from_str(&t.line()).expect("one JSON object");
        assert_eq!(line["traceId"], trace_id);
        assert!(!t.line().contains("b7ad6b7169203331"), "the caller's span id is not logged: {}", t.line());

        let mut malformed = Trace::default();
        malformed.accept_traceparent("not-a-traceparent");
        assert_ne!(malformed.context().trace_id(), "not-a-traceparent");
        assert_eq!(malformed.context().trace_id().len(), 32);

        let mut twice = Trace::default();
        twice.accept_traceparent(&format!("00-{trace_id}-b7ad6b7169203331-01"));
        twice.accept_traceparent(&format!("00-{trace_id}-b7ad6b7169203331-01"));
        assert_ne!(twice.context().trace_id(), trace_id, "two headers are an invalid context and start a new trace");
    }
}
