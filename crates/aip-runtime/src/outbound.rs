//! Outbound webhooks: signed delivery of events to endpoints that users registered.
//!
//! An `outbound webhooks for E` form queues one row per event and matching endpoint in `_aip_outbound_delivery`
//! (from the event's dispatch, see `aip-pg/src/plan/outbound.rs`). This module sends those rows.
//!
//! What a receiver can rely on, and what it cannot:
//! - at least once: a delivery that is not acknowledged with a 2xx is repeated, and one that was acknowledged but
//!   whose answer got lost is repeated too. Every attempt of one event to one endpoint carries the same
//!   `AIP-Event-Id`; the receiver dedupes on it.
//! - no order: deliveries to one endpoint are independent, and a retried event arrives after later ones.
//! - authenticity and freshness: `AIP-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256 of "<t>.<body>">`, the scheme of
//!   the inbound `payments.stripe.webhook` source, with the endpoint's own secret. The timestamp is renewed on every
//!   attempt, so a receiver can refuse anything older than a few minutes and a captured request cannot be replayed.
//! - the endpoint's secret is `whsec_` + HMAC-SHA256(server secret, "aip-outbound-webhook:<Entity>:<endpoint id>"). It
//!   is stored nowhere (a copy of the database does not contain it) and is shown once, in the response of the command
//!   that created the endpoint (`returns ep { secret: ep.signingSecret }`).
//! - retries: `retry N over D` makes N retries after the first attempt, each wait twice the one before, the last one
//!   exactly D after the event. 4xx and 5xx answers, timeouts and connection errors are all retried; a URL the server
//!   refuses to call (below) is not.
//! - an endpoint that has only failed for `disable after D` is disabled and gets no more events; its undelivered
//!   deliveries are cancelled and the reason is kept in `_aip_outbound_endpoint`.
//!
//! The server only calls public addresses. The URL is resolved here, every address it resolves to must be public,
//! and the connection goes to the address that was checked (a second lookup by the HTTP client could answer
//! differently: DNS rebinding). Redirects are not followed and no proxy is used, for the same reason.

use crate::engine::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

pub const SIGNATURE_HEADER: &str = "AIP-Signature";
pub const EVENT_ID_HEADER: &str = "AIP-Event-Id";
pub const EVENT_HEADER: &str = "AIP-Event";
pub const DELIVERY_HEADER: &str = "AIP-Delivery-Id";
pub const ATTEMPT_HEADER: &str = "AIP-Delivery-Attempt";

/// How old a signature may be before a receiver should refuse it (Stripe's default window, as for inbound webhooks).
pub const TOLERANCE_SECS: i64 = 300;

const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// A claimed delivery is invisible to other workers for this long; a worker that dies mid-send is retried after it.
const LEASE_SECS: f64 = 120.0;
const WORKERS: usize = 4;

#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Call private, loopback and link-local addresses too. Never on in production.
    pub allow_private: bool,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn mac(key: &[u8], parts: &[&[u8]]) -> String {
    // HMAC takes a key of any length
    let Ok(mut m) = HmacSha256::new_from_slice(key) else { return String::new() };
    for p in parts {
        m.update(p);
    }
    hex(&m.finalize().into_bytes())
}

/// The secret the endpoint `endpoint` of the form for `entity` is signed with. The SQL of `ep.signingSecret` computes the same value.
pub fn signing_secret(server_secret: &[u8], entity: &str, endpoint: &str) -> String {
    let key = String::from_utf8_lossy(server_secret);
    format!("whsec_{}", mac(key.as_bytes(), &[format!("aip-outbound-webhook:{entity}:{endpoint}").as_bytes()]))
}

/// Value of the `AIP-Signature` header for `body` sent at unix time `t`.
pub fn signature_header(secret: &str, t: i64, body: &[u8]) -> String {
    format!("t={t},v1={}", mac(secret.as_bytes(), &[format!("{t}.").as_bytes(), body]))
}

