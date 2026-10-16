//! OpenTelemetry Protocol (OTLP/HTTP) exporter for traces and metrics.
//!
//! Spans and metric snapshots are exported over HTTP using standard OTLP JSON payload format
//! (`/v1/traces` and `/v1/metrics`).
//!
//! Configuration is optional (default off). When turned off, the exporter costs a single `Option` check.
//! When active, spans are passed through a bounded queue to a background task so observability
//! never blocks request handling.

use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

fn default_timeout_ms() -> u64 {
    5000
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OtlpConfig {
    pub endpoint: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

impl OtlpConfig {
    pub fn validate(&self) -> std::io::Result<()> {
        if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "otlp.endpoint must begin with http:// or https://"));
        }
        if !(100..=60_000).contains(&self.timeout_ms) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "otlp.timeout_ms must be between 100 and 60000"));
        }
        for (k, v) in &self.headers {
            if k.is_empty() || k.len() > 128 || v.len() > 512 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid otlp header name or value"));
            }
        }
        Ok(())
    }
}

/// Official OpenTelemetry trace.proto status code enum:
/// STATUS_CODE_UNSET = 0
/// STATUS_CODE_OK = 1
/// STATUS_CODE_ERROR = 2
/// (Note: do NOT confuse with SpanKind enum, where INTERNAL=1, SERVER=2, CLIENT=3)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[repr(i32)]
pub enum SpanStatusCode {
    Unset = 0,
    Ok = 1,
    Error = 2,
}

impl SpanStatusCode {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Map HTTP response status code to OTLP SpanStatusCode.
    /// 200..400 -> Ok (1)
    /// Some(_) (4xx, 5xx, etc.) -> Error (2)
    /// None (interrupted / unrecorded) -> Unset (0)
    pub fn from_http_status(status: Option<u16>) -> Self {
        match status {
            Some(s) if (200..400).contains(&s) => Self::Ok,
            Some(_) => Self::Error,
            None => Self::Unset,
        }
    }

