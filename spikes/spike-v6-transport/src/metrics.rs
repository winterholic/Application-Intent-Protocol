//! Request metrics in the Prometheus text exposition format, without an exporter dependency.
//!
//! Every finished request is counted by route name, status and AIP error code, and its duration goes into a
//! per-route histogram. Labels come only from fixed sets (the route names of `trace`, the status number, the
//! server's own error codes), so a caller cannot grow the series count with paths or values.
//!
//! `/metrics` is served only when the operator configured a token; the scrape sends it as a Bearer token.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// Upper bounds in seconds. Requests are bounded by the 30 s request deadline, so larger buckets add nothing.
const BUCKETS: [f64; 12] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

#[derive(Default)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    count: u64,
    sum: f64,
}

#[derive(Default)]
struct Registry {
    requests: BTreeMap<(&'static str, u16, String), u64>,
    durations: BTreeMap<&'static str, Histogram>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry { requests: BTreeMap::new(), durations: BTreeMap::new() });

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

/// The current values as Prometheus text (version 0.0.4).
pub(crate) fn render() -> String {
    let Ok(registry) = REGISTRY.lock() else { return String::new() };
    let mut out = String::new();
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
    fn only_the_exact_bearer_token_opens_the_scrape() {
        assert!(token_matches("s3cret-token", Some("Bearer s3cret-token")));
        for given in [None, Some("s3cret-token"), Some("Bearer s3cret-toke"), Some("bearer s3cret-token"), Some("Bearer ")] {
            assert!(!token_matches("s3cret-token", given), "{given:?}");
        }
    }
}