/// What a receiver does: the timestamp is recent and one `v1` signature matches (compared in constant time).
pub fn verify_signature(secret: &str, header: &str, body: &[u8], now: i64) -> Result<(), String> {
    let mut t = None;
    let mut sigs = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => t = v.parse::<i64>().ok(),
            Some(("v1", v)) => sigs.push(v.to_string()),
            _ => {}
        }
    }
    let t = t.ok_or("signature header has no timestamp")?;
    if (now - t).abs() > TOLERANCE_SECS {
        return Err("signature timestamp outside the tolerance window".into());
    }
    let want = mac(secret.as_bytes(), &[format!("{t}.").as_bytes(), body]);
    let same = |a: &str| a.len() == want.len() && a.bytes().zip(want.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0;
    if sigs.iter().any(|s| same(s.trim())) { Ok(()) } else { Err("no matching signature".into()) }
}

/// When retry `attempt` (1-based) of `retry` is due, as milliseconds after the event: the waits double and the last retry falls
/// exactly `over_seconds` after it, so the whole schedule fits the window.
pub fn retry_offset_ms(attempt: u32, retry: u32, over_seconds: u64) -> i64 {
    if retry == 0 || attempt == 0 {
        return 0;
    }
    let total = (1u128 << retry.min(40)) - 1;
    let part = (1u128 << attempt.min(40)) - 1;
    (over_seconds as u128 * 1000 * part / total) as i64
}

fn public_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_broadcast()
        || a.is_multicast()
        || a.is_documentation()
        || o[0] == 0
        // carrier-grade NAT 100.64.0.0/10, IETF protocol assignments 192.0.0.0/24, benchmarking 198.18.0.0/15, reserved 240.0.0.0/4
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || o[0] >= 240)
}

fn public_v6(a: Ipv6Addr) -> bool {
    if let Some(v4) = a.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = a.segments();
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_multicast()
        // IPv4-compatible ::a.b.c.d
        || s[..6].iter().all(|x| *x == 0)
        // unique local fc00::/7, link-local fe80::/10, site-local fec0::/10
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        // documentation 2001:db8::/32, Teredo 2001::/32, 6to4 2002::/16 and NAT64 64:ff9b::/96 embed an IPv4 address we would have to vet
        || (s[0] == 0x2001 && (s[1] == 0x0db8 || s[1] == 0))
        || s[0] == 0x2002
        || (s[0] == 0x64 && s[1] == 0xff9b))
}

/// Is this an address on the public internet (not loopback, private, link-local, shared, reserved or multicast)?
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => public_v4(a),
        IpAddr::V6(a) => public_v6(a),
    }
}

/// A URL the server agreed to call, and the address to call it at.
#[derive(Debug)]
pub struct Target {
    pub url: url::Url,
    /// Host name and the address it resolved to; `None` when the URL names an IP address itself.
    pub pin: Option<(String, SocketAddr)>,
}

