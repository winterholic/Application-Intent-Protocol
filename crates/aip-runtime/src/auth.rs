//! Actor identification. Production uses signed bearer tokens issued by the
//! auth extension; development may pass `x-aip-actor` when explicitly enabled.
//!
//! A token is `aip1.<b64 body>.<b64 HMAC-SHA256 of the body>`. The body is `<actor>.<exp>`, or
//! `<target>.<exp>.<session>` for an impersonation token: the actor is the person acted as, the session names the
//! row in `_aip_impersonation` that the engine checks on every call. The signature covers the whole body, so a
//! token cannot be turned into an impersonation one, or pointed at another session, without the secret.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Who a valid token speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub actor: String,
    /// Set for an impersonation token.
    pub session: Option<String>,
}

fn sign(secret: &[u8], body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(body.as_bytes());
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("aip1.{}.{sig}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body))
}

pub fn issue(secret: &[u8], actor: &str, ttl_seconds: i64) -> String {
    let exp = chrono::Utc::now().timestamp() + ttl_seconds;
    sign(secret, &format!("{actor}.{exp}"))
}

/// A token for `target` that is only good while `session` is open; `exp` is a Unix timestamp.
pub fn issue_impersonation(secret: &[u8], target: &str, session: &str, exp: i64) -> String {
    sign(secret, &format!("{target}.{exp}.{session}"))
}

/// Returns who a valid, unexpired token speaks for.
pub fn verify(secret: &[u8], token: &str) -> Option<Identity> {
    verify_until(secret, token).map(|(id, _)| id)
}

/// Like [`verify`], and also when the token expires (a Unix timestamp), for a connection that outlives one request.
pub fn verify_until(secret: &[u8], token: &str) -> Option<(Identity, i64)> {
    let rest = token.strip_prefix("aip1.")?;
    let (body_b64, sig_b64) = rest.split_once('.')?;
    let body = String::from_utf8(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body_b64).ok()?).ok()?;
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64).ok()?;
    let mut mac = HmacSha256::new_from_slice(secret).ok()?;
    mac.update(body.as_bytes());
    mac.verify_slice(&sig).ok()?;
    let mut parts = body.split('.');
    let (actor, exp, session) = (parts.next()?, parts.next()?, parts.next());
    let exp: i64 = exp.parse().ok()?;
    if parts.next().is_some() || exp < chrono::Utc::now().timestamp() {
        return None;
    }
    uuid::Uuid::parse_str(actor).ok()?;
    if let Some(s) = session {
        uuid::Uuid::parse_str(s).ok()?;
    }
    Some((Identity { actor: actor.to_string(), session: session.map(String::from) }, exp))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "7b0c8a58-8f1f-4a5e-9a57-0c2f2b3f2e11";
    const S: &str = "0a3c1d52-4b8e-4e43-8d0f-5a6f2d9e7c10";

    #[test]
    fn roundtrip_and_tamper() {
        let t = issue(b"k", A, 60);
        assert_eq!(verify(b"k", &t), Some(Identity { actor: A.into(), session: None }));
        assert!(verify(b"other", &t).is_none());
        assert!(verify(b"k", &t.replace("aip1.", "aip1.x")).is_none());
        assert!(verify(b"k", &issue(b"k", A, -1)).is_none());
    }

    #[test]
    fn impersonation_token_carries_its_session_and_cannot_be_forged() {
        let exp = chrono::Utc::now().timestamp() + 60;
        let t = issue_impersonation(b"k", A, S, exp);
        assert_eq!(verify(b"k", &t), Some(Identity { actor: A.into(), session: Some(S.into()) }));
        assert!(verify(b"other", &t).is_none(), "another secret does not verify");
        assert!(verify(b"k", &issue_impersonation(b"k", A, S, chrono::Utc::now().timestamp() - 1)).is_none(), "expired");
        // the same payload re-encoded with another session, or the session dropped, keeps the old signature
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let sig = t.rsplit('.').next().expect("signature");
        let other_session = format!("aip1.{}.{sig}", b64(&format!("{A}.{exp}.{}", "11111111-1111-4111-8111-111111111111")));
        assert!(verify(b"k", &other_session).is_none(), "a swapped session breaks the signature");
        let dropped = format!("aip1.{}.{sig}", b64(&format!("{A}.{exp}")));
        assert!(verify(b"k", &dropped).is_none(), "a token cannot lose its session either");
        let extra = sign(b"k", &format!("{A}.{exp}.{S}.extra"));
        assert!(verify(b"k", &extra).is_none(), "a validly signed body with a fourth part is not a token");
    }
}
