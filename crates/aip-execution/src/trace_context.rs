//! W3C Trace Context (`traceparent`) for the HTTP calls the server makes while it handles one request.
//!
//! The transport parses the inbound header, keeps its trace id in the request log line, and stores the context here
//! for the task that handles the request. An outbound HTTP call made by that same task (an embedding or answer
//! provider, an object store) adds a `traceparent` with the same trace id, the sampled flag of the caller and a fresh
//! span id, so the caller's tracing backend and the provider's logs name one trace.
//!
//! This module only carries the context. Spans are recorded by the transport's optional OTLP exporter. A task spawned from the
//! request does not inherit the context by itself; a durable job gets it through the `trace_id` text column of its row, which
//! holds `<trace id>` or `<trace id>-<span id>` ([`TraceContext::job_value`]): the trace of the enqueue request and the span
//! that enqueued it, so the job's own span can name that span as its parent.
use std::{cell::RefCell, future::Future};

tokio::task_local! {
    static CURRENT: RefCell<Option<TraceContext>>;
}

/// The trace a request belongs to: the 32-hex trace id and the trace flags of the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: String,
    parent_span_id: Option<String>,
    /// The span this server records for the work it is doing (a request or a job run), once one is assigned.
    span_id: Option<String>,
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
        Some(Self {
            trace_id: trace_id.to_owned(),
            parent_span_id: Some(parent_id.to_owned()),
            span_id: None,
            flags: u8::from_str_radix(flags, 16).ok()?,
        })
    }

    /// A trace that starts at this server: a random trace id, not sampled.
    pub fn generate() -> Self {
        Self { trace_id: random_hex(16), parent_span_id: None, span_id: None, flags: 0 }
    }

    /// Creates a trace context from an existing trace id (e.g. preserved on a durable job).
    pub fn from_trace_id(trace_id: impl Into<String>) -> Self {
        Self { trace_id: trace_id.into(), parent_span_id: None, span_id: None, flags: 0 }
    }

    /// Rebuilds the context of a job from what [`job_value`](Self::job_value) stored. A bare 32-hex trace id (rows written before spans
    /// were stored) has no parent span. Anything else is not trusted as a trace id: a new trace starts, so a corrupt value cannot
    /// reach an exporter or an outbound header.
    pub fn from_job_value(value: &str) -> Self {
        let lower_hex = |s: &str, len: usize| {
            s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && s.bytes().any(|b| b != b'0')
        };
        match value.split_once('-') {
            None if lower_hex(value, 32) => Self::from_trace_id(value),
            Some((trace, span)) if lower_hex(trace, 32) && lower_hex(span, 16) => {
                Self { trace_id: trace.to_owned(), parent_span_id: Some(span.to_owned()), span_id: None, flags: 0 }
            }
            _ => Self::generate(),
        }
    }

    /// What a durable job row stores so the run can join this trace: the trace id, and the span that is enqueueing it when there is one.
    pub fn job_value(&self) -> String {
        match &self.span_id {
            Some(span) => format!("{}-{span}", self.trace_id),
            None => self.trace_id.clone(),
        }
    }

    /// The same trace with `span_id` as the span of the work now starting.
    pub fn with_span_id(mut self, span_id: impl Into<String>) -> Self {
        self.span_id = Some(span_id.into());
        self
    }

    /// A nested execution in this trace. Preserve flags and name the active span as its parent.
    pub fn child(&self, span_id: impl Into<String>) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            parent_span_id: self.span_id.clone().or_else(|| self.parent_span_id.clone()),
            span_id: Some(span_id.into()),
            flags: self.flags,
        }
    }

    pub fn span_id(&self) -> Option<&str> {
        self.span_id.as_deref()
    }

    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    pub fn parent_span_id(&self) -> Option<&str> {
        self.parent_span_id.as_deref()
    }

    pub fn flags(&self) -> u8 {
        self.flags
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

/// Returns a clone of the current request's trace context, if set.
pub fn current() -> Option<TraceContext> {
    CURRENT.try_with(|slot| slot.borrow().clone()).ok().flatten()
}

/// Returns the current trace id, if set.
pub fn current_trace_id() -> Option<String> {
    CURRENT.try_with(|slot| slot.borrow().as_ref().map(|c| c.trace_id().to_string())).ok().flatten()
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

#[cfg(test)]
mod job_value_tests {
    use super::TraceContext;

    const TRACE: &str = "0af7651916cd43dd8448eb211c80319c";
    const SPAN: &str = "b7ad6b7169203331";

    #[test]
    fn a_job_value_round_trips_the_trace_and_the_enqueuing_span() {
        let request = TraceContext::parse(&format!("00-{TRACE}-00f067aa0ba902b7-01")).expect("valid").with_span_id(SPAN);
        let stored = request.job_value();
        assert_eq!(stored, format!("{TRACE}-{SPAN}"));
        let run = TraceContext::from_job_value(&stored);
        assert_eq!((run.trace_id(), run.parent_span_id(), run.span_id()), (TRACE, Some(SPAN), None));
    }

    #[test]
    fn rows_written_before_spans_were_stored_still_join_their_trace() {
        let run = TraceContext::from_job_value(TRACE);
        assert_eq!((run.trace_id(), run.parent_span_id()), (TRACE, None));
        assert_eq!(TraceContext::from_trace_id(TRACE).job_value(), TRACE, "no span, no suffix");
    }

    #[test]
    fn a_corrupt_stored_value_starts_a_new_trace_instead_of_reaching_an_exporter() {
        for bad in [
            "",
            "not-a-trace",
            "0af7651916cd43dd8448eb211c80319",
            &format!("{TRACE}-short"),
            &format!("{TRACE}-{SPAN}-extra"),
            &"0".repeat(32),
            &format!("{}-{SPAN}", TRACE.to_uppercase()),
        ] {
            let run = TraceContext::from_job_value(bad);
            assert_eq!(run.trace_id().len(), 32, "{bad}");
            assert_ne!(run.trace_id(), TRACE, "{bad}");
            assert_eq!(run.parent_span_id(), None, "{bad}");
        }
    }
}

#[cfg(test)]
mod nested_span_tests {
    use super::TraceContext;
    #[test]
    fn nested_span_uses_the_active_span_as_parent_and_preserves_sampling() {
        let request = TraceContext::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap().with_span_id("1111111111111111");
        let child = request.child("2222222222222222");
        assert_eq!(child.trace_id(), request.trace_id());
        assert_eq!(child.parent_span_id(), Some("1111111111111111"));
        assert_eq!(child.span_id(), Some("2222222222222222"));
        assert_eq!(child.flags(), 1);
        let grandchild = child.child("3333333333333333");
        assert_eq!(grandchild.parent_span_id(), child.span_id());
        assert_eq!(request.span_id(), Some("1111111111111111"));
        assert_eq!(request.parent_span_id(), Some("00f067aa0ba902b7"));
    }
}
