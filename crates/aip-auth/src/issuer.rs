//! Access tokens minted by the service itself, for deployments whose users log in with a password the service checks
//! (WBS 7.2.2). The tokens use exactly the profile `AuthenticatorImpl::verify_token` demands of any identity provider
//! (RS256, `typ: at+jwt`, issuer, audience, `sub`, `iat`, `exp` within 900 seconds), plus a `sid` claim naming the login
//! session. A service that mints tokens for itself must publish the matching public key in its configured JWKS; starting
//! with a signing key the verifier does not trust would mint tokens nobody accepts, so the service checks the round trip.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_ACCESS_TTL_SECS: u32 = 900;

#[derive(Debug, PartialEq, Eq)]
pub enum IssuerError {
    BadKey,
    BadTtl,
    BadClaim,
    Clock,
    Sign,
}

pub struct LocalIssuer {
    key: EncodingKey,
    kid: String,
    issuer: String,
    audience: String,
    ttl_secs: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AccessToken {
    pub token: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    aud: &'a str,
    sub: &'a str,
    sid: &'a str,
    iat: i64,
    nbf: i64,
    exp: i64,
}

/// The body of the first PEM block, for a PKCS#1 (`RSA PRIVATE KEY`) or PKCS#8 (`PRIVATE KEY`) RSA key. Encrypted keys and every
/// other label are refused rather than guessed at.
fn pem_to_der(pem: &[u8]) -> Result<Vec<u8>, IssuerError> {
    let text = std::str::from_utf8(pem).map_err(|_| IssuerError::BadKey)?;
    let (label, rest) = text.trim().strip_prefix("-----BEGIN ").and_then(|rest| rest.split_once("-----")).ok_or(IssuerError::BadKey)?;
    if !matches!(label, "RSA PRIVATE KEY" | "PRIVATE KEY") {
        return Err(IssuerError::BadKey);
    }
    let (body, tail) = rest.split_once("-----END ").ok_or(IssuerError::BadKey)?;
    if !tail.starts_with(label) {
        return Err(IssuerError::BadKey);
    }
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let der = STANDARD.decode(compact).map_err(|_| IssuerError::BadKey)?;
    // `EncodingKey::from_rsa_der` accepts any bytes and wants PKCS#1; reject anything that is not a DER sequence before it is
    // used to sign, and unwrap PKCS#8 (what OpenSSL 3 writes) to the PKCS#1 key inside.
    let (tag, _, rest) = der_item(&der).ok_or(IssuerError::BadKey)?;
    if tag != 0x30 || !rest.is_empty() {
        return Err(IssuerError::BadKey);
    }
    if label == "RSA PRIVATE KEY" {
        return Ok(der);
    }
    pkcs8_inner(&der).map(<[u8]>::to_vec).ok_or(IssuerError::BadKey)
}

/// One DER element: tag, content and the bytes after it. Lengths above 4 bytes of length are refused.
fn der_item(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        (rest[..count].iter().fold(0_usize, |acc, byte| (acc << 8) | usize::from(*byte)), &rest[count..])
    };
    (rest.len() >= length).then(|| (tag, &rest[..length], &rest[length..]))
}

/// The PKCS#1 key inside a PKCS#8 `PrivateKeyInfo`, only when its algorithm is `rsaEncryption`.
fn pkcs8_inner(der: &[u8]) -> Option<&[u8]> {
    const RSA_ENCRYPTION: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    let (_, info, _) = der_item(der)?;
    let (version_tag, version, rest) = der_item(info)?;
    let (algorithm_tag, algorithm, rest) = der_item(rest)?;
    let (key_tag, key, _) = der_item(rest)?;
    let (oid_tag, oid, _) = der_item(algorithm)?;
    (version_tag == 0x02 && version == [0] && algorithm_tag == 0x30 && oid_tag == 0x06 && oid == RSA_ENCRYPTION && key_tag == 0x04).then_some(key)
}

