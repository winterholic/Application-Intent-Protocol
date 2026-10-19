//! OpenTelemetry Protocol (OTLP/HTTP) exporter integration tests.
//! Verifies trace span propagation, durable job trace linking, metrics export,
//! and that sensitive parameters are never recorded in span attributes.

use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    otlp::OtlpConfig,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

struct NoSessions;
impl Authenticator for NoSessions {
    fn verify_session<'a>(&'a self, _: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
    fn verify_durable<'a>(
        &'a self,
        _: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
}

/// A minimal in-process mock OTLP/HTTP collector.
struct MockCollector {
    addr: SocketAddr,
    received_traces: Arc<Mutex<Vec<Value>>>,
    received_metrics: Arc<Mutex<Vec<Value>>>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

async fn collector_request(socket: &mut TcpStream) -> Option<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut request = Vec::new();
        loop {
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let head = std::str::from_utf8(&request[..end]).ok()?;
                let length: usize = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))?
                    .1
                    .trim()
                    .parse()
                    .ok()?;
                let expected = (end + 4).checked_add(length)?;
                if expected > 256 * 1024 {
                    return None;
                }
                if request.len() >= expected {
                    request.truncate(expected);
                    return Some(request);
                }
            }
            if request.len() >= 256 * 1024 {
                return None;
            }
            let mut chunk = [0; 4096];
            let read = socket.read(&mut chunk).await.ok()?;
            if read == 0 {
                return None;
            }
            request.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .ok()
    .flatten()
}

impl MockCollector {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let received_traces = Arc::new(Mutex::new(Vec::new()));
        let received_metrics = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let traces_store = received_traces.clone();
        let metrics_store = received_metrics.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    conn = listener.accept() => {
                        let Ok((mut socket, _)) = conn else { break };
                        let traces = traces_store.clone();
                        let metrics = metrics_store.clone();
                        tokio::spawn(async move {
                            let Some(buf) = collector_request(&mut socket).await else { return };
                            let req = String::from_utf8_lossy(&buf);
                            let (head, body) = match req.split_once("\r\n\r\n") {
                                Some(pair) => pair,
                                None => return,
                            };
                            let first_line = head.lines().next().unwrap_or("");
                            if let Ok(val) = serde_json::from_str::<Value>(body) {
                                if first_line.contains("/v1/traces") {
                                    traces.lock().unwrap().push(val);
                                } else if first_line.contains("/v1/metrics") {
                                    metrics.lock().unwrap().push(val);
                                }
                            }
                            let resp = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
                            let _ = socket.write_all(resp.as_bytes()).await;
                        });
                    }
                }
            }
        });

        Self { addr, received_traces, received_metrics, _shutdown: shutdown_tx }
    }
}

