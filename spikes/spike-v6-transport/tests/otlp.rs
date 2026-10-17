//! OpenTelemetry Protocol (OTLP/HTTP) exporter integration tests.
//! Verifies trace span propagation, durable job trace linking, metrics export,
//! and that sensitive parameters are never recorded in span attributes.

use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    otlp::OtlpConfig,
    server::{listen_with_infrastructure, ServerOptions},
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
                            let mut buf = vec![0u8; 64 * 1024];
                            let n = match socket.read(&mut buf).await {
                                Ok(n) if n > 0 => n,
                                _ => return,
                            };
                            let req = String::from_utf8_lossy(&buf[..n]);
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
async fn durable_job_and_http_spans_share_same_trace_id() {
    let collector = MockCollector::start().await;

    let otlp_config = OtlpConfig { endpoint: format!("http://{}", collector.addr), headers: HashMap::new(), timeout_ms: 3000 };

    let exporter = spike_v6_transport::otlp::OtlpExporter::start(otlp_config).unwrap();

    let shared_trace_id = "0af7651916cd43dd8448eb211c80319c".to_string();
    let http_span_id = spike_v6_transport::otlp::random_span_id();
    let job_span_id = spike_v6_transport::otlp::random_span_id();
    let now = spike_v6_transport::otlp::now_nanos();

    // Export HTTP ingress span
    exporter.export_span(spike_v6_transport::otlp::SpanData {
        trace_id: shared_trace_id.clone(),
        span_id: http_span_id.clone(),
        parent_span_id: None,
        name: "HTTP jobs".to_string(),
        kind: 2, // SERVER
        start_time_nanos: now - 100_000_000,
        end_time_nanos: now - 50_000_000,
        attributes: vec![
            ("http.method".to_string(), serde_json::Value::String("POST".into())),
            ("http.route".to_string(), serde_json::Value::String("jobs".into())),
            ("http.status_code".to_string(), serde_json::json!(200)),
        ],
        status_code: spike_v6_transport::otlp::SpanStatusCode::Ok,
        status_message: None,
    });

    // Export durable job execution span with the exact same trace_id
    exporter.export_span(spike_v6_transport::otlp::SpanData {
        trace_id: shared_trace_id.clone(),
        span_id: job_span_id.clone(),
        parent_span_id: Some(http_span_id.clone()),
        name: "JOB report".to_string(),
        kind: 1, // INTERNAL
        start_time_nanos: now - 40_000_000,
        end_time_nanos: now,
        attributes: vec![
            ("job.operation".to_string(), serde_json::Value::String("report".into())),
            ("job.id".to_string(), serde_json::Value::String("b0123456-789a-bcde-f012-3456789abcde".into())),
            ("job.epoch".to_string(), serde_json::json!(1)),
            ("job.state".to_string(), serde_json::Value::String("SUCCEEDED".into())),
        ],
        status_code: spike_v6_transport::otlp::SpanStatusCode::Ok,
        status_message: None,
    });

    // Wait for the collector to receive both spans in the same trace
    let started = std::time::Instant::now();
    let mut collected_spans = Vec::new();
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
                                        if span.get("traceId").and_then(Value::as_str) == Some(&shared_trace_id) {
                                            collected_spans.push(span.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if collected_spans.len() >= 2 {
            break;
        }
        collected_spans.clear();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(collected_spans.len(), 2, "Both HTTP span and durable job span must be received under the same trace_id");
    let names: Vec<&str> = collected_spans.iter().filter_map(|s| s.get("name").and_then(Value::as_str)).collect();
    assert!(names.contains(&"HTTP jobs"));
    assert!(names.contains(&"JOB report"));

    let job_span = collected_spans.iter().find(|s| s["name"] == "JOB report").unwrap();
    assert_eq!(job_span["parentSpanId"].as_str(), Some(http_span_id.as_str()), "Job span parentSpanId must match HTTP spanId");
    assert_eq!(job_span["status"]["code"].as_i64(), Some(1), "Durable job success span status must be OK (1)");

    let http_span = collected_spans.iter().find(|s| s["name"] == "HTTP jobs").unwrap();
    assert_eq!(http_span["status"]["code"].as_i64(), Some(1), "HTTP success span status must be OK (1)");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otlp_exporter_queue_saturation_drops_spans_without_blocking() {
    let unreachable_addr: SocketAddr = "127.0.0.1:19999".parse().unwrap();
    let otlp_config = OtlpConfig { endpoint: format!("http://{unreachable_addr}"), headers: HashMap::new(), timeout_ms: 100 };
    let exporter = spike_v6_transport::otlp::OtlpExporter::start(otlp_config).unwrap();
    let now = spike_v6_transport::otlp::now_nanos();

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
    assert!(elapsed < Duration::from_millis(100), "Enqueuing 5000 spans must complete non-blockingly under 100ms, took {elapsed:?}");
}