/// Is `url` something the server may call? http or https, no credentials, and every address of the host is public
/// (unless `allow_private`). DNS is resolved here so the answer can be checked; the caller connects to `pin`.
pub async fn check_url(raw: &str, allow_private: bool) -> Result<Target, String> {
    let url = url::Url::parse(raw).map_err(|e| format!("not a URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("scheme '{}' is not http or https", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("a URL with credentials is not called".into());
    }
    let port = url.port_or_known_default().ok_or("no port")?;
    let (addrs, pin_host): (Vec<IpAddr>, Option<String>) = match url.host() {
        None => return Err("no host".into()),
        Some(url::Host::Ipv4(a)) => (vec![IpAddr::V4(a)], None),
        Some(url::Host::Ipv6(a)) => (vec![IpAddr::V6(a)], None),
        Some(url::Host::Domain(d)) => {
            let found = tokio::net::lookup_host((d, port)).await.map_err(|e| format!("cannot resolve {d}: {e}"))?;
            (found.map(|s| s.ip()).collect(), Some(d.to_string()))
        }
    };
    let Some(first) = addrs.first().copied() else { return Err("the host has no address".into()) };
    if !allow_private && let Some(bad) = addrs.iter().find(|a| !is_public(**a)) {
        return Err(format!("the host is or resolves to {bad}, which is not a public address"));
    }
    Ok(Target { url, pin: pin_host.map(|h| (h, SocketAddr::new(first, port))) })
}

enum Outcome {
    Delivered(i32),
    /// Worth another try: the answer, or why there was none.
    Retry(Option<i32>, String),
    /// Will not work however often it is tried.
    Permanent(String),
}

struct Due {
    id: uuid::Uuid,
    form: String,
    endpoint: uuid::Uuid,
    event: String,
    event_id: String,
    body: String,
    attempts: i32,
    created_at: DateTime<Utc>,
}

pub async fn run(engine: Arc<Engine>) {
    for _ in 0..WORKERS {
        let engine = engine.clone();
        tokio::spawn(async move {
            loop {
                match tick(&engine).await {
                    Ok(true) => continue,
                    Ok(false) => tokio::time::sleep(Duration::from_millis(500)).await,
                    Err(e) => {
                        tracing::warn!(error = %e, "outbound webhooks");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }
}

/// Sends one due delivery. Returns true if there was one.
pub async fn tick(engine: &Engine) -> anyhow::Result<bool> {
    let db = engine.pool.get().await?;
    let Some(r) = db
        .query_opt(
            "UPDATE \"_aip_outbound_delivery\" SET \"next_attempt_at\" = now() + make_interval(secs => $1::float8) WHERE \"id\" = \
             (SELECT \"id\" FROM \"_aip_outbound_delivery\" WHERE \"status\" = 'PENDING' AND \"next_attempt_at\" <= now() ORDER BY \"next_attempt_at\", \"id\" LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING \"id\", \"form\", \"endpoint\", \"event\", \"event_id\", \"body\"::text, \"attempts\", \"created_at\"",
            &[&LEASE_SECS],
        )
        .await?
    else {
        return Ok(false);
    };
    let due = Due {
        id: r.get(0),
        form: r.get(1),
        endpoint: r.get(2),
        event: r.get(3),
        event_id: r.get(4),
        body: r.get(5),
        attempts: r.get(6),
        created_at: r.get(7),
    };
    let Some(plan) = engine.program.outbound.iter().find(|o| o.entity == due.form) else {
        cancel(&db, &due, "the form is no longer defined").await?;
        return Ok(true);
    };
    // an endpoint disabled after this delivery was queued gets nothing more
    let disabled: bool = db
        .query_opt(
            "SELECT \"disabled_at\" IS NOT NULL FROM \"_aip_outbound_endpoint\" WHERE \"form\" = $1 AND \"endpoint\" = $2",
            &[&due.form, &due.endpoint],
        )
        .await?
        .is_some_and(|r| r.get(0));
    if disabled {
        cancel(&db, &due, "the endpoint is disabled").await?;
        return Ok(true);
    }
    let live = if plan.soft_delete { " AND \"deleted_at\" IS NULL" } else { "" };
    let url: Option<String> = db
        .query_opt(&format!("SELECT \"{}\"::text FROM \"{}\" WHERE \"id\" = $1{live}", plan.url_column, plan.table), &[&due.endpoint])
        .await?
        .and_then(|r| r.get(0));
    let Some(url) = url else {
        cancel(&db, &due, "the endpoint no longer exists").await?;
        return Ok(true);
    };
    let outcome = send(engine, &due, &url).await;
    settle(&db, plan, &due, outcome).await?;
    Ok(true)
}

async fn send(engine: &Engine, due: &Due, url: &str) -> Outcome {
    let target = match check_url(url, engine.outbound.allow_private).await {
        Ok(t) => t,
        Err(why) => return Outcome::Permanent(format!("URL_REJECTED: {why}")),
    };
    let mut builder =
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().timeout(SEND_TIMEOUT).user_agent("aip-webhooks/1");
    if let Some((host, addr)) = &target.pin {
        builder = builder.resolve(host, *addr);
    }
    let Ok(client) = builder.build() else { return Outcome::Retry(None, "cannot build an HTTP client".into()) };
    let secret = signing_secret(&engine.secret, &due.form, &due.endpoint.to_string());
    let signature = signature_header(&secret, Utc::now().timestamp(), due.body.as_bytes());
    let sent = client
        .post(target.url)
        .header("Content-Type", "application/json")
        .header(SIGNATURE_HEADER, signature)
        .header(EVENT_ID_HEADER, &due.event_id)
        .header(EVENT_HEADER, &due.event)
        .header(DELIVERY_HEADER, due.id.to_string())
        .header(ATTEMPT_HEADER, (due.attempts + 1).to_string())
        .body(due.body.clone())
        .send()
        .await;
    match sent {
        Ok(resp) if resp.status().is_success() => Outcome::Delivered(i32::from(resp.status().as_u16())),
        Ok(resp) => Outcome::Retry(Some(i32::from(resp.status().as_u16())), format!("HTTP {}", resp.status().as_u16())),
        Err(e) => Outcome::Retry(None, format!("{}", e.without_url()).chars().take(300).collect()),
    }
}

async fn cancel(db: &deadpool_postgres::Object, due: &Due, why: &str) -> anyhow::Result<()> {
    db.execute("UPDATE \"_aip_outbound_delivery\" SET \"status\" = 'CANCELLED', \"last_error\" = $2 WHERE \"id\" = $1", &[&due.id, &why]).await?;
    Ok(())
}

async fn settle(db: &deadpool_postgres::Object, plan: &aip_plan::Outbound, due: &Due, outcome: Outcome) -> anyhow::Result<()> {
    let attempt = due.attempts + 1;
    let (status, error, permanent) = match outcome {
        Outcome::Delivered(code) => {
            db.execute(
                "UPDATE \"_aip_outbound_delivery\" SET \"status\" = 'DELIVERED', \"attempts\" = $2, \"delivered_at\" = now(), \"last_status\" = $3, \"last_error\" = NULL WHERE \"id\" = $1",
                &[&due.id, &attempt, &code],
            )
            .await?;
            db.execute(
                "UPDATE \"_aip_outbound_endpoint\" SET \"failing_since\" = NULL WHERE \"form\" = $1 AND \"endpoint\" = $2",
                &[&due.form, &due.endpoint],
            )
            .await?;
            return Ok(());
        }
        Outcome::Retry(status, error) => (status, error, false),
        Outcome::Permanent(error) => (None, error, true),
    };
    let retry_no = u32::try_from(attempt).unwrap_or(u32::MAX);
    if !permanent && retry_no <= plan.retry {
        let at = due.created_at + chrono::Duration::milliseconds(retry_offset_ms(retry_no, plan.retry, plan.over_seconds));
        db.execute(
            "UPDATE \"_aip_outbound_delivery\" SET \"attempts\" = $2, \"last_status\" = $3, \"last_error\" = $4, \"next_attempt_at\" = greatest($5::timestamptz, now()) WHERE \"id\" = $1",
            &[&due.id, &attempt, &status, &error, &at],
        )
        .await?;
    } else {
        db.execute(
            "UPDATE \"_aip_outbound_delivery\" SET \"status\" = 'FAILED', \"attempts\" = $2, \"last_status\" = $3, \"last_error\" = $4 WHERE \"id\" = $1",
            &[&due.id, &attempt, &status, &error],
        )
        .await?;
    }
    // the endpoint has been failing since its last success; too long a streak disables it
    let streak = db
        .query_one(
            "INSERT INTO \"_aip_outbound_endpoint\" (\"form\", \"endpoint\", \"failing_since\") VALUES ($1, $2, now()) \
             ON CONFLICT (\"form\", \"endpoint\") DO UPDATE SET \"failing_since\" = coalesce(\"_aip_outbound_endpoint\".\"failing_since\", now()) \
             RETURNING extract(epoch FROM now() - \"failing_since\")::float8, \"disabled_at\" IS NOT NULL",
            &[&due.form, &due.endpoint],
        )
        .await?;
    let (failing_for, already): (f64, bool) = (streak.get(0), streak.get(1));
    if let Some(limit) = plan.disable_after_seconds
        && !already
        && failing_for >= limit as f64
    {
        let why = format!("failing for {}s with no success; last error: {error}", failing_for as i64);
        db.execute(
            "UPDATE \"_aip_outbound_endpoint\" SET \"disabled_at\" = now(), \"disabled_reason\" = $3 WHERE \"form\" = $1 AND \"endpoint\" = $2",
            &[&due.form, &due.endpoint, &why],
        )
        .await?;
        db.execute(
            "UPDATE \"_aip_outbound_delivery\" SET \"status\" = 'CANCELLED', \"last_error\" = 'the endpoint is disabled' WHERE \"form\" = $1 AND \"endpoint\" = $2 AND \"status\" = 'PENDING'",
            &[&due.form, &due.endpoint],
        )
        .await?;
        tracing::warn!(form = %due.form, endpoint = %due.endpoint, "outbound webhook endpoint disabled: {why}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_round_trip_and_tampering() {
        let secret = signing_secret(b"server", "Endpoint", "11111111-1111-1111-1111-111111111111");
        assert!(secret.starts_with("whsec_") && secret.len() == 6 + 64);
        assert_ne!(secret, signing_secret(b"server", "Endpoint", "22222222-2222-2222-2222-222222222222"), "one secret per endpoint");
        assert_ne!(secret, signing_secret(b"other", "Endpoint", "11111111-1111-1111-1111-111111111111"), "and per server secret");
        let body = br#"{"id":"evt_1"}"#;
        let header = signature_header(&secret, 1000, body);
        assert!(verify_signature(&secret, &header, body, 1100).is_ok());
        assert!(verify_signature(&secret, &header, b"{}", 1100).is_err(), "the body is signed");
        assert!(verify_signature("whsec_other", &header, body, 1100).is_err(), "the key is the endpoint's secret");
        assert!(verify_signature(&secret, &header, body, 1000 + TOLERANCE_SECS + 1).is_err(), "a captured request cannot be replayed later");
        assert!(verify_signature(&secret, &header.replace("t=1000", "t=1001"), body, 1100).is_err(), "the timestamp is signed");
    }

    #[test]
    fn retries_double_and_end_at_the_window() {
        let at = |a| retry_offset_ms(a, 3, 7);
        assert_eq!((at(1), at(2), at(3)), (1000, 3000, 7000));
        assert_eq!(retry_offset_ms(10, 10, 24 * 3600), 24 * 3600 * 1000);
        let waits: Vec<i64> = (1..=5).map(|a| retry_offset_ms(a, 5, 31) - retry_offset_ms(a - 1, 5, 31)).collect();
        assert_eq!(waits, [1000, 2000, 4000, 8000, 16000]);
    }

    #[test]
    fn only_public_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
        ] {
            assert!(!is_public(ip.parse().expect("ip")), "{ip} must be refused");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "172.32.0.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
            assert!(is_public(ip.parse().expect("ip")), "{ip} is public");
        }
    }

    #[tokio::test]
    async fn urls_are_checked_before_a_call() {
        for bad in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "http://127.0.0.1/hook",
            "http://[::1]:8080/hook",
            "http://169.254.169.254/latest/meta-data",
            "http://user:pw@example.com/",
            "http://localhost/hook",
            "not a url",
        ] {
            assert!(check_url(bad, false).await.is_err(), "{bad} must be refused");
        }
        assert!(check_url("http://127.0.0.1:9/hook", true).await.is_ok(), "the explicit development setting allows it");
        assert!(check_url("https://8.8.8.8/hook", false).await.is_ok());
    }
}