impl LocalIssuer {
    pub fn from_pem(pem: &[u8], kid: &str, issuer: &str, audience: &str, ttl_secs: u32) -> Result<Self, IssuerError> {
        if ttl_secs == 0 || ttl_secs > MAX_ACCESS_TTL_SECS {
            return Err(IssuerError::BadTtl);
        }
        if kid.is_empty() || kid.len() > 128 || issuer.is_empty() || audience.is_empty() {
            return Err(IssuerError::BadClaim);
        }
        let key = EncodingKey::from_rsa_der(&pem_to_der(pem)?);
        Ok(Self { key, kid: kid.to_owned(), issuer: issuer.to_owned(), audience: audience.to_owned(), ttl_secs })
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn ttl_secs(&self) -> u32 {
        self.ttl_secs
    }

    /// `subject` is the identity the service resolved itself and `session` the login session it belongs to. Neither comes
    /// from a request.
    pub fn issue(&self, subject: &str, session: &str) -> Result<AccessToken, IssuerError> {
        if subject.is_empty() || subject.len() > 256 || subject.chars().any(char::is_control) || session.is_empty() || session.len() > 64 {
            return Err(IssuerError::BadClaim);
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| IssuerError::Clock)?.as_secs() as i64;
        let claims =
            Claims { iss: &self.issuer, aud: &self.audience, sub: subject, sid: session, iat: now, nbf: now, exp: now + i64::from(self.ttl_secs) };
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some(self.kid.clone());
        let token = encode(&header, &claims, &self.key).map_err(|_| IssuerError::Sign)?;
        Ok(AccessToken { token, issued_at: now, expires_at: claims.exp })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{AuthError, AuthenticatorImpl, Config, JwksSource};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, pkcs8::LineEnding, traits::PublicKeyParts};

    const ISSUER: &str = "https://login.facereview.example";
    const AUDIENCE: &str = "aip-api";

    fn key_pair() -> (String, serde_json::Value) {
        let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("test RSA key");
        let public = private.to_public_key();
        let jwk = serde_json::json!({"kty":"RSA","kid":"local-1","alg":"RS256","use":"sig",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()), "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())});
        (private.to_pkcs1_pem(LineEnding::LF).expect("PEM").to_string(), jwk)
    }

    async fn verifier(jwk: &serde_json::Value, dir: &tempfile::TempDir) -> AuthenticatorImpl {
        let path = dir.path().join("jwks.json");
        std::fs::write(&path, serde_json::json!({"keys":[jwk]}).to_string()).expect("write JWKS");
        let config = Config { issuer: ISSUER.into(), audience: AUDIENCE.into(), jwks: JwksSource::File { path }, provisioning: Default::default() };
        AuthenticatorImpl::new_explained(config, "host=unused".into(), "aip_app".into()).await.expect("verifier")
    }

    #[tokio::test]
    async fn a_minted_token_passes_the_same_verifier_as_an_identity_provider_token() {
        let (pem, jwk) = key_pair();
        let dir = tempfile::tempdir().unwrap();
        let verifier = verifier(&jwk, &dir).await;
        let issuer = LocalIssuer::from_pem(pem.as_bytes(), "local-1", ISSUER, AUDIENCE, 600).unwrap();
        let minted = issuer.issue("11111111-1111-4111-8111-111111111111", "session-a").unwrap();
        let claims = verifier.verify_token(&minted.token).await.expect("the verifier accepts what the issuer mints");
        assert_eq!((claims.issuer.as_str(), claims.subject.as_str()), (ISSUER, "11111111-1111-4111-8111-111111111111"));
        assert_eq!((claims.issued_at, claims.expires_at), (minted.issued_at, minted.expires_at));
        assert_eq!(claims.expires_at - claims.issued_at, 600);
        assert_eq!(claims.claims.get("sid").and_then(|value| value.as_str()), Some("session-a"));
    }

    #[tokio::test]
    async fn tokens_signed_by_an_untrusted_key_or_for_another_audience_do_not_verify() {
        let (_, trusted_jwk) = key_pair();
        let (other_pem, _) = key_pair();
        let dir = tempfile::tempdir().unwrap();
        let verifier = verifier(&trusted_jwk, &dir).await;
        let forged = LocalIssuer::from_pem(other_pem.as_bytes(), "local-1", ISSUER, AUDIENCE, 60).unwrap().issue("u", "s").unwrap();
        assert_eq!(verifier.verify_token(&forged.token).await, Err(AuthError::Invalid), "same kid, different key");
        let wrong_kid = LocalIssuer::from_pem(other_pem.as_bytes(), "rogue", ISSUER, AUDIENCE, 60).unwrap().issue("u", "s").unwrap();
        assert_eq!(verifier.verify_token(&wrong_kid.token).await, Err(AuthError::Invalid), "unknown kid");
    }

    #[tokio::test]
    async fn pkcs8_keys_sign_and_encrypted_or_foreign_pem_blocks_are_refused() {
        use rsa::pkcs8::EncodePrivateKey;
        let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("test RSA key");
        let public = private.to_public_key();
        let jwk = serde_json::json!({"kty":"RSA","kid":"local-1","alg":"RS256","use":"sig",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()), "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())});
        let pkcs8 = private.to_pkcs8_pem(LineEnding::LF).expect("PKCS#8").to_string();
        assert!(pkcs8.starts_with("-----BEGIN PRIVATE KEY-----"));
        let dir = tempfile::tempdir().unwrap();
        let verifier = verifier(&jwk, &dir).await;
        let minted = LocalIssuer::from_pem(pkcs8.as_bytes(), "local-1", ISSUER, AUDIENCE, 60).unwrap().issue("u", "s").unwrap();
        assert!(verifier.verify_token(&minted.token).await.is_ok());
        for refused in [
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMAA=\n-----END ENCRYPTED PRIVATE KEY-----",
            "-----BEGIN PUBLIC KEY-----\nMAA=\n-----END PUBLIC KEY-----",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
            "-----BEGIN PRIVATE KEY-----\n!!!!\n-----END PRIVATE KEY-----",
            "-----BEGIN PRIVATE KEY-----\nMAA=\n-----END RSA PRIVATE KEY-----",
        ] {
            assert_eq!(LocalIssuer::from_pem(refused.as_bytes(), "k", ISSUER, AUDIENCE, 60).err(), Some(IssuerError::BadKey), "{refused}");
        }
    }

    #[test]
    fn lifetime_and_claims_are_bounded() {
        let (pem, _) = key_pair();
        for ttl in [0, MAX_ACCESS_TTL_SECS + 1] {
            assert_eq!(LocalIssuer::from_pem(pem.as_bytes(), "k", ISSUER, AUDIENCE, ttl).err(), Some(IssuerError::BadTtl));
        }
        assert_eq!(LocalIssuer::from_pem(b"not a key", "k", ISSUER, AUDIENCE, 60).err(), Some(IssuerError::BadKey));
        assert_eq!(LocalIssuer::from_pem(pem.as_bytes(), "", ISSUER, AUDIENCE, 60).err(), Some(IssuerError::BadClaim));
        let issuer = LocalIssuer::from_pem(pem.as_bytes(), "k", ISSUER, AUDIENCE, 900).unwrap();
        assert_eq!(issuer.issue("", "s").err(), Some(IssuerError::BadClaim));
        assert_eq!(issuer.issue("u\n", "s").err(), Some(IssuerError::BadClaim));
        assert_eq!(issuer.issue("u", "").err(), Some(IssuerError::BadClaim));
    }
}
