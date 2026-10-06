//! Inbound webhooks: verify, deduplicate, store, acknowledge. Handlers run later
//! from the outbox (see `dispatch`), so a slow or failing handler never makes
//! the provider time out, and a provider retry of the same event is a no-op.

use crate::engine::Engine;
use crate::error::AipError;
use aip_ir::codes;
use aip_plan::Webhook;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

/// Stripe's documented default replay window.
const TOLERANCE_SECS: i64 = 300;

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Constant-time check of a hex HMAC-SHA256 of `message`.
fn hmac_matches(secret: &[u8], message: &[u8], hex_sig: &str) -> bool {
    let (Some(sig), Ok(mut mac)) = (hex_decode(hex_sig.trim()), HmacSha256::new_from_slice(secret)) else { return false };
    mac.update(message);
    mac.verify_slice(&sig).is_ok()
}

pub fn verify(w: &Webhook, secret: &[u8], headers: &HashMap<String, String>, body: &[u8], now: i64) -> Result<(), String> {
    let header = headers.get(&w.header.to_ascii_lowercase()).ok_or_else(|| format!("missing {} header", w.header))?;
    match w.scheme.as_str() {
        "stripe" => {
            let mut t = None;
            let mut sigs = Vec::new();
            for part in header.split(',') {
                match part.trim().split_once('=') {
                    Some(("t", v)) => t = v.parse::<i64>().ok(),
                    // only v1 is a live scheme; anything else is ignored to prevent downgrades
                    Some(("v1", v)) => sigs.push(v.to_string()),
                    _ => {}
                }
            }
            let t = t.ok_or("signature header has no timestamp")?;
            if (now - t).abs() > TOLERANCE_SECS {
                return Err("signature timestamp outside the tolerance window".into());
            }
            let mut signed = format!("{t}.").into_bytes();
            signed.extend_from_slice(body);
            if sigs.iter().any(|s| hmac_matches(secret, &signed, s)) { Ok(()) } else { Err("no matching signature".into()) }
        }
        _ => {
            let sig = header.strip_prefix("sha256=").unwrap_or(header);
            if hmac_matches(secret, body, sig) { Ok(()) } else { Err("signature mismatch".into()) }
        }
    }
}

pub fn at_path<'v>(v: &'v Value, path: &str) -> Option<&'v Value> {
    path.split('.').filter(|p| !p.is_empty()).try_fold(v, |cur, key| cur.get(key))
}

/// Handles one delivery. Returns the acknowledgement body.
pub async fn receive(engine: &Engine, name: &str, headers: &HashMap<String, String>, body: &[u8]) -> Result<Value, AipError> {
    let intent = format!("webhook:{name}");
    let w = engine.program.webhooks.iter().find(|w| w.name == name).ok_or_else(|| AipError::new(codes::NOT_FOUND, &intent, "no such webhook"))?;
    let secret = std::env::var(&w.secret_env).ok().filter(|s| !s.is_empty()).ok_or_else(|| {
        tracing::error!(webhook = name, env = %w.secret_env, "webhook secret is not configured");
        AipError::new(codes::INTERNAL, &intent, "webhook is not configured")
    })?;
    let now = chrono::Utc::now().timestamp();
    if let Err(why) = verify(w, secret.as_bytes(), headers, body, now) {
        tracing::warn!(webhook = name, reason = %why, "webhook rejected");
        return Err(AipError::new(codes::AUTH_UNAUTHENTICATED, &intent, "invalid webhook signature"));
    }
    let doc: Value = serde_json::from_slice(body).map_err(|_| AipError::new(codes::REQUEST_MALFORMED, &intent, "body must be JSON"))?;
    let text = |p: &str| at_path(&doc, p).and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string())));
    let event_id = text(&w.id_path).ok_or_else(|| AipError::new(codes::REQUEST_MALFORMED, &intent, format!("no event id at '{}'", w.id_path)))?;
    let event =
        text(&w.event_path).ok_or_else(|| AipError::new(codes::REQUEST_MALFORMED, &intent, format!("no event type at '{}'", w.event_path)))?;
    let payload = at_path(&doc, &w.payload_path).cloned().unwrap_or(Value::Null);
    let handled = w.handlers.iter().any(|h| h.event == event);
    let mut client = engine.pool.get().await.map_err(|e| AipError::new(codes::INTERNAL, &intent, e.to_string()))?;
    let tx = client.transaction().await.map_err(|e| crate::exec::db_error(&engine.program, &intent, &e))?;
    let fresh = tx
        .query_opt(
            "INSERT INTO \"_aip_webhook_seen\" (\"webhook\", \"event_id\") VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING 1",
            &[&w.name, &event_id],
        )
        .await
        .map_err(|e| crate::exec::db_error(&engine.program, &intent, &e))?
        .is_some();
    if fresh && handled {
        let row = json!({"event": event, "id": event_id, "data": payload});
        tx.execute(
            "INSERT INTO \"_aip_outbox\" (\"kind\", \"name\", \"key\", \"payload\") VALUES ('webhook', $1, $2, $3)",
            &[&w.name, &event_id, &row],
        )
        .await
        .map_err(|e| crate::exec::db_error(&engine.program, &intent, &e))?;
    }
    tx.commit().await.map_err(|e| crate::exec::db_error(&engine.program, &intent, &e))?;
    Ok(json!({"received": true, "duplicate": !fresh, "handled": handled}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(scheme: &str, header: &str) -> Webhook {
        Webhook {
            name: "W".into(),
            source: "s".into(),
            scheme: scheme.into(),
            header: header.into(),
            secret_env: "X".into(),
            event_path: "type".into(),
            id_path: "id".into(),
            payload_path: "data".into(),
            handlers: Vec::new(),
        }
    }

    fn sign(secret: &[u8], msg: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).expect("key");
        mac.update(msg);
        mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn stripe_scheme() {
        let w = hook("stripe", "Stripe-Signature");
        let body = br#"{"id":"evt_1"}"#;
        let good = sign(b"whsec", &[b"1000.".as_slice(), body].concat());
        let h = |v: String| HashMap::from([("stripe-signature".to_string(), v)]);
        assert!(verify(&w, b"whsec", &h(format!("t=1000,v1=deadbeef,v1={good},v0=00")), body, 1100).is_ok(), "any v1 may match (secret rolling)");
        assert!(verify(&w, b"whsec", &h(format!("t=1000,v0={good}")), body, 1100).is_err(), "v0 is ignored");
        assert!(verify(&w, b"whsec", &h(format!("t=1000,v1={good}")), body, 1000 + 301).is_err(), "replay window");
        assert!(verify(&w, b"whsec", &h(format!("t=1001,v1={good}")), body, 1100).is_err(), "timestamp is signed");
    }

    #[test]
    fn hmac_scheme() {
        let w = hook("hmac-sha256", "X-Signature");
        let body = b"{}";
        let sig = sign(b"k", body);
        assert!(verify(&w, b"k", &HashMap::from([("x-signature".into(), format!("sha256={sig}"))]), body, 0).is_ok());
        assert!(verify(&w, b"k", &HashMap::from([("x-signature".into(), sig)]), b"{ }", 0).is_err());
        assert!(verify(&w, b"k", &HashMap::new(), body, 0).is_err());
    }
}
