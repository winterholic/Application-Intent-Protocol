//! Server-configured triggers that start durable jobs without a caller session.
//!
//! Both schedules and inbound webhooks run as an operator-bound principal (`service principal bind`).
//! The job then passes the same deployment fence, principal check, operation `allow` and capability
//! limits as a job a user enqueued, so a trigger never widens what the server may execute.
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use spike_v2_read::id_wire::IdWire;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ServicePrincipal {
    pub issuer: String,
    pub subject: String,
}

pub struct Schedule {
    pub name: String,
    pub operation: String,
    pub cron: crate::schedule::Cron,
    pub input: Value,
    pub principal: ServicePrincipal,
    /// Slots older than this are not caught up after downtime; only the latest slot is considered.
    pub misfire_grace: Duration,
}

pub enum Signature {
    /// Generic HMAC-SHA256 over the raw body, e.g. GitHub `x-hub-signature-256: sha256=<hex>` or Shopify base64.
    /// Only the body is signed, so the delivery identity is the body digest; unsigned headers never feed the key.
    HmacSha256 { header: String, prefix: String, base64: bool },
    /// Stripe `stripe-signature: t=<unix>,v1=<hex>` over `<t>.<body>`.
    Stripe { tolerance: Duration },
    /// Standard Webhooks (`webhook-id`, `webhook-timestamp`, `webhook-signature: v1,<base64>`) over `<id>.<t>.<body>`.
    StandardWebhooks { tolerance: Duration },
}

pub struct Webhook {
    pub name: String,
    pub path: String,
    pub operation: String,
    pub principal: ServicePrincipal,
    pub secret: Vec<u8>,
    pub signature: Signature,
    pub max_body_bytes: usize,
}

/// Delivers committed `notify` outbox rows of one topic to a durable job.
pub struct OutboxHandler {
    pub topic: String,
    pub operation: String,
    pub principal: ServicePrincipal,
}

/// Per-operation retry policy for durable jobs, set in service configuration like Celery `max_retries`
/// or Sidekiq `retry:`. Every retry replays the effect journal, so it never repeats a recorded effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JobPolicy {
    /// Total attempts including the first, 1..=10.
    pub max_attempts: i32,
    /// Also retry an uncaught extension error (`EXTENSION_ERROR`), not only `ctx.retryLater()`.
    pub retry_uncaught: bool,
}

impl Default for JobPolicy {
    fn default() -> Self {
        Self { max_attempts: 3, retry_uncaught: false }
    }
}

pub const MAX_JOB_ATTEMPTS: i32 = 10;