    /// Map durable job outcome state to OTLP SpanStatusCode.
    /// "SUCCEEDED" -> Ok (1)
    /// "FAILED" | "CANCELLED" -> Error (2)
    /// _ ("RETRY", "PENDING", etc.) -> Unset (0)
    pub fn from_job_state(state: &str) -> Self {
        match state {
            "SUCCEEDED" => Self::Ok,
            "FAILED" | "CANCELLED" => Self::Error,
            _ => Self::Unset,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpanData {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: i32, // 1: INTERNAL, 2: SERVER, 3: CLIENT
    pub start_time_nanos: u64,
    pub end_time_nanos: u64,
    pub attributes: Vec<(String, Value)>,
    pub status_code: SpanStatusCode, // Official OTLP: 0=UNSET, 1=OK, 2=ERROR
    pub status_message: Option<String>,
}

pub struct OtlpExporter {
    tx: mpsc::Sender<SpanData>,
    dropped: Arc<AtomicU64>,
}

impl OtlpExporter {
    pub fn start(config: OtlpConfig) -> std::io::Result<Arc<Self>> {
        config.validate()?;
        let (tx, mut rx) = mpsc::channel::<SpanData>(4096);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_bg = dropped.clone();

        let client =
            reqwest::Client::builder().timeout(Duration::from_millis(config.timeout_ms)).build().map_err(|e| std::io::Error::other(e.to_string()))?;

        let traces_url = format!("{}/v1/traces", config.endpoint.trim_end_matches('/'));
        let metrics_url = format!("{}/v1/metrics", config.endpoint.trim_end_matches('/'));
        let headers = config.headers;

        tokio::spawn(async move {
            let mut span_batch: Vec<SpanData> = Vec::with_capacity(64);
            let mut trace_flush_interval = tokio::time::interval(Duration::from_millis(500));
            let mut metric_flush_interval = tokio::time::interval(Duration::from_secs(2));

            loop {
                tokio::select! {
                    maybe_span = rx.recv() => {
                        match maybe_span {
                            Some(span) => {
                                span_batch.push(span);
                                if span_batch.len() >= 64 {
                                    send_traces(&client, &traces_url, &headers, std::mem::take(&mut span_batch)).await;
                                }
                            }
                            None => {
                                // Channel closed, flush remaining spans and exit
                                if !span_batch.is_empty() {
                                    send_traces(&client, &traces_url, &headers, std::mem::take(&mut span_batch)).await;
                                }
                                break;
                            }
                        }
                    }
                    _ = trace_flush_interval.tick() => {
                        if !span_batch.is_empty() {
                            send_traces(&client, &traces_url, &headers, std::mem::take(&mut span_batch)).await;
                        }
                    }
                    _ = metric_flush_interval.tick() => {
                        send_metrics(&client, &metrics_url, &headers).await;
                    }
                }

                let d = dropped_bg.swap(0, Ordering::Relaxed);
                if d > 0 {
                    eprintln!("{{\"event\":\"otlp_spans_dropped\",\"count\":{d}}}");
                }
            }
        });

        Ok(Arc::new(Self { tx, dropped }))
    }

    /// Enqueues one finished span. Never blocks the caller task.
    pub fn export_span(&self, span: SpanData) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(span) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn now_nanos() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
}

pub fn random_span_id() -> String {
    loop {
        let id = uuid::Uuid::new_v4();
        let raw = &id.as_bytes()[..8];
        if raw.iter().any(|b| *b != 0) {
            return raw.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
}

fn to_otlp_value(val: &Value) -> Value {
    match val {
        Value::String(s) => json!({ "stringValue": s }),
        Value::Bool(b) => json!({ "boolValue": b }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                json!({ "intValue": i })
            } else if let Some(f) = n.as_f64() {
                json!({ "doubleValue": f })
            } else {
                json!({ "stringValue": n.to_string() })
            }
        }
        _ => json!({ "stringValue": val.to_string() }),
    }
}

async fn send_traces(client: &reqwest::Client, url: &str, headers: &HashMap<String, String>, spans: Vec<SpanData>) {
    if spans.is_empty() {
        return;
    }
    let otlp_spans: Vec<Value> = spans
        .into_iter()
        .map(|s| {
            let attrs: Vec<Value> = s
                .attributes
                .into_iter()
                .map(|(k, v)| {
                    json!({
                        "key": k,
                        "value": to_otlp_value(&v),
                    })
                })
                .collect();

            let mut span_obj = json!({
                "traceId": s.trace_id,
                "spanId": s.span_id,
                "name": s.name,
                "kind": s.kind,
                "startTimeUnixNano": s.start_time_nanos.to_string(),
                "endTimeUnixNano": s.end_time_nanos.to_string(),
                "attributes": attrs,
                "status": {
                    "code": s.status_code.as_i32(),
                }
            });

            if let Some(parent) = s.parent_span_id {
                span_obj["parentSpanId"] = json!(parent);
            }
            if let Some(msg) = s.status_message {
                span_obj["status"]["message"] = json!(msg);
            }

            span_obj
        })
        .collect();

    let payload = json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "aip-service" } }
                ]
            },
            "scopeSpans": [{
                "scope": {
                    "name": "aip",
                    "version": "0.1.0"
                },
                "spans": otlp_spans
            }]
        }]
    });

    let mut req = client.post(url).json(&payload);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let _ = req.send().await;
}

fn to_otlp_histogram_points(durations: &[crate::metrics::DurationHistogramData], attr_key: &str, now: &str) -> Vec<Value> {
    durations
        .iter()
        .map(|(label, buckets, count, sum)| {
            let mut explicit_bounds = Vec::new();
            let mut bucket_counts = Vec::new();
            let mut prev = 0u64;
            for (bound, cum) in buckets {
                explicit_bounds.push(*bound);
                bucket_counts.push(cum.saturating_sub(prev).to_string());
                prev = *cum;
            }
            bucket_counts.push(count.saturating_sub(prev).to_string());

            json!({
                "timeUnixNano": now,
                "count": count.to_string(),
                "sum": *sum,
                "bucketCounts": bucket_counts,
                "explicitBounds": explicit_bounds,
                "attributes": [
                    { "key": attr_key, "value": { "stringValue": label } }
                ]
            })
        })
        .collect()
}

