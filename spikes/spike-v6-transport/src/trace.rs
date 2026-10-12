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
    started: Instant,
    method: String,
    route: &'static str,
    status: Option<u16>,
    code: Option<String>,
}

impl Default for Trace {
    fn default() -> Self {
        Trace { id: generate_id(), started: Instant::now(), method: String::new(), route: "other", status: None, code: None }
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
        t.record("200 OK", "{\"code\":\"BAD_VALUE\",\"msg\":\"password=hunter2\",\"ok\":false}");
        let line = t.line();
        let v: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
        assert_eq!((v["event"].as_str(), v["method"].as_str(), v["route"].as_str()), (Some("request"), Some("POST"), Some("apply")));
        assert_eq!((v["status"].as_u64(), v["code"].as_str()), (Some(200), Some("BAD_VALUE")));
        assert!(!line.contains("hunter2") && !line.contains("abc"), "{line}");
        t.record("200 OK", "{\"ok\":true}");
        assert!(t.line().contains("\"status\":200") && !t.line().contains("BAD_VALUE"), "a later reply replaces the earlier outcome");
    }
}