/// Rejects a policy for an operation that is not a declared job or an attempt count the store refuses.
pub fn validate_job_policies(policies: &std::collections::HashMap<String, JobPolicy>, facts: &Value) -> Result<(), &'static str> {
    for (operation, policy) in policies {
        if facts["operations"][operation]["kind"] != "job" {
            return Err("job policy operation must be a declared durable job");
        }
        if !(1..=MAX_JOB_ATTEMPTS).contains(&policy.max_attempts) {
            return Err("job policy max_attempts must be between 1 and 10");
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct Triggers {
    pub schedules: Vec<Schedule>,
    pub webhooks: Vec<Webhook>,
    pub outbox: Vec<OutboxHandler>,
}

pub const MAX_WEBHOOK_BODY: usize = 1 << 20;
const MAX_TOLERANCE: Duration = Duration::from_secs(3600);
pub const MAX_MISFIRE_GRACE: Duration = Duration::from_secs(86_400);
/// At least two schedule evaluation ticks, so a slot cannot fall between evaluations.
pub const MIN_MISFIRE_GRACE: Duration = Duration::from_secs(10);

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Triggers {
    pub fn is_empty(&self) -> bool {
        self.schedules.is_empty() && self.webhooks.is_empty() && self.outbox.is_empty()
    }

    /// Rejects configuration that could never run before the listener accepts traffic.
    pub fn validate(&self, facts: &Value, wire: IdWire, reserved: &[&str]) -> Result<(), &'static str> {
        let mut names = std::collections::HashSet::new();
        for schedule in &self.schedules {
            if !valid_name(&schedule.name) || !names.insert(format!("s:{}", schedule.name)) {
                return Err("schedule name must be a unique ASCII identifier of at most 40 bytes");
            }
            if spike_v4_worker::validate_job(facts, &schedule.operation, &schedule.input, wire).is_err() {
                return Err("schedule operation must be a declared job whose input matches the configured input");
            }
            if schedule.misfire_grace < MIN_MISFIRE_GRACE || schedule.misfire_grace > MAX_MISFIRE_GRACE {
                return Err("schedule misfire grace must be between 10 seconds and 24 hours");
            }
            if schedule.principal.subject.is_empty() || schedule.principal.subject.len() > 256 {
                return Err("schedule principal subject is invalid");
            }
        }
        let mut topics = std::collections::HashSet::new();
        for handler in &self.outbox {
            if handler.topic.is_empty()
                || handler.topic.len() > 200
                || handler.topic.chars().any(char::is_control)
                || !topics.insert(handler.topic.clone())
            {
                return Err("outbox handler topic must be unique, non-empty and at most 200 bytes");
            }
            if !declares_notify_topic(facts, &handler.topic) {
                return Err("outbox handler topic must be emitted by a declared notify effect");
            }
            if spike_v4_worker::validate_job(facts, &handler.operation, &json!({"event":"{}"}), wire).is_err() {
                return Err("outbox handler operation must be a declared job whose input is exactly { event: Text }");
            }
            if handler.principal.subject.is_empty() || handler.principal.subject.len() > 256 {
                return Err("outbox handler principal subject is invalid");
            }
        }
        let mut paths = std::collections::HashSet::new();
        for webhook in &self.webhooks {
            if !valid_name(&webhook.name) || !names.insert(format!("w:{}", webhook.name)) {
                return Err("webhook name must be a unique ASCII identifier of at most 40 bytes");
            }
            let path_ok = webhook.path.starts_with('/')
                && webhook.path.len() <= 200
                && webhook.path[1..].split('/').all(valid_name)
                && !reserved.iter().any(|prefix| webhook.path == *prefix || webhook.path.starts_with(&format!("{prefix}/")));
            if !path_ok || !paths.insert(webhook.path.clone()) {
                return Err("webhook path must be a unique /segment path outside AIP routes and mounts");
            }
            if spike_v4_worker::validate_job(facts, &webhook.operation, &json!({"body":"{}"}), wire).is_err() {
                return Err("webhook operation must be a declared job whose input is exactly { body: Text }");
            }
            if webhook.secret.len() < 16 || webhook.max_body_bytes == 0 || webhook.max_body_bytes > MAX_WEBHOOK_BODY {
                return Err("webhook secret must be at least 16 bytes and body limit between 1 byte and 1 MiB");
            }
            if let Signature::Stripe { tolerance } | Signature::StandardWebhooks { tolerance } = &webhook.signature {
                if tolerance.is_zero() || *tolerance > MAX_TOLERANCE {
                    return Err("webhook timestamp tolerance must be between 1 second and 1 hour");
                }
            }
            if webhook.principal.subject.is_empty() || webhook.principal.subject.len() > 256 {
                return Err("webhook principal subject is invalid");
            }
        }
        Ok(())
    }
}

fn declares_notify_topic(facts: &Value, topic: &str) -> bool {
    facts["resources"].as_object().is_some_and(|resources| {
        resources.values().any(|resource| {
            resource["transitions"].as_object().is_some_and(|transitions| {
                transitions.values().any(|transition| {
                    transition["effects"].as_array().is_some_and(|effects| effects.iter().any(|effect| effect["notify"]["topic"] == topic))
                })
            })
        })
    })
}

/// Prefixes of the keys below. User-submitted jobs may not use them.
pub const RESERVED_KEY_PREFIXES: [&str; 3] = ["outbox:", "schedule:", "webhook:"];

/// The job key for one committed outbox row; every instance derives the same key.
pub fn outbox_key(id: i64, created_micros: i64) -> String {
    format!("outbox:{id}:{created_micros}")
}

/// The job key for one schedule slot. Every instance derives the same key for the same slot.
pub fn schedule_key(name: &str, slot_unix: i64) -> String {
    format!("schedule:{name}:{slot_unix}")
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let mut found = headers.iter().filter(|(key, _)| key == name);
    let first = found.next()?;
    // Duplicate signature headers are ambiguous; refuse rather than pick one.
    found.next().is_none().then_some(first.1.as_str())
}

fn mac(secret: &[u8], parts: &[&[u8]]) -> Hmac<Sha256> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts any key length");
    for part in parts {
        mac.update(part);
    }
    mac
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    Signature,
    Stale,
}

