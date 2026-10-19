//! Request metrics in the Prometheus text exposition format, without an exporter dependency.
//!
//! Every finished request is counted by route name, status and AIP error code, and its duration goes into a
//! per-route histogram. Labels come only from fixed sets (the route names of `trace`, the status number, the
//! server's own error codes), so a caller cannot grow the series count with paths or values.
//!
//! `/metrics` is served only when the operator configured a token; the scrape sends it as a Bearer token.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Upper bounds in seconds. Requests are bounded by the 30 s request deadline, so larger buckets add nothing.
pub const BUCKETS: [f64; 12] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

static OTLP_SPANS_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Queue overflow must remain nonblocking and independent of the resettable log batch.
pub(crate) fn observe_otlp_span_drop() {
    OTLP_SPANS_DROPPED.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn dropped_span_count() -> u64 {
    OTLP_SPANS_DROPPED.load(Ordering::Relaxed)
}

#[derive(Clone, Default)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    count: u64,
    sum: f64,
}

#[derive(Default)]
struct Registry {
    requests: BTreeMap<(&'static str, u16, String), u64>,
    durations: BTreeMap<&'static str, Histogram>,
    operations: BTreeMap<(String, u16, String), u64>,
    operation_durations: BTreeMap<String, Histogram>,
    jobs: BTreeMap<(String, String, String), u64>,
    job_durations: BTreeMap<String, Histogram>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    requests: BTreeMap::new(),
    durations: BTreeMap::new(),
    operations: BTreeMap::new(),
    operation_durations: BTreeMap::new(),
    jobs: BTreeMap::new(),
    job_durations: BTreeMap::new(),
});

fn sanitize_label(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '_' | '-' | '.')).take(64).collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// Counts one finished request. `status` 0 means the connection ended before a reply was written.
pub(crate) fn observe(route: &'static str, status: Option<u16>, code: Option<&str>, elapsed: Duration) {
    let Ok(mut registry) = REGISTRY.lock() else { return };
    *registry.requests.entry((route, status.unwrap_or(0), code.unwrap_or("").to_string())).or_default() += 1;
    let seconds = elapsed.as_secs_f64();
    let histogram = registry.durations.entry(route).or_default();
    for (slot, bound) in histogram.buckets.iter_mut().zip(BUCKETS) {
        if seconds <= bound {
            *slot += 1;
        }
    }
    histogram.count += 1;
    histogram.sum += seconds;
}

/// Counts one finished operation execution.
pub(crate) fn observe_operation(operation: &str, status: u16, code: Option<&str>, elapsed: Duration) {
    let Ok(mut registry) = REGISTRY.lock() else { return };
    let op = sanitize_label(operation);
    *registry.operations.entry((op.clone(), status, code.unwrap_or("").to_string())).or_default() += 1;
    let seconds = elapsed.as_secs_f64();
    let histogram = registry.operation_durations.entry(op).or_default();
    for (slot, bound) in histogram.buckets.iter_mut().zip(BUCKETS) {
        if seconds <= bound {
            *slot += 1;
        }
    }
    histogram.count += 1;
    histogram.sum += seconds;
}

/// Counts one finished durable job attempt or outcome.
pub(crate) fn observe_job(operation: &str, state: &str, code: Option<&str>, elapsed: Duration) {
    let Ok(mut registry) = REGISTRY.lock() else { return };
    let op = sanitize_label(operation);
    let state_str = match state {
        "SUCCEEDED" | "FAILED" | "RETRY" | "CANCELLED" => state.to_string(),
        _ => sanitize_label(state),
    };
    *registry.jobs.entry((op.clone(), state_str, code.unwrap_or("").to_string())).or_default() += 1;
    let seconds = elapsed.as_secs_f64();
    let histogram = registry.job_durations.entry(op).or_default();
    for (slot, bound) in histogram.buckets.iter_mut().zip(BUCKETS) {
        if seconds <= bound {
            *slot += 1;
        }
    }
    histogram.count += 1;
    histogram.sum += seconds;
}

pub type DurationHistogramData = (String, Vec<(f64, u64)>, u64, f64);

#[derive(Clone, Debug, Default)]
pub struct MetricsSnapshot {
    pub otlp_spans_dropped: u64,
    pub requests: Vec<((String, u16, String), u64)>,
    pub request_durations: Vec<DurationHistogramData>,
    pub operations: Vec<((String, u16, String), u64)>,
    pub operation_durations: Vec<DurationHistogramData>,
    pub jobs: Vec<((String, String, String), u64)>,
    pub job_durations: Vec<DurationHistogramData>,
}

