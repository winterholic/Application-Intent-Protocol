//! A request that carries a W3C `traceparent` keeps its trace id on the outbound HTTP calls the server makes while
//! it handles that request, so the caller's tracing backend and the provider's logs name the same trace.
use aip_execution::rag::{EmbeddingProvider, OpenAiCompatibleEmbedder};
use aip_execution::trace_context::{self, TraceContext};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

const INBOUND: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// A one-shot provider that returns its request headers.
fn provider() -> (OpenAiCompatibleEmbedder, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!("http://{}/v1/test", listener.local_addr().expect("address"));
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("timeout");
        let mut bytes = Vec::new();
        let mut byte = [0; 1];
        while !bytes.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).expect("header byte");
            bytes.push(byte[0]);
        }
        let headers = String::from_utf8(bytes).expect("utf-8 headers");
        let length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().expect("length"))
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).expect("body");
        let reply = r#"{"data":[{"index":0,"embedding":[0.5]}]}"#;
        let raw = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        stream.write_all(raw.as_bytes()).expect("reply");
        headers
    });
    (OpenAiCompatibleEmbedder::new(&endpoint, "m", "server-secret", Duration::from_secs(2), 4).expect("embedder"), worker)
}

fn traceparent_of(headers: &str) -> Option<String> {
    headers.lines().find_map(|line| line.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("traceparent")).map(|(_, v)| v.trim().to_owned()))
}

#[tokio::test]
async fn an_outbound_call_inside_a_traced_request_carries_the_same_trace_id_with_a_new_span() {
    let (embedder, worker) = provider();
    trace_context::scope(async {
        trace_context::set(TraceContext::parse(INBOUND).expect("a valid header"));
        embedder.embed(&["hello".into()]).await.expect("embedding");
    })
    .await;
    let sent = traceparent_of(&worker.join().expect("provider thread")).expect("the call carries traceparent");
    let parts: Vec<&str> = sent.split('-').collect();
    assert_eq!((parts[0], parts[1], parts[3]), ("00", "0af7651916cd43dd8448eb211c80319c", "01"), "{sent}");
    assert!(parts[2].len() == 16 && parts[2] != "b7ad6b7169203331" && parts[2] != "0000000000000000", "a fresh span id: {sent}");
}

#[tokio::test]
async fn a_call_outside_any_request_carries_no_trace_header() {
    let (embedder, worker) = provider();
    embedder.embed(&["hello".into()]).await.expect("embedding");
    assert_eq!(traceparent_of(&worker.join().expect("provider thread")), None);
}

#[test]
fn only_a_well_formed_traceparent_is_accepted() {
    for good in [INBOUND, "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00", "01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01-extra"]
    {
        assert!(TraceContext::parse(good).is_some(), "{good}");
    }
    for bad in [
        "",
        "ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        "00-00000000000000000000000000000000-b7ad6b7169203331-01",
        "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01",
        "00-0AF7651916CD43DD8448EB211C80319C-b7ad6b7169203331-01",
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-1",
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01-extra",
        "0af7651916cd43dd8448eb211c80319c",
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-0g",
    ] {
        assert!(TraceContext::parse(bad).is_none(), "{bad}");
    }
}

#[tokio::test]
async fn the_context_belongs_to_one_scope() {
    trace_context::scope(async {
        assert!(trace_context::outgoing().is_none());
        trace_context::set(TraceContext::parse(INBOUND).expect("a valid header"));
        let first = trace_context::outgoing().expect("inside the scope");
        let second = trace_context::outgoing().expect("inside the scope");
        assert_ne!(first, second, "every call is its own span");
    })
    .await;
    assert!(trace_context::outgoing().is_none(), "nothing leaks out of the scope");
    // A task spawned from a request does not inherit the request's trace; a job has to carry its own.
    trace_context::scope(async {
        trace_context::set(TraceContext::parse(INBOUND).expect("a valid header"));
        let detached = tokio::spawn(async { trace_context::outgoing() }).await.expect("task");
        assert!(detached.is_none());
    })
    .await;
}