/// Verifies the request signature and returns the delivery identity used for the job idempotency key.
/// `now_unix` comes from the server clock and bounds replay of captured requests for timestamped schemes.
pub fn verify(webhook: &Webhook, headers: &[(String, String)], body: &[u8], now_unix: i64) -> Result<String, Rejection> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let within = |timestamp: &str, tolerance: &Duration| -> Result<(), Rejection> {
        let t = timestamp.parse::<i64>().map_err(|_| Rejection::Signature)?;
        if now_unix.abs_diff(t) > tolerance.as_secs() {
            return Err(Rejection::Stale);
        }
        Ok(())
    };
    let body_digest = || hex(&Sha256::digest(body));
    match &webhook.signature {
        Signature::HmacSha256 { header: name, prefix, base64 } => {
            let value = header(headers, name).ok_or(Rejection::Signature)?;
            let encoded = value.strip_prefix(prefix.as_str()).ok_or(Rejection::Signature)?;
            let expected = if *base64 { STANDARD.decode(encoded).ok() } else { unhex(encoded) }.ok_or(Rejection::Signature)?;
            mac(&webhook.secret, &[body]).verify_slice(&expected).map_err(|_| Rejection::Signature)?;
            Ok(body_digest())
        }
        Signature::Stripe { tolerance } => {
            let value = header(headers, "stripe-signature").ok_or(Rejection::Signature)?;
            let mut timestamp = None;
            let mut candidates = Vec::new();
            for item in value.split(',') {
                match item.trim().split_once('=') {
                    Some(("t", t)) if timestamp.is_none() => timestamp = Some(t),
                    Some(("t", _)) => return Err(Rejection::Signature),
                    Some(("v1", signature)) => candidates.push(signature),
                    _ => {}
                }
            }
            let timestamp = timestamp.ok_or(Rejection::Signature)?;
            let signed = mac(&webhook.secret, &[timestamp.as_bytes(), b".", body]);
            // Stripe sends one v1 per active secret during rotation; any match authenticates.
            if !candidates.iter().filter_map(|c| unhex(c)).any(|expected| signed.clone().verify_slice(&expected).is_ok()) {
                return Err(Rejection::Signature);
            }
            within(timestamp, tolerance)?;
            Ok(body_digest())
        }
        Signature::StandardWebhooks { tolerance } => {
            let id = header(headers, "webhook-id").ok_or(Rejection::Signature)?;
            let timestamp = header(headers, "webhook-timestamp").ok_or(Rejection::Signature)?;
            let value = header(headers, "webhook-signature").ok_or(Rejection::Signature)?;
            let signed = mac(&webhook.secret, &[id.as_bytes(), b".", timestamp.as_bytes(), b".", body]);
            let matched = value
                .split(' ')
                .filter_map(|item| item.strip_prefix("v1,"))
                .filter_map(|encoded| STANDARD.decode(encoded).ok())
                .any(|expected| signed.clone().verify_slice(&expected).is_ok());
            if !matched {
                return Err(Rejection::Signature);
            }
            within(timestamp, tolerance)?;
            Ok(hex(&Sha256::digest(id.as_bytes())))
        }
    }
}

