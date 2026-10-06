//! V8 인증(실험): segment = base64url(payload JSON). 토큰 = segment "." base64url(HMAC-SHA256(key, segment의 바이트)).
//! payload = { sub: actor id, iat, exp }(초). 키는 서버 기동 때 무작위로 만들고 코드·설정에 두지 않는다.
//! 토큰이 있는데 검증에 실패하면 익명으로 낮추지 않고 거부한다.
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct Keyring {
    key: [u8; 32],
}

#[derive(Debug, PartialEq)]
pub enum AuthError {
    Malformed,
    BadSignature,
    Expired,
    Unavailable,
}

pub trait Authenticator: Send + Sync {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>>;
}

pub struct Session {
    pub actor_id: i64,
    expires_at: i64,
}

impl Session {
    pub fn new(actor_id: i64, expires_at: i64) -> Result<Self, AuthError> {
        if actor_id <= 0 {
            return Err(AuthError::Malformed);
        }
        if expires_at <= now_secs() {
            return Err(AuthError::Expired);
        }
        Ok(Self { actor_id, expires_at })
    }
    pub fn remaining_ms(&self) -> u64 {
        let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else { return 0 };
        let expiry = (self.expires_at as i128) * 1000;
        (expiry - now.as_millis() as i128).clamp(0, u64::MAX as i128) as u64
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

impl Keyring {
    pub fn generate() -> Keyring {
        let mut key = [0u8; 32];
        getrandom::getrandom(&mut key).expect("난수");
        Keyring { key }
    }

    fn mac(&self, data: &[u8]) -> HmacSha256 {
        let mut m = HmacSha256::new_from_slice(&self.key).unwrap();
        m.update(data);
        m
    }

    /// 로그인 제공자가 하는 일을 흉내 낸다(실험). 실제 신원 확인은 범위 밖.
    pub fn issue(&self, actor: i64, ttl_secs: i64) -> String {
        let iat = now_secs();
        let payload = B64.encode(json!({ "sub": actor, "iat": iat, "exp": iat + ttl_secs }).to_string());
        let sig = B64.encode(self.mac(payload.as_bytes()).finalize().into_bytes());
        format!("{payload}.{sig}")
    }

    pub fn verify(&self, token: &str) -> Result<i64, AuthError> {
        self.verify_session(token).map(|session| session.actor_id)
    }

    pub fn verify_session(&self, token: &str) -> Result<Session, AuthError> {
        let (payload, sig) = token.split_once('.').ok_or(AuthError::Malformed)?;
        let sig = B64.decode(sig).map_err(|_| AuthError::Malformed)?;
        // 서명 비교는 상수 시간(verify_slice).
        self.mac(payload.as_bytes()).verify_slice(&sig).map_err(|_| AuthError::BadSignature)?;
        let claims: Value = serde_json::from_slice(&B64.decode(payload).map_err(|_| AuthError::Malformed)?).map_err(|_| AuthError::Malformed)?;
        let (sub, exp) = (claims["sub"].as_i64().ok_or(AuthError::Malformed)?, claims["exp"].as_i64().ok_or(AuthError::Malformed)?);
        if exp <= now_secs() {
            return Err(AuthError::Expired);
        }
        Ok(Session { actor_id: sub, expires_at: exp })
    }
}