/// Takes a snapshot of all accumulated metrics for exporters.
pub fn snapshot() -> MetricsSnapshot {
    let Ok(registry) = REGISTRY.lock() else { return MetricsSnapshot::default() };
    MetricsSnapshot {
        otlp_spans_dropped: dropped_span_count(),
        requests: registry.requests.iter().map(|((r, s, c), n)| ((r.to_string(), *s, c.clone()), *n)).collect(),
        request_durations: registry
            .durations
            .iter()
            .map(|(r, h)| (r.to_string(), BUCKETS.iter().copied().zip(h.buckets).collect(), h.count, h.sum))
            .collect(),
        operations: registry.operations.iter().map(|(k, n)| (k.clone(), *n)).collect(),
        operation_durations: registry
            .operation_durations
            .iter()
            .map(|(op, h)| (op.clone(), BUCKETS.iter().copied().zip(h.buckets).collect(), h.count, h.sum))
            .collect(),
        jobs: registry.jobs.iter().map(|(k, n)| (k.clone(), *n)).collect(),
        job_durations: registry
            .job_durations
            .iter()
            .map(|(op, h)| (op.clone(), BUCKETS.iter().copied().zip(h.buckets).collect(), h.count, h.sum))
            .collect(),
    }
}

/// The current values as Prometheus text (version 0.0.4).
pub(crate) fn render() -> String {
    let Ok(registry) = REGISTRY.lock() else { return String::new() };
    let mut out = String::new();
    out.push_str("# HELP aip_otlp_spans_dropped_total Spans dropped because the OTLP export queue was full.\n");
    out.push_str("# TYPE aip_otlp_spans_dropped_total counter\n");
    out.push_str(&format!("aip_otlp_spans_dropped_total {}\n", dropped_span_count()));
    out.push_str("# HELP aip_requests_total Finished HTTP requests by route, status and AIP error code.\n");
    out.push_str("# TYPE aip_requests_total counter\n");
    for ((route, status, code), n) in &registry.requests {
        out.push_str(&format!("aip_requests_total{{route=\"{route}\",status=\"{status}\",code=\"{code}\"}} {n}\n"));
    }
    out.push_str("# HELP aip_request_duration_seconds Time from accepting a request to finishing its reply.\n");
    out.push_str("# TYPE aip_request_duration_seconds histogram\n");
    for (route, histogram) in &registry.durations {
        for (n, bound) in histogram.buckets.iter().zip(BUCKETS) {
            out.push_str(&format!("aip_request_duration_seconds_bucket{{route=\"{route}\",le=\"{bound}\"}} {n}\n"));
        }
        out.push_str(&format!("aip_request_duration_seconds_bucket{{route=\"{route}\",le=\"+Inf\"}} {}\n", histogram.count));
        out.push_str(&format!("aip_request_duration_seconds_sum{{route=\"{route}\"}} {}\n", histogram.sum));
        out.push_str(&format!("aip_request_duration_seconds_count{{route=\"{route}\"}} {}\n", histogram.count));
    }
    out.push_str("# HELP aip_operations_total Finished operations by name, status and AIP error code.\n");
    out.push_str("# TYPE aip_operations_total counter\n");
    for ((operation, status, code), n) in &registry.operations {
        out.push_str(&format!("aip_operations_total{{operation=\"{operation}\",status=\"{status}\",code=\"{code}\"}} {n}\n"));
    }
    out.push_str("# HELP aip_operation_duration_seconds Duration of operation executions.\n");
    out.push_str("# TYPE aip_operation_duration_seconds histogram\n");
    for (operation, histogram) in &registry.operation_durations {
        for (n, bound) in histogram.buckets.iter().zip(BUCKETS) {
            out.push_str(&format!("aip_operation_duration_seconds_bucket{{operation=\"{operation}\",le=\"{bound}\"}} {n}\n"));
        }
        out.push_str(&format!("aip_operation_duration_seconds_bucket{{operation=\"{operation}\",le=\"+Inf\"}} {}\n", histogram.count));
        out.push_str(&format!("aip_operation_duration_seconds_sum{{operation=\"{operation}\"}} {}\n", histogram.sum));
        out.push_str(&format!("aip_operation_duration_seconds_count{{operation=\"{operation}\"}} {}\n", histogram.count));
    }
    out.push_str("# HELP aip_jobs_total Finished durable job runs and attempts by operation, state and AIP error code.\n");
    out.push_str("# TYPE aip_jobs_total counter\n");
    for ((operation, state, code), n) in &registry.jobs {
        out.push_str(&format!("aip_jobs_total{{operation=\"{operation}\",state=\"{state}\",code=\"{code}\"}} {n}\n"));
    }
    out.push_str("# HELP aip_job_duration_seconds Duration of durable job attempts.\n");
    out.push_str("# TYPE aip_job_duration_seconds histogram\n");
    for (operation, histogram) in &registry.job_durations {
        for (n, bound) in histogram.buckets.iter().zip(BUCKETS) {
            out.push_str(&format!("aip_job_duration_seconds_bucket{{operation=\"{operation}\",le=\"{bound}\"}} {n}\n"));
        }
        out.push_str(&format!("aip_job_duration_seconds_bucket{{operation=\"{operation}\",le=\"+Inf\"}} {}\n", histogram.count));
        out.push_str(&format!("aip_job_duration_seconds_sum{{operation=\"{operation}\"}} {}\n", histogram.sum));
        out.push_str(&format!("aip_job_duration_seconds_count{{operation=\"{operation}\"}} {}\n", histogram.count));
    }
    out
}

