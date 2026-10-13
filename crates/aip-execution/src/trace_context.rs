//! W3C Trace Context (`traceparent`) for the HTTP calls the server makes while it handles one request.
//!
//! The transport parses the inbound header, keeps its trace id in the request log line, and stores the context here
//! for the task that handles the request. An outbound HTTP call made by that same task (an embedding or answer
//! provider, an object store) adds a `traceparent` with the same trace id, the sampled flag of the caller and a fresh
//! span id, so the caller's tracing backend and the provider's logs name one trace.
//!
//! This is propagation only. The server does not record spans and takes no exporter dependency; an OpenTelemetry
//! exporter is an optional package candidate. A task spawned from the request, such as a durable job step, does not
//! inherit the context: the trace of an enqueue request would have to be stored in the job row, which is a deployment
//! schema change and is left as a design decision.
use std::{cell::RefCell, future::Future};

tokio::task_local! {
    static CURRENT: RefCell<Option<TraceContext>>;
}

/// The trace a request belongs to: the 32-hex trace id and the trace flags of the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: String,
    flags: u8,
}

impl TraceContext {
    /// Reads a `traceparent` header. Version `00` has exactly four fields; a later version may append more after a
    /// `-`. The ids are lower-case hex and not all zero, and version `ff` is invalid, as the specification requires.
    pub fn parse(header: &str) -> Option<Self> {
        let mut fields = header.split('-');
        let (version, trace_id, parent_id, flags) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
        let lower_hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !lower_hex(version, 2) || version == "ff" || !lower_hex(trace_id, 32) || !lower_hex(parent_id, 16) || !lower_hex(flags, 2) {
            return None;
        }
        if trace_id.bytes().all(|b| b == b'0') || parent_id.bytes().all(|b| b == b'0') {
            return None;
        }
        if version == "00" && fields.next().is_some() {
            return None;
        }
        Some(Self { trace_id: trace_id.to_owned(), flags: u8::from_str_radix(flags, 16).ok()? })
    }

    /// A trace that starts at this server: a random trace id, not sampled.
    pub fn generate() -> Self {
        Self { trace_id: random_hex(16), flags: 0 }
    }

    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// The header an outbound call carries: the same trace id and flags, a new span id for the call.
    pub fn outgoing(&self) -> String {
        format!("00-{}-{}-{:02x}", self.trace_id, random_hex(8), self.flags)
    }
}

/// Lower-case hex of `bytes` random bytes, never all zero (an all-zero id is invalid).
fn random_hex(bytes: usize) -> String {
    loop {
        let id = uuid::Uuid::new_v4();
        let raw = &id.as_bytes()[..bytes];
        if raw.iter().any(|b| *b != 0) {
            return raw.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
}

/// Runs `future` with an empty slot for the context of the request it serves.
pub async fn scope<F: Future>(future: F) -> F::Output {
    CURRENT.scope(RefCell::new(None), future).await
}

/// Records the context of the request being handled. Outside a [`scope`] it does nothing.
pub fn set(context: TraceContext) {
    let _ = CURRENT.try_with(|slot| *slot.borrow_mut() = Some(context));
}

/// The `traceparent` value for an outbound call of this request, or `None` outside a traced request.
pub fn outgoing() -> Option<String> {
    CURRENT.try_with(|slot| slot.borrow().as_ref().map(TraceContext::outgoing)).ok().flatten()
}

/// Adds the `traceparent` of the current request to an outbound call.
pub fn propagate(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match outgoing() {
        Some(value) => request.header("traceparent", value),
        None => request,
    }
}