async fn post_request(addr: SocketAddr, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut req = format!("POST {path} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);

    stream.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn otlp_exporter_receives_traces_and_metrics_with_correlated_trace_id() {
    let collector = MockCollector::start().await;

    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    let mut otlp_headers = HashMap::new();
    otlp_headers.insert("x-test-token".into(), "secret-collector-token".into());

    let otlp_config = OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: otlp_headers, timeout_ms: 3000 };

    let infrastructure = Infrastructure { otlp: Some(otlp_config), ..Infrastructure::default() };

    let server = listen_with_infrastructure(
        facts,
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(NoSessions),
        None,
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();

    let server_addr = server.address;

    // Send HTTP request with inbound traceparent
    let client_trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let client_span_id = "00f067aa0ba902b7";
    let traceparent = format!("00-{client_trace_id}-{client_span_id}-01");

    let resp = post_request(
        server_addr,
        "/read?sensitive_token=do_not_leak",
        &[("traceparent", &traceparent), ("authorization", "Bearer client-secret-token")],
        "{\"query\":{}}",
    )
    .await;
    assert!(resp.contains("x-request-id: req_"), "{resp}");

    // Wait up to 3 seconds for trace batch flush (flush interval is 500ms)
    let started = std::time::Instant::now();
    let mut found_span: Option<Value> = None;
    while started.elapsed() < Duration::from_secs(3) {
        {
            let traces = collector.received_traces.lock().unwrap();
            for batch in traces.iter() {
                if let Some(r_spans) = batch.get("resourceSpans").and_then(Value::as_array) {
                    for r_span in r_spans {
                        if let Some(s_spans) = r_span.get("scopeSpans").and_then(Value::as_array) {
                            for s_span in s_spans {
                                if let Some(spans) = s_span.get("spans").and_then(Value::as_array) {
                                    for span in spans {
                                        if span.get("traceId").and_then(Value::as_str) == Some(client_trace_id) {
                                            found_span = Some(span.clone());
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if found_span.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let span = found_span.expect("OTLP span with matching trace_id must be received by collector");
    assert_eq!(span["traceId"].as_str(), Some(client_trace_id));
    assert_eq!(span["parentSpanId"].as_str(), Some(client_span_id));
    assert_eq!(span["name"].as_str(), Some("HTTP read"));

    // Check that sensitive query/body parameters and auth tokens are NOT in attributes
    let span_str = span.to_string();
    assert!(!span_str.contains("do_not_leak"), "Sensitive query params must never leak to OTLP attributes: {span_str}");
    assert!(!span_str.contains("client-secret-token"), "Auth tokens must never leak to OTLP attributes: {span_str}");

    // Wait for metrics export (interval is 2s)
    let metric_started = std::time::Instant::now();
    let mut found_metrics = false;
    while metric_started.elapsed() < Duration::from_secs(4) {
        {
            let metrics = collector.received_metrics.lock().unwrap();
            if !metrics.is_empty() {
                for batch in metrics.iter() {
                    let m_str = batch.to_string();
                    if m_str.contains("aip_requests_total") || m_str.contains("aip_request_duration_seconds") {
                        found_metrics = true;
                        break;
                    }
                }
            }
        }
        if found_metrics {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    assert!(found_metrics, "OTLP metrics must be received by collector");

    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_trace_status_mapping_conforms_to_otlp_spec() {
    let collector = MockCollector::start().await;
    let otlp_config = OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: HashMap::new(), timeout_ms: 3000 };
    let exporter = spike_v6_transport::otlp::OtlpExporter::start(otlp_config).unwrap();

    // 1. Success HTTP request (200 OK) -> must map to OTLP status 1 (STATUS_CODE_OK)
    let ok_trace_id = "00000000000000000000000000000001";
    {
        let mut trace = spike_v6_transport::trace::Trace::default();
        trace.set_otlp_exporter(Some(exporter.clone()));
        trace.accept_traceparent(&format!("00-{ok_trace_id}-0000000000000001-01"));
        trace.begin("GET", "/test-ok");
        trace.record("200 OK", "{\"ok\":true}");
    } // drop emits span

    // 2. Client error HTTP request (400 Bad Request) -> must map to OTLP status 2 (STATUS_CODE_ERROR)
    let err400_trace_id = "00000000000000000000000000000002";
    {
        let mut trace = spike_v6_transport::trace::Trace::default();
        trace.set_otlp_exporter(Some(exporter.clone()));
        trace.accept_traceparent(&format!("00-{err400_trace_id}-0000000000000002-01"));
        trace.begin("POST", "/test-400");
        trace.record("400 Bad Request", "{\"ok\":false,\"code\":\"WIRE.BAD_REQUEST\"}");
    }

    // 3. Server error HTTP request (500 Internal Error) -> must map to OTLP status 2 (STATUS_CODE_ERROR)
    let err500_trace_id = "00000000000000000000000000000003";
    {
        let mut trace = spike_v6_transport::trace::Trace::default();
        trace.set_otlp_exporter(Some(exporter.clone()));
        trace.accept_traceparent(&format!("00-{err500_trace_id}-0000000000000003-01"));
        trace.begin("POST", "/test-500");
        trace.record("500 Internal Server Error", "{\"ok\":false,\"code\":\"WIRE.INTERNAL\"}");
    }

    // 4. Incomplete request with no status line recorded -> must map to OTLP status 0 (STATUS_CODE_UNSET)
    let unset_trace_id = "00000000000000000000000000000004";
    {
        let mut trace = spike_v6_transport::trace::Trace::default();
        trace.set_otlp_exporter(Some(exporter.clone()));
        trace.accept_traceparent(&format!("00-{unset_trace_id}-0000000000000004-01"));
        trace.begin("GET", "/test-unset");
    }

    // Wait for collector to receive all 4 spans
    let started = std::time::Instant::now();
    let mut spans = HashMap::new();
    while started.elapsed() < Duration::from_secs(3) {
        {
            let traces = collector.received_traces.lock().unwrap();
            for batch in traces.iter() {
                if let Some(r_spans) = batch.get("resourceSpans").and_then(Value::as_array) {
                    for r_span in r_spans {
                        if let Some(s_spans) = r_span.get("scopeSpans").and_then(Value::as_array) {
                            for s_span in s_spans {
                                if let Some(batch_spans) = s_span.get("spans").and_then(Value::as_array) {
                                    for span in batch_spans {
                                        if let Some(tid) = span.get("traceId").and_then(Value::as_str) {
                                            spans.insert(tid.to_string(), span.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if spans.len() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(spans.len(), 4, "Must receive all 4 spans from the 4 HTTP branches");

    // Official OTLP trace.proto StatusCode values:
    // STATUS_CODE_UNSET = 0
    // STATUS_CODE_OK = 1
    // STATUS_CODE_ERROR = 2
    let ok_span = &spans[ok_trace_id];
    let err400_span = &spans[err400_trace_id];
    let err500_span = &spans[err500_trace_id];
    let unset_span = &spans[unset_trace_id];

    assert_eq!(ok_span["status"]["code"].as_i64(), Some(1), "HTTP 200 OK must map to OTLP STATUS_CODE_OK (1), got {:?}", ok_span["status"]["code"]);
    assert_ne!(ok_span["status"]["code"].as_i64(), Some(2), "HTTP 200 OK must NEVER map to 2 (which is STATUS_CODE_ERROR in OTLP specification)");

    assert_eq!(
        err400_span["status"]["code"].as_i64(),
        Some(2),
        "HTTP 400 Bad Request must map to OTLP STATUS_CODE_ERROR (2), got {:?}",
        err400_span["status"]["code"]
    );
    assert_ne!(
        err400_span["status"]["code"].as_i64(),
        Some(3),
        "HTTP 400 Bad Request must NEVER map to 3 (which is an uninterpreted/invalid enum in OTLP)"
    );

    assert_eq!(
        err500_span["status"]["code"].as_i64(),
        Some(2),
        "HTTP 500 Internal Error must map to OTLP STATUS_CODE_ERROR (2), got {:?}",
        err500_span["status"]["code"]
    );

    assert_eq!(
        unset_span["status"]["code"].as_i64(),
        Some(0),
        "Unset HTTP trace must map to OTLP STATUS_CODE_UNSET (0), got {:?}",
        unset_span["status"]["code"]
    );
    assert_ne!(
        unset_span["status"]["code"].as_i64(),
        Some(1),
        "Unset HTTP trace must NEVER map to 1 (which is STATUS_CODE_OK in OTLP specification)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_job_status_mapping_conforms_to_otlp_spec() {
    let collector = MockCollector::start().await;
    let otlp_config = OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: HashMap::new(), timeout_ms: 3000 };
    let exporter = spike_v6_transport::otlp::OtlpExporter::start(otlp_config).unwrap();
    let now = spike_v6_transport::otlp::now_nanos();

    // 1. Terminal success: "SUCCEEDED" -> must map to OTLP status 1 (STATUS_CODE_OK)
    let succeeded_trace_id = "00000000000000000000000000000011";
    exporter.export_span(spike_v6_transport::otlp::SpanData {
        trace_id: succeeded_trace_id.to_string(),
        span_id: spike_v6_transport::otlp::random_span_id(),
        parent_span_id: None,
        name: "JOB sync_data".to_string(),
        kind: 1, // INTERNAL
        start_time_nanos: now - 30_000_000,
        end_time_nanos: now - 10_000_000,
        attributes: vec![
            ("job.operation".to_string(), serde_json::Value::String("sync_data".into())),
            ("job.state".to_string(), serde_json::Value::String("SUCCEEDED".into())),
        ],
        status_code: spike_v6_transport::otlp::SpanStatusCode::from_job_state("SUCCEEDED"),
        status_message: None,
    });

    // 2. Terminal failure: "FAILED" -> must map to OTLP status 2 (STATUS_CODE_ERROR)
    let failed_trace_id = "00000000000000000000000000000012";
    exporter.export_span(spike_v6_transport::otlp::SpanData {
        trace_id: failed_trace_id.to_string(),
        span_id: spike_v6_transport::otlp::random_span_id(),
        parent_span_id: None,
        name: "JOB process_order".to_string(),
        kind: 1, // INTERNAL
        start_time_nanos: now - 30_000_000,
        end_time_nanos: now - 10_000_000,
        attributes: vec![
            ("job.operation".to_string(), serde_json::Value::String("process_order".into())),
            ("job.state".to_string(), serde_json::Value::String("FAILED".into())),
            ("aip.code".to_string(), serde_json::Value::String("WIRE.PAYMENT_REJECTED".into())),
        ],
        status_code: spike_v6_transport::otlp::SpanStatusCode::from_job_state("FAILED"),
        status_message: Some("WIRE.PAYMENT_REJECTED".into()),
    });

    // 3. Terminal cancellation: "CANCELLED" -> must map to OTLP status 2 (STATUS_CODE_ERROR)
    let cancelled_trace_id = "00000000000000000000000000000013";
    exporter.export_span(spike_v6_transport::otlp::SpanData {
        trace_id: cancelled_trace_id.to_string(),
        span_id: spike_v6_transport::otlp::random_span_id(),
        parent_span_id: None,
        name: "JOB batch_export".to_string(),
        kind: 1, // INTERNAL
        start_time_nanos: now - 30_000_000,
        end_time_nanos: now - 10_000_000,
        attributes: vec![
            ("job.operation".to_string(), serde_json::Value::String("batch_export".into())),
            ("job.state".to_string(), serde_json::Value::String("CANCELLED".into())),
        ],
        status_code: spike_v6_transport::otlp::SpanStatusCode::from_job_state("CANCELLED"),
        status_message: None,
    });

    // 4. Transient retry: "RETRY" -> must map to OTLP status 0 (STATUS_CODE_UNSET)
    let retry_trace_id = "00000000000000000000000000000014";
    exporter.export_span(spike_v6_transport::otlp::SpanData {
        trace_id: retry_trace_id.to_string(),
        span_id: spike_v6_transport::otlp::random_span_id(),
        parent_span_id: None,
        name: "JOB poll_upstream".to_string(),
        kind: 1, // INTERNAL
        start_time_nanos: now - 30_000_000,
        end_time_nanos: now - 10_000_000,
        attributes: vec![
            ("job.operation".to_string(), serde_json::Value::String("poll_upstream".into())),
            ("job.state".to_string(), serde_json::Value::String("RETRY".into())),
        ],
        status_code: spike_v6_transport::otlp::SpanStatusCode::from_job_state("RETRY"),
        status_message: None,
    });

    // Wait for collector to receive all 4 spans
    let started = std::time::Instant::now();
    let mut spans = HashMap::new();
    while started.elapsed() < Duration::from_secs(3) {
        {
            let traces = collector.received_traces.lock().unwrap();
            for batch in traces.iter() {
                if let Some(r_spans) = batch.get("resourceSpans").and_then(Value::as_array) {
                    for r_span in r_spans {
                        if let Some(s_spans) = r_span.get("scopeSpans").and_then(Value::as_array) {
                            for s_span in s_spans {
                                if let Some(batch_spans) = s_span.get("spans").and_then(Value::as_array) {
                                    for span in batch_spans {
                                        if let Some(tid) = span.get("traceId").and_then(Value::as_str) {
                                            spans.insert(tid.to_string(), span.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if spans.len() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(spans.len(), 4, "Must receive all 4 job spans");

    let succ_span = &spans[succeeded_trace_id];
    let fail_span = &spans[failed_trace_id];
    let cancel_span = &spans[cancelled_trace_id];
    let retry_span = &spans[retry_trace_id];

    assert_eq!(
        succ_span["status"]["code"].as_i64(),
        Some(1),
        "Job state SUCCEEDED must map to OTLP STATUS_CODE_OK (1), got {:?}",
        succ_span["status"]["code"]
    );
    assert_ne!(succ_span["status"]["code"].as_i64(), Some(2), "Job state SUCCEEDED must NEVER map to 2 (STATUS_CODE_ERROR in OTLP)");

    assert_eq!(
        fail_span["status"]["code"].as_i64(),
        Some(2),
        "Job state FAILED must map to OTLP STATUS_CODE_ERROR (2), got {:?}",
        fail_span["status"]["code"]
    );
    assert_ne!(fail_span["status"]["code"].as_i64(), Some(3), "Job state FAILED must NEVER map to 3 (uninterpreted/invalid enum in OTLP)");

    assert_eq!(
        cancel_span["status"]["code"].as_i64(),
        Some(2),
        "Job state CANCELLED must map to OTLP STATUS_CODE_ERROR (2), got {:?}",
        cancel_span["status"]["code"]
    );
    assert_ne!(cancel_span["status"]["code"].as_i64(), Some(3), "Job state CANCELLED must NEVER map to 3 (uninterpreted/invalid enum in OTLP)");

    assert_eq!(
        retry_span["status"]["code"].as_i64(),
        Some(0),
        "Job state RETRY must map to OTLP STATUS_CODE_UNSET (0), got {:?}",
        retry_span["status"]["code"]
    );
    assert_ne!(retry_span["status"]["code"].as_i64(), Some(1), "Job state RETRY must NEVER map to 1 (STATUS_CODE_OK in OTLP)");
}

#[tokio::test(flavor = "current_thread")]
async fn otlp_exporter_queue_saturation_drops_spans_without_blocking() {
    let unreachable_addr: SocketAddr = "127.0.0.1:19999".parse().unwrap();
    let otlp_config = OtlpConfig { endpoint: format!("http://{unreachable_addr}"), headers: HashMap::new(), timeout_ms: 100 };
    let exporter = spike_v6_transport::otlp::OtlpExporter::start(otlp_config).unwrap();
    let now = spike_v6_transport::otlp::now_nanos();
    let dropped_before = spike_v6_transport::otlp::dropped_span_count();

    // Export 5000 spans (exceeding queue capacity of 4096)
    let start = std::time::Instant::now();
    for i in 0..5000 {
        exporter.export_span(spike_v6_transport::otlp::SpanData {
            trace_id: format!("{:032x}", i + 1),
            span_id: spike_v6_transport::otlp::random_span_id(),
            parent_span_id: None,
            name: format!("SPAN_{i}"),
            kind: 1,
            start_time_nanos: now,
            end_time_nanos: now + 1_000_000,
            attributes: vec![],
            status_code: spike_v6_transport::otlp::SpanStatusCode::Ok,
            status_message: None,
        });
    }
    let elapsed = start.elapsed();
    assert!(spike_v6_transport::otlp::dropped_span_count() >= dropped_before + 5000 - 4096);
    assert!(elapsed < Duration::from_millis(100), "Enqueuing 5000 spans must complete non-blockingly under 100ms, took {elapsed:?}");
}

// ---- the real path: HTTP enqueue -> job row -> worker run -> spans from the server's own exporter ----

#[tokio::test(flavor = "current_thread")]
async fn queue_drop_counter_reaches_the_otlp_collector() {
    let collector = MockCollector::start().await;
    let before = spike_v6_transport::otlp::dropped_span_count();
    let exporter = spike_v6_transport::otlp::OtlpExporter::start(OtlpConfig {
        endpoint: format!("http://{}", collector.addr),
        headers: HashMap::new(),
        timeout_ms: 1000,
    })
    .unwrap();
    for i in 0..5000 {
        exporter.export_span(spike_v6_transport::otlp::SpanData {
            trace_id: format!("{:032x}", i + 1),
            span_id: format!("{:016x}", i + 1),
            parent_span_id: None,
            name: "queue-counter".into(),
            kind: 1,
            start_time_nanos: 1,
            end_time_nanos: 2,
            attributes: vec![],
            status_code: spike_v6_transport::otlp::SpanStatusCode::Ok,
            status_message: None,
        });
    }
    let start = std::time::Instant::now();
    loop {
        let found = collector.received_metrics.lock().unwrap().iter().any(|batch| {
            batch["resourceMetrics"].as_array().into_iter().flatten().any(|resource| {
                resource["scopeMetrics"].as_array().into_iter().flatten().any(|scope| {
                    scope["metrics"].as_array().into_iter().flatten().any(|metric| {
                        metric["name"] == "aip_otlp_spans_dropped_total"
                            && metric["sum"]["isMonotonic"] == true
                            && metric["sum"]["aggregationTemporality"] == 2
                            && metric["sum"]["dataPoints"][0]["asInt"]
                                .as_str()
                                .and_then(|n| n.parse::<u64>().ok())
                                .is_some_and(|n| n >= before + 5000 - 4096)
                    })
                })
            })
        });
        if found {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "collector never received the cumulative queue-drop counter");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// sqlgen keeps one schema per process: the two tests below share its name and run one after the other.
static SCHEMA_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct DbAuth {
    schema: String,
    iat: i64,
}

impl Authenticator for DbAuth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let grant = DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: 1, issued_at: self.iat, min_iat: 0 };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.iat + 600)
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

struct DropSchema(String);

impl Drop for DropSchema {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::Builder::new().name("aip-otlp-test-schema-cleanup".into()).spawn(move || {
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

async fn call(addr: SocketAddr, path: &str, fingerprint: &str, extra: &[(&str, &str)], body: Value) -> Value {
    let mut headers = vec![("authorization", "Bearer owner"), ("x-aip-contract", fingerprint), ("content-type", "application/json")];
    headers.extend_from_slice(extra);
    let response = post_request(addr, path, &headers, &body.to_string()).await;
    serde_json::from_str(response.split_once("\r\n\r\n").expect("HTTP response").1).expect("JSON body")
}

fn spans_of(collector: &MockCollector, trace_id: &str) -> Vec<Value> {
    let traces = collector.received_traces.lock().unwrap();
    let mut spans = Vec::new();
    for batch in traces.iter() {
        for resource in batch["resourceSpans"].as_array().into_iter().flatten() {
            for scope in resource["scopeSpans"].as_array().into_iter().flatten() {
                for span in scope["spans"].as_array().into_iter().flatten() {
                    if span["traceId"].as_str() == Some(trace_id) {
                        spans.push(span.clone());
                    }
                }
            }
        }
    }
    spans
}

/// An enqueue request with a `traceparent` produces an HTTP span under the caller's span, stores trace and span in the job row, and the
/// job that a worker then runs is exported by the server as a child of that HTTP span, in the same trace. Nothing here is exported by the test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_enqueued_over_http_is_exported_as_a_child_of_the_request_span() {
    let collector = MockCollector::start().await;
    let _gate = SCHEMA_GATE.lock().await;
    let schema = format!("aip_otlp_job_{}", std::process::id());
    let _cleanup = DropSchema(schema.clone());
    sqlgen::set_schema(&schema);
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init(aip_test_support::test_database_url!(), &schema, &facts).await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat: i64 = db.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
    let infrastructure = Infrastructure {
        otlp: Some(OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: HashMap::new(), timeout_ms: 3000 }),
        ..Infrastructure::default()
    };
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        Arc::new(DbAuth { schema: schema.clone(), iat }),
        Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();

    let (trace_id, caller_span) = ("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7");
    let traceparent = format!("00-{trace_id}-{caller_span}-01");
    let enqueued = call(
        server.address,
        "/jobs/enqueue",
        &fingerprint,
        &[("traceparent", &traceparent)],
        json!({"operation":"count","input":{"text":"가😀A"},"key":"trace-e2e","delayMs":0}),
    )
    .await;
    assert_eq!(enqueued["ok"], true, "{enqueued}");
    let job_id = enqueued["run"]["id"].as_str().expect("job id").to_owned();
    for _ in 0..200 {
        let status = call(server.address, "/jobs/status", &fingerprint, &[], json!({"id": job_id})).await;
        if status["run"]["state"] == "SUCCEEDED" {
            break;
        }
        assert_ne!(status["run"]["state"], "FAILED", "{status}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The exporter flushes in batches; wait until the HTTP span of the enqueue and the run's span have both arrived.
    let started = std::time::Instant::now();
    let mut spans = Vec::new();
    while started.elapsed() < Duration::from_secs(10) {
        spans = spans_of(&collector, trace_id);
        let jobs = spans.iter().filter(|span| span["name"].as_str().is_some_and(|name| name.starts_with("JOB "))).count();
        if spans.iter().any(|span| span["name"] == "HTTP jobs") && jobs >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let names: Vec<_> = spans.iter().map(|span| span["name"].as_str().unwrap_or("?")).collect();
    let enqueue_span = spans
        .iter()
        .find(|span| span["name"] == "HTTP jobs" && span["parentSpanId"].as_str() == Some(caller_span))
        .unwrap_or_else(|| panic!("the enqueue request span under the caller's span is missing: {names:?}"));
    let enqueue_span_id = enqueue_span["spanId"].as_str().expect("span id");
    assert_eq!(enqueue_span_id.len(), 16);

    let run_span = spans.iter().find(|span| span["name"] == "JOB count").unwrap_or_else(|| panic!("the job's own span is missing: {names:?}"));
    assert_eq!(run_span["traceId"], trace_id);
    assert_eq!(run_span["parentSpanId"].as_str(), Some(enqueue_span_id), "the job run must be a child of the request that enqueued it");
    assert_eq!(run_span["status"]["code"].as_i64(), Some(1));
    assert!(run_span["attributes"].as_array().unwrap().iter().any(|a| a["key"] == "job.id" && a["value"]["stringValue"] == job_id));

    // What was stored in the row is the trace and the enqueuing span, nothing else.
    let stored: String = db.query_one(&format!("SELECT trace_id FROM {schema}.aip_jobs WHERE id::text=$1"), &[&job_id]).await.unwrap().get(0);
    assert_eq!(stored, format!("{trace_id}-{enqueue_span_id}"));
}

/// A request without a `traceparent` starts a trace at the server and the job still joins it, with a parent span of its own request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_without_an_inbound_trace_joins_the_trace_the_server_started() {
    let collector = MockCollector::start().await;
    let _gate = SCHEMA_GATE.lock().await;
    let schema = format!("aip_otlp_job_{}", std::process::id());
    let _cleanup = DropSchema(schema.clone());
    sqlgen::set_schema(&schema);
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init(aip_test_support::test_database_url!(), &schema, &facts).await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat: i64 = db.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
    let infrastructure = Infrastructure {
        otlp: Some(OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: HashMap::new(), timeout_ms: 3000 }),
        ..Infrastructure::default()
    };
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        Arc::new(DbAuth { schema: schema.clone(), iat }),
        Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();
    let enqueued =
        call(server.address, "/jobs/enqueue", &fingerprint, &[], json!({"operation":"count","input":{"text":"abc"},"key":"no-trace","delayMs":0}))
            .await;
    let job_id = enqueued["run"]["id"].as_str().expect("job id").to_owned();
    let stored: String = db.query_one(&format!("SELECT trace_id FROM {schema}.aip_jobs WHERE id::text=$1"), &[&job_id]).await.unwrap().get(0);
    let (trace_id, request_span) = stored.split_once('-').expect("trace and span are stored");
    assert_eq!((trace_id.len(), request_span.len()), (32, 16), "{stored}");
    let started = std::time::Instant::now();
    loop {
        let spans = spans_of(&collector, trace_id);
        if let Some(run) = spans.iter().find(|span| span["name"] == "JOB count") {
            assert_eq!(run["parentSpanId"].as_str(), Some(request_span));
            let request = spans.iter().find(|span| span["name"] == "HTTP jobs").expect("the request span of the same trace");
            assert_eq!(request["spanId"].as_str(), Some(request_span));
            assert!(
                request.get("parentSpanId").is_none_or(|parent| parent.as_str().is_none_or(str::is_empty)),
                "the trace started here, so the request has no parent"
            );
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the job span never arrived: {spans:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// ctx.jobs.run executes nested steps in the root lease; each step must have its own span and its actual caller as parent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_node_and_python_job_steps_form_a_span_tree_without_new_durable_runs() {
    let _gate = SCHEMA_GATE.lock().await;
    for (label, lang, trace_id) in
        [("node", WorkerLang::Node, "4bf92f3577b34da6a3ce929d0e0e4738"), ("python", WorkerLang::Python, "4bf92f3577b34da6a3ce929d0e0e4739")]
    {
        let collector = MockCollector::start().await;
        // Reuse the process's immutable sqlgen namespace under SCHEMA_GATE, with per-iteration cleanup.
        let schema = format!("aip_otlp_job_{}", std::process::id());
        let _cleanup = DropSchema(schema.clone());
        sqlgen::set_schema(&schema);
        let source = format!(
            "{}\n{}",
            include_str!("fixtures/jobs.aip"),
            r#"
operation job traceRoot {
 input {text: Text} output {length: Int} allow actor != null
 access jobs.traceMiddle, jobs.count effect managed deadline 30s implementation "jobs.traceRoot"
}
operation job traceMiddle {
 input {text: Text} output {length: Int} allow actor != null
 access jobs.count effect managed deadline 20s implementation "jobs.traceMiddle"
}
"#
        );
        let facts = load_str(&source, Form::A).unwrap().execution;
        aip_migrate::init(aip_test_support::test_database_url!(), &schema, &facts).await.unwrap();
        let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        db.batch_execute(&format!(
            "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
        ))
        .await
        .unwrap();
        let iat: i64 = db.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
        let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::DecimalString,
            "127.0.0.1:0".parse().unwrap(),
            aip_test_support::test_database_url!().into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
            ServerOptions::default(),
            false,
            Arc::new(DbAuth { schema: schema.clone(), iat }),
            Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
            None,
            false,
            Infrastructure {
                otlp: Some(OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: HashMap::new(), timeout_ms: 3000 }),
                ..Infrastructure::default()
            },
        )
        .await
        .unwrap();
        let caller_span = "00f067aa0ba902b7";
        let traceparent = format!("00-{trace_id}-{caller_span}-01");
        let submitted = call(
            server.address,
            "/jobs/enqueue",
            &fingerprint,
            &[("traceparent", &traceparent)],
            json!({"operation":"traceRoot","input":{"text":"TRACE_SENSITIVE"},"key":"nested-trace","delayMs":0}),
        )
        .await;
        assert_eq!(submitted["ok"], true, "{label}: {submitted}");
        let job_id = submitted["run"]["id"].as_str().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let status = call(server.address, "/jobs/status", &fingerprint, &[], json!({"id":job_id})).await;
            if status["run"]["state"] == "SUCCEEDED" {
                break;
            }
            assert_ne!(status["run"]["state"], "FAILED", "{label}: {status}");
            assert!(tokio::time::Instant::now() < deadline, "{label}: {status}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let spans = loop {
            let spans = spans_of(&collector, trace_id);
            if ["JOB traceRoot", "JOB traceMiddle", "JOB count"].iter().all(|name| spans.iter().any(|span| span["name"] == *name)) {
                break spans;
            }
            assert!(tokio::time::Instant::now() < deadline, "{label}: missing nested spans: {spans:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let find = |name: &str| spans.iter().find(|span| span["name"] == name).unwrap();
        let root = find("JOB traceRoot");
        let middle = find("JOB traceMiddle");
        let leaf = find("JOB count");
        let request = spans.iter().find(|span| span["name"] == "HTTP jobs" && span["parentSpanId"] == caller_span).unwrap();
        assert_eq!(root["parentSpanId"], request["spanId"], "{label}");
        assert_eq!(middle["parentSpanId"], root["spanId"], "{label}");
        assert_eq!(leaf["parentSpanId"], middle["spanId"], "{label}");
        for span in [root, middle, leaf] {
            assert_eq!(span["status"]["code"], 1);
            assert_eq!(span["traceId"], trace_id);
            assert!(span["attributes"].as_array().unwrap().iter().any(|a| a["key"] == "job.id" && a["value"]["stringValue"] == job_id));
        }
        assert_ne!(root["spanId"], middle["spanId"]);
        assert_ne!(middle["spanId"], leaf["spanId"]);
        assert_ne!(root["spanId"], leaf["spanId"]);
        assert!(!serde_json::to_string(&spans).unwrap().contains("TRACE_SENSITIVE"));
        let runs: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_jobs"), &[]).await.unwrap().get(0);
        assert_eq!(runs, 1, "nested steps share the root run; they do not enqueue new durable jobs");
    }
}

#[tokio::test]
async fn mock_collector_reassembles_a_fragmented_json_body() {
    let collector = MockCollector::start().await;
    let body = r#"{"resourceSpans":[]}"#;
    let mut stream = TcpStream::connect(collector.addr).await.unwrap();
    stream
        .write_all(format!("POST /v1/traces HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    let mut early = [0; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.read(&mut early)).await.is_err(),
        "collector acknowledged headers before receiving the JSON body"
    );
    for chunk in body.as_bytes().chunks(3) {
        stream.write_all(chunk).await.unwrap();
        tokio::task::yield_now().await;
    }
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    assert_eq!(collector.received_traces.lock().unwrap().len(), 1);
}