/// Compares the scrape token without an early exit on the first differing byte.
pub(crate) fn token_matches(expected: &str, authorization: Option<&str>) -> bool {
    use sha2::{Digest, Sha256};
    let Some(given) = authorization.and_then(|value| value.strip_prefix("Bearer ")) else { return false };
    let (a, b) = (Sha256::digest(expected.as_bytes()), Sha256::digest(given.as_bytes()));
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_are_cumulative_and_codes_stay_labels() {
        observe("metrics_test", Some(200), None, Duration::from_millis(3));
        observe("metrics_test", Some(200), Some("WIRE.BAD_VALUE"), Duration::from_millis(70));
        let text = render();
        assert!(text.contains("aip_requests_total{route=\"metrics_test\",status=\"200\",code=\"\"} 1\n"), "{text}");
        assert!(text.contains("aip_requests_total{route=\"metrics_test\",status=\"200\",code=\"WIRE.BAD_VALUE\"} 1\n"), "{text}");
        assert!(text.contains("aip_request_duration_seconds_bucket{route=\"metrics_test\",le=\"0.005\"} 1\n"), "{text}");
        assert!(text.contains("aip_request_duration_seconds_bucket{route=\"metrics_test\",le=\"0.1\"} 2\n"), "{text}");
        assert!(text.contains("aip_request_duration_seconds_count{route=\"metrics_test\"} 2\n"), "{text}");
    }

    #[test]
    fn operations_and_jobs_metrics_render_properly() {
        observe_operation("compute_summary", 200, None, Duration::from_millis(15));
        observe_operation("compute_summary", 400, Some("WIRE.ACCESS_DENIED"), Duration::from_millis(50));
        observe_job("backup", "SUCCEEDED", None, Duration::from_millis(250));
        observe_job("backup", "FAILED", Some("WIRE.WORKER_FAILED"), Duration::from_millis(500));
        let text = render();
        assert!(text.contains("aip_operations_total{operation=\"compute_summary\",status=\"200\",code=\"\"} 1\n"), "{text}");
        assert!(text.contains("aip_operations_total{operation=\"compute_summary\",status=\"400\",code=\"WIRE.ACCESS_DENIED\"} 1\n"), "{text}");
        assert!(text.contains("aip_operation_duration_seconds_count{operation=\"compute_summary\"} 2\n"), "{text}");
        assert!(text.contains("aip_jobs_total{operation=\"backup\",state=\"SUCCEEDED\",code=\"\"} 1\n"), "{text}");
        assert!(text.contains("aip_jobs_total{operation=\"backup\",state=\"FAILED\",code=\"WIRE.WORKER_FAILED\"} 1\n"), "{text}");
        assert!(text.contains("aip_job_duration_seconds_count{operation=\"backup\"} 2\n"), "{text}");

        let snap = snapshot();
        assert!(snap.operations.iter().any(|((op, s, c), n)| op == "compute_summary" && *s == 200 && c.is_empty() && *n == 1));
        assert!(snap.jobs.iter().any(|((op, state, c), n)| op == "backup" && state == "SUCCEEDED" && c.is_empty() && *n == 1));
    }

    #[test]
    fn only_the_exact_bearer_token_opens_the_scrape() {
        assert!(token_matches("s3cret-token", Some("Bearer s3cret-token")));
        for given in [None, Some("s3cret-token"), Some("Bearer s3cret-toke"), Some("bearer s3cret-token"), Some("Bearer ")] {
            assert!(!token_matches("s3cret-token", given), "{given:?}");
        }
    }
}