/// Job key for one delivery. Truncated SHA-256 keeps the key inside the 100-byte job key limit.
pub fn webhook_key(name: &str, delivery: &str) -> String {
    format!("webhook:{name}:{}", &delivery[..40])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.len() > 256 {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// Standard Webhooks secrets are distributed as `whsec_<base64>`; other schemes use the raw value.
pub fn decode_secret(signature: &Signature, raw: &str) -> Option<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    match signature {
        Signature::StandardWebhooks { .. } => STANDARD.decode(raw.strip_prefix("whsec_")?).ok(),
        _ => Some(raw.as_bytes().to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};

    fn webhook(signature: Signature) -> Webhook {
        Webhook {
            name: "hook".into(),
            path: "/hooks/test".into(),
            operation: "receive".into(),
            principal: ServicePrincipal { issuer: "i".into(), subject: "svc".into() },
            secret: b"0123456789abcdef-secret".to_vec(),
            signature,
            max_body_bytes: 1024,
        }
    }
    fn sign(secret: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        mac(secret, parts).finalize().into_bytes().to_vec()
    }
    fn headers(pairs: &[(&str, String)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn generic_hmac_accepts_only_the_exact_body_and_keys_on_its_digest() {
        let hook = webhook(Signature::HmacSha256 { header: "x-hub-signature-256".into(), prefix: "sha256=".into(), base64: false });
        let body = br#"{"action":"opened"}"#;
        let good = format!("sha256={}", hex(&sign(&hook.secret, &[body])));
        // A spoofed delivery header cannot mint a new identity for a captured request.
        let ok = verify(&hook, &headers(&[("x-hub-signature-256", good.clone()), ("x-github-delivery", "d-1".into())]), body, 0).unwrap();
        let spoofed = verify(&hook, &headers(&[("x-hub-signature-256", good.clone()), ("x-github-delivery", "d-2".into())]), body, 0).unwrap();
        assert_eq!((ok.clone(), spoofed), (hex(&Sha256::digest(body)), ok));
        assert_eq!(verify(&hook, &headers(&[("x-hub-signature-256", good.clone())]), br#"{"action":"closed"}"#, 0), Err(Rejection::Signature));
        assert_eq!(verify(&hook, &headers(&[]), body, 0), Err(Rejection::Signature));
        let duplicated = headers(&[("x-hub-signature-256", good.clone()), ("x-hub-signature-256", good)]);
        assert_eq!(verify(&hook, &duplicated, body, 0), Err(Rejection::Signature));
    }

    #[test]
    fn stripe_checks_signature_before_timestamp_and_accepts_rotated_secrets() {
        let hook = webhook(Signature::Stripe { tolerance: Duration::from_secs(300) });
        let body = br#"{"id":"evt_1"}"#;
        let signature = hex(&sign(&hook.secret, &[b"1000.", body]));
        let value = format!("t=1000,v1={},v1={signature}", "00".repeat(32));
        assert!(verify(&hook, &headers(&[("stripe-signature", value.clone())]), body, 1200).is_ok());
        assert_eq!(verify(&hook, &headers(&[("stripe-signature", value.clone())]), body, 1400), Err(Rejection::Stale));
        let forged = format!("t=1300,v1={signature}");
        assert_eq!(verify(&hook, &headers(&[("stripe-signature", forged)]), body, 1300), Err(Rejection::Signature));
    }

    #[test]
    fn standard_webhooks_bind_id_timestamp_and_body() {
        let hook = webhook(Signature::StandardWebhooks { tolerance: Duration::from_secs(300) });
        let body = br#"{"type":"user.created"}"#;
        let signature = STANDARD.encode(sign(&hook.secret, &[b"msg_1.1000.", body]));
        let good =
            headers(&[("webhook-id", "msg_1".into()), ("webhook-timestamp", "1000".into()), ("webhook-signature", format!("v1,bad v1,{signature}"))]);
        assert_eq!(verify(&hook, &good, body, 1000).unwrap(), hex(&Sha256::digest(b"msg_1")));
        let other_id =
            headers(&[("webhook-id", "msg_2".into()), ("webhook-timestamp", "1000".into()), ("webhook-signature", format!("v1,{signature}"))]);
        assert_eq!(verify(&hook, &other_id, body, 1000), Err(Rejection::Signature));
        assert_eq!(decode_secret(&hook.signature, "whsec_MDEyMzQ1Njc4OWFiY2RlZg=="), Some(b"0123456789abcdef".to_vec()));
        assert_eq!(decode_secret(&hook.signature, "MDEy"), None);
        assert!(webhook_key("hook", &hex(&Sha256::digest(b"x"))).len() <= 100);
    }
}