async fn send_metrics(client: &reqwest::Client, url: &str, headers: &HashMap<String, String>) {
    let snap = crate::metrics::snapshot();
    let now = now_nanos().to_string();

    let mut metrics: Vec<Value> = Vec::new();

    // Requests counter
    if !snap.requests.is_empty() {
        let data_points: Vec<Value> = snap
            .requests
            .iter()
            .map(|((route, status, code), count)| {
                json!({
                    "timeUnixNano": now,
                    "asInt": count.to_string(),
                    "attributes": [
                        { "key": "route", "value": { "stringValue": route } },
                        { "key": "status", "value": { "intValue": *status as i64 } },
                        { "key": "code", "value": { "stringValue": code } }
                    ]
                })
            })
            .collect();

        metrics.push(json!({
            "name": "aip_requests_total",
            "description": "Finished HTTP requests by route, status and AIP error code",
            "unit": "1",
            "sum": {
                "dataPoints": data_points,
                "aggregationTemporality": 2, // AGGREGATION_TEMPORALITY_CUMULATIVE
                "isMonotonic": true
            }
        }));
    }

    // Request durations histogram
    if !snap.request_durations.is_empty() {
        let data_points = to_otlp_histogram_points(&snap.request_durations, "route", &now);
        metrics.push(json!({
            "name": "aip_request_duration_seconds",
            "description": "Finished HTTP request duration histogram by route",
            "unit": "s",
            "histogram": {
                "dataPoints": data_points,
                "aggregationTemporality": 2
            }
        }));
    }

    // Operations counter
    if !snap.operations.is_empty() {
        let data_points: Vec<Value> = snap
            .operations
            .iter()
            .map(|((op, status, code), count)| {
                json!({
                    "timeUnixNano": now,
                    "asInt": count.to_string(),
                    "attributes": [
                        { "key": "operation", "value": { "stringValue": op } },
                        { "key": "status", "value": { "intValue": *status as i64 } },
                        { "key": "code", "value": { "stringValue": code } }
                    ]
                })
            })
            .collect();

        metrics.push(json!({
            "name": "aip_operations_total",
            "description": "Finished operations by name, status and AIP error code",
            "unit": "1",
            "sum": {
                "dataPoints": data_points,
                "aggregationTemporality": 2,
                "isMonotonic": true
            }
        }));
    }

    // Operation durations histogram
    if !snap.operation_durations.is_empty() {
        let data_points = to_otlp_histogram_points(&snap.operation_durations, "operation", &now);
        metrics.push(json!({
            "name": "aip_operation_duration_seconds",
            "description": "Finished operation duration histogram by operation name",
            "unit": "s",
            "histogram": {
                "dataPoints": data_points,
                "aggregationTemporality": 2
            }
        }));
    }

    // Jobs counter
    if !snap.jobs.is_empty() {
        let data_points: Vec<Value> = snap
            .jobs
            .iter()
            .map(|((op, state, code), count)| {
                json!({
                    "timeUnixNano": now,
                    "asInt": count.to_string(),
                    "attributes": [
                        { "key": "operation", "value": { "stringValue": op } },
                        { "key": "state", "value": { "stringValue": state } },
                        { "key": "code", "value": { "stringValue": code } }
                    ]
                })
            })
            .collect();

        metrics.push(json!({
            "name": "aip_jobs_total",
            "description": "Finished durable job runs and attempts by operation, state and AIP error code",
            "unit": "1",
            "sum": {
                "dataPoints": data_points,
                "aggregationTemporality": 2,
                "isMonotonic": true
            }
        }));
    }

    // Job durations histogram
    if !snap.job_durations.is_empty() {
        let data_points = to_otlp_histogram_points(&snap.job_durations, "operation", &now);
        metrics.push(json!({
            "name": "aip_job_duration_seconds",
            "description": "Finished durable job execution duration histogram by operation name",
            "unit": "s",
            "histogram": {
                "dataPoints": data_points,
                "aggregationTemporality": 2
            }
        }));
    }

    if metrics.is_empty() {
        return;
    }

    let payload = json!({
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "aip-service" } }
                ]
            },
            "scopeMetrics": [{
                "scope": {
                    "name": "aip",
                    "version": "0.1.0"
                },
                "metrics": metrics
            }]
        }]
    });

    let mut req = client.post(url).json(&payload);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let _ = req.send().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_status_code_enum_values_match_official_trace_proto() {
        // Official OpenTelemetry trace.proto:
        // enum StatusCode {
        //   STATUS_CODE_UNSET = 0;
        //   STATUS_CODE_OK = 1;
        //   STATUS_CODE_ERROR = 2;
        // }
        assert_eq!(SpanStatusCode::Unset.as_i32(), 0);
        assert_eq!(SpanStatusCode::Ok.as_i32(), 1);
        assert_eq!(SpanStatusCode::Error.as_i32(), 2);

        assert_ne!(SpanStatusCode::Ok.as_i32(), 2, "Ok must NOT be 2 (which is ERROR)");
        assert_ne!(SpanStatusCode::Error.as_i32(), 3, "Error must NOT be 3 (which is uninterpreted/invalid)");
    }

    #[test]
    fn http_status_mapping_produces_standard_status_codes() {
        // Success ranges (200..400) -> STATUS_CODE_OK (1)
        for s in [200, 201, 204, 301, 302, 304, 307, 308] {
            assert_eq!(SpanStatusCode::from_http_status(Some(s)), SpanStatusCode::Ok);
            assert_eq!(SpanStatusCode::from_http_status(Some(s)).as_i32(), 1);
        }

        // Error ranges (4xx, 5xx) -> STATUS_CODE_ERROR (2)
        for s in [400, 401, 403, 404, 409, 422, 500, 502, 503, 504] {
            assert_eq!(SpanStatusCode::from_http_status(Some(s)), SpanStatusCode::Error);
            assert_eq!(SpanStatusCode::from_http_status(Some(s)).as_i32(), 2);
        }

        // None (incomplete / connection dropped before response) -> STATUS_CODE_UNSET (0)
        assert_eq!(SpanStatusCode::from_http_status(None), SpanStatusCode::Unset);
        assert_eq!(SpanStatusCode::from_http_status(None).as_i32(), 0);
    }

    #[test]
    fn job_state_mapping_produces_standard_status_codes() {
        // Terminal success -> STATUS_CODE_OK (1)
        assert_eq!(SpanStatusCode::from_job_state("SUCCEEDED"), SpanStatusCode::Ok);
        assert_eq!(SpanStatusCode::from_job_state("SUCCEEDED").as_i32(), 1);

        // Terminal errors -> STATUS_CODE_ERROR (2)
        assert_eq!(SpanStatusCode::from_job_state("FAILED"), SpanStatusCode::Error);
        assert_eq!(SpanStatusCode::from_job_state("FAILED").as_i32(), 2);
        assert_eq!(SpanStatusCode::from_job_state("CANCELLED"), SpanStatusCode::Error);
        assert_eq!(SpanStatusCode::from_job_state("CANCELLED").as_i32(), 2);

        // Non-terminal / transient -> STATUS_CODE_UNSET (0)
        assert_eq!(SpanStatusCode::from_job_state("RETRY"), SpanStatusCode::Unset);
        assert_eq!(SpanStatusCode::from_job_state("RETRY").as_i32(), 0);
        assert_eq!(SpanStatusCode::from_job_state("PENDING"), SpanStatusCode::Unset);
        assert_eq!(SpanStatusCode::from_job_state("RUNNING"), SpanStatusCode::Unset);
        assert_eq!(SpanStatusCode::from_job_state("CANCEL_REQUESTED"), SpanStatusCode::Unset);
    }
}
