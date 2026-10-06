//! Field encryption: the runtime encrypts a value just before it is bound to a statement and decrypts it just before
//! an answer leaves, so the database only ever holds ciphertext (`encrypted` fields).
//!
//! A stored value is `v1:<key id>:<base64 nonce>:<base64 ciphertext and tag>`: AES-256-GCM with a random 96-bit nonce
//! per value. The key id says which of the configured keys sealed it, so keys can rotate without a flag day. The
//! associated data is `aip.enc.v1|<Entity>.<field>|<row id>`: a ciphertext copied to another row, or to another field,
//! fails authentication instead of being read as that row's value. The names are the program's, not the tables':
//! the published copy and the history versions of a row keep its id and decrypt as the row does.
//!
//! Keys come from `AIP_ENCRYPTION_KEYS` (`k2:<base64 of 32 bytes>,k1:<...>`): the first key encrypts, every key decrypts.
//! It is deliberately not `AIP_SECRET`: the signing secret and the data keys have different owners and rotate separately.

use crate::error::AipError;
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, Generate, KeyInit, Nonce, Payload};
use aip_ir::codes;
use aip_plan::{DecryptPath, Program};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use serde_json::Value;
use std::sync::Arc;

pub const ENV_KEYS: &str = "AIP_ENCRYPTION_KEYS";
const VERSION: &str = "v1";

struct Entry {
    id: String,
    cipher: Aes256Gcm,
    /// Only for [`Keys::fingerprint`].
    raw: [u8; 32],
}

pub struct Keys {
    entries: Vec<Entry>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the key material
        write!(f, "Keys({})", self.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "))
    }
}

/// Why a stored value could not be opened. Never carries the value.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Not `v1:<id>:<nonce>:<ciphertext>`: plaintext, or something else wrote the column.
    Format,
    /// Written under a key that is not configured.
    UnknownKey(String),
    /// The key is right and the data is not: changed, or copied from another row or field.
    Authentication(String),
}

impl Keys {
    /// `id:<base64 of 32 bytes>` entries separated by commas; the first one encrypts.
    pub fn parse(spec: &str) -> Result<Keys, String> {
        let mut entries: Vec<Entry> = Vec::new();
        for (i, part) in spec.split(',').map(str::trim).filter(|p| !p.is_empty()).enumerate() {
            let (id, b64) = part.split_once(':').ok_or_else(|| format!("entry {} is not `<id>:<base64 key>`", i + 1))?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(format!("entry {}: the key id must be letters, digits, `_` or `-`", i + 1));
            }
            if entries.iter().any(|e| e.id == id) {
                return Err(format!("key id `{id}` appears twice"));
            }
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|_| format!("key `{id}` is not valid base64"))?;
            if raw.len() != 32 {
                return Err(format!("key `{id}` is {} bytes, AES-256 needs 32", raw.len()));
            }
            let raw: [u8; 32] = raw.as_slice().try_into().map_err(|_| format!("key `{id}` is not 32 bytes"))?;
            entries.push(Entry { id: id.to_string(), cipher: Aes256Gcm::new_from_slice(&raw).map_err(|_| format!("key `{id}` is not 32 bytes"))?, raw });
        }
        if entries.is_empty() {
            return Err("no key given".into());
        }
        Ok(Keys { entries })
    }

    /// Id of the key new values are encrypted with.
    pub fn current(&self) -> &str {
        &self.entries[0].id
    }

    pub fn ids(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.id.as_str()).collect()
    }

    fn aad(field: &str, row: &str) -> Vec<u8> {
        format!("aip.enc.{VERSION}|{field}|{row}").into_bytes()
    }

    pub fn encrypt(&self, field: &str, row: &str, plaintext: &str) -> String {
        let e = &self.entries[0];
        let nonce = Nonce::<Aes256Gcm>::generate();
        let sealed = e
            .cipher
            .encrypt(&nonce, Payload { msg: plaintext.as_bytes(), aad: &Self::aad(field, row) })
            // AES-GCM seals any message this size; the only failure is a message of more than 2^36 bytes
            .expect("AES-GCM encryption of a field value");
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        format!("{VERSION}:{}:{}:{}", e.id, b64(nonce.as_slice()), b64(&sealed))
    }

    pub fn decrypt(&self, field: &str, row: &str, stored: &str) -> Result<String, OpenError> {
        let mut parts = stored.splitn(4, ':');
        let (Some(VERSION), Some(id), Some(nonce), Some(body)) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(OpenError::Format);
        };
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| OpenError::Format);
        let (nonce, body) = (b64(nonce)?, b64(body)?);
        let nonce = Nonce::<Aes256Gcm>::try_from(nonce.as_slice()).map_err(|_| OpenError::Format)?;
        let entry = self.entries.iter().find(|e| e.id == id).ok_or_else(|| OpenError::UnknownKey(id.to_string()))?;
        let plain = entry
            .cipher
            .decrypt(&nonce, Payload { msg: &body, aad: &Self::aad(field, row) })
            .map_err(|_| OpenError::Authentication(id.to_string()))?;
        String::from_utf8(plain).map_err(|_| OpenError::Authentication(id.to_string()))
    }

    /// A keyed digest of a value that is bound for an encrypted field, for places that only need to tell two inputs apart
    /// (the idempotency table): a plain hash of an email can be guessed offline, this cannot without the key.
    pub fn fingerprint(&self, value: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.entries[0].raw).expect("hmac accepts any key length");
        mac.update(b"aip.enc.fingerprint.v1|");
        mac.update(value.as_bytes());
        format!("enc:{:x}", mac.finalize().into_bytes())
    }

    /// Whether `stored` was sealed with the key new values use.
    pub fn is_current(&self, stored: &str) -> bool {
        stored.split(':').nth(1).is_some_and(|id| id == self.current())
    }
}

/// Whether the program stores anything encrypted.
pub fn uses_encryption(program: &Program) -> bool {
    program.entities.values().any(|e| e.columns.iter().any(|c| c.encrypted))
}

/// The keys a server needs for `program`. A program with encrypted fields refuses to start without usable keys:
/// continuing would either fail every write or, worse, be mistaken for protection.
pub fn load(program: &Program, spec: Option<&str>) -> Result<Option<Arc<Keys>>, AipError> {
    if !uses_encryption(program) {
        return Ok(None);
    }
    let fail = |why: String| {
        AipError::new(
            codes::ENCRYPTION_KEYS_MISSING,
            "startup",
            format!("the program has encrypted fields but {ENV_KEYS} is unusable: {why}; `aip explain-code {}` says how to set it", codes::ENCRYPTION_KEYS_MISSING),
        )
    };
    match spec.filter(|s| !s.trim().is_empty()) {
        Some(s) => Keys::parse(s).map(|k| Some(Arc::new(k))).map_err(fail),
        None => Err(fail("it is not set".into())),
    }
}

fn open_failed(intent: &str, field: &str, e: OpenError) -> AipError {
    let why = match e {
        OpenError::Format => "the stored value is not in the encrypted format".to_string(),
        OpenError::UnknownKey(id) => format!("it was written with key `{id}`, which is not in {ENV_KEYS}"),
        OpenError::Authentication(id) => format!("it failed authentication under key `{id}`: changed, or moved from another row or field"),
    };
    tracing::error!(intent, field, "{why}");
    AipError::new(codes::ENCRYPTION_DECRYPT_FAILED, intent, format!("{field} cannot be decrypted: {why}")).reason(field.to_string())
}

/// `mask.email` and `mask.phone` of a plaintext, the same results the database gives for a field that is not encrypted.
/// Another mask yields nothing, as it does there.
fn mask(name: &str, plain: &str) -> Option<String> {
    let chars: Vec<char> = plain.chars().collect();
    match name {
        // ^(.).*(@.*)$ -> \1***\2, the last `@` after the first character; no match leaves the text as it is
        "mask.email" => match chars.iter().rposition(|c| *c == '@').filter(|i| *i >= 1) {
            Some(i) => Some(format!("{}***{}", chars[0], chars[i..].iter().collect::<String>())),
            None => Some(plain.to_string()),
        },
        // ^(\d{3}).*(\d{4})$ -> \1-****-\2
        "mask.phone" => {
            if chars.len() >= 7 && chars[..3].iter().all(char::is_ascii_digit) && chars[chars.len() - 4..].iter().all(char::is_ascii_digit) {
                Some(format!("{}-****-{}", chars[..3].iter().collect::<String>(), chars[chars.len() - 4..].iter().collect::<String>()))
            } else {
                Some(plain.to_string())
            }
        }
        _ => None,
    }
}

/// Replaces the `{"c": ciphertext, "i": row id, "m": mask?}` objects the plan says are at `paths` with the plaintext
/// (masked when asked to). Only those places are touched: a string that merely looks like ciphertext elsewhere is data.
pub fn open_result(keys: Option<&Keys>, intent: &str, paths: &[DecryptPath], value: &mut Value) -> Result<(), AipError> {
    for p in paths {
        let Some(keys) = keys else {
            return Err(AipError::new(codes::ENCRYPTION_KEYS_MISSING, intent, format!("{} is encrypted and the server has no keys", p.field)));
        };
        open_at(keys, intent, &p.field, &p.path, value)?;
    }
    Ok(())
}

fn open_at(keys: &Keys, intent: &str, field: &str, path: &[String], v: &mut Value) -> Result<(), AipError> {
    match path.split_first() {
        None => open_cell(keys, intent, field, v),
        Some((seg, rest)) if seg == "[]" => match v {
            Value::Array(items) => items.iter_mut().try_for_each(|it| open_at(keys, intent, field, rest, it)),
            _ => Ok(()),
        },
        Some((seg, rest)) => match v {
            Value::Object(m) => match m.get_mut(seg) {
                Some(child) => open_at(keys, intent, field, rest, child),
                None => Ok(()),
            },
            _ => Ok(()),
        },
    }
}

fn open_cell(keys: &Keys, intent: &str, field: &str, v: &mut Value) -> Result<(), AipError> {
    let Value::Object(m) = v else { return Ok(()) };
    let (Some(Value::String(c)), Some(Value::String(row))) = (m.get("c"), m.get("i")) else {
        return Err(AipError::new(codes::INTERNAL, intent, format!("{field} came back in an unexpected form")));
    };
    let plain = keys.decrypt(field, row, c).map_err(|e| open_failed(intent, field, e))?;
    *v = match m.get("m").and_then(Value::as_str) {
        Some(name) => mask(name, &plain).map(Value::String).unwrap_or(Value::Null),
        None => Value::String(plain),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(spec_ids: &[&str]) -> Keys {
        // keys made inside the test, from fixed bytes: nothing here is a real secret
        let spec: Vec<String> = spec_ids
            .iter()
            .enumerate()
            .map(|(i, id)| format!("{id}:{}", base64::engine::general_purpose::STANDARD.encode([(i as u8) + 1; 32])))
            .collect();
        Keys::parse(&spec.join(",")).expect("valid keys")
    }

    #[test]
    fn roundtrip_with_a_fresh_nonce_each_time() {
        let k = keys(&["k1"]);
        let (a, b) = (k.encrypt("Member.email", "r1", "a@b.kr"), k.encrypt("Member.email", "r1", "a@b.kr"));
        assert_ne!(a, b, "a random nonce makes equal plaintexts differ");
        assert!(a.starts_with("v1:k1:") && !a.contains("a@b.kr"));
        assert_eq!(k.decrypt("Member.email", "r1", &a), Ok("a@b.kr".into()));
    }

    #[test]
    fn bound_to_field_and_row() {
        let k = keys(&["k1"]);
        let c = k.encrypt("Member.email", "r1", "a@b.kr");
        assert_eq!(k.decrypt("Member.email", "r2", &c), Err(OpenError::Authentication("k1".into())), "another row");
        assert_eq!(k.decrypt("Member.phone", "r1", &c), Err(OpenError::Authentication("k1".into())), "another field");
        let mut flipped = c.clone().into_bytes();
        let last = flipped.len() - 3;
        flipped[last] = if flipped[last] == b'A' { b'B' } else { b'A' };
        assert!(k.decrypt("Member.email", "r1", &String::from_utf8(flipped).expect("ascii")).is_err(), "a changed byte");
    }

    #[test]
    fn rotation_reads_old_keys_and_writes_the_first() {
        let old = keys(&["k1"]);
        let c = old.encrypt("T.f", "r", "x");
        let both = Keys::parse(&format!(
            "k2:{},k1:{}",
            base64::engine::general_purpose::STANDARD.encode([9u8; 32]),
            base64::engine::general_purpose::STANDARD.encode([1u8; 32])
        ))
        .expect("keys");
        assert_eq!(both.decrypt("T.f", "r", &c), Ok("x".into()));
        assert!(!both.is_current(&c));
        assert!(both.is_current(&both.encrypt("T.f", "r", "x")));
        let only_new = Keys::parse(&format!("k2:{}", base64::engine::general_purpose::STANDARD.encode([9u8; 32]))).expect("keys");
        assert_eq!(only_new.decrypt("T.f", "r", &c), Err(OpenError::UnknownKey("k1".into())));
        assert_eq!(only_new.decrypt("T.f", "r", "plain text"), Err(OpenError::Format));
    }

    #[test]
    fn key_spec_is_checked() {
        let b = |n: usize| base64::engine::general_purpose::STANDARD.encode(vec![5u8; n]);
        assert!(Keys::parse("").is_err());
        assert!(Keys::parse(&format!("k1:{}", b(16))).is_err(), "AES-256 needs 32 bytes");
        assert!(Keys::parse("k1:not base64!").is_err());
        assert!(Keys::parse(&format!("k1:{},k1:{}", b(32), b(32))).is_err(), "duplicate id");
        assert!(Keys::parse(&format!("bad id:{}", b(32))).is_err());
        assert!(format!("{:?}", Keys::parse(&format!("k1:{}", b(32))).expect("ok")).find(&b(32)).is_none(), "Debug never prints key material");
    }

    #[test]
    fn only_the_planned_places_are_opened() {
        let k = keys(&["k1"]);
        let ct = k.encrypt("Member.email", "r1", "a@b.kr");
        let forged = json!({"c": ct, "i": "r1"});
        let mut v = json!([{"id": "r1", "email": {"c": ct, "i": "r1"}, "bio": forged}, {"id": "r2", "email": null}]);
        let paths = [DecryptPath { path: vec!["[]".into(), "email".into()], field: "Member.email".into() }];
        open_result(Some(&k), "q", &paths, &mut v).expect("opens");
        assert_eq!(v[0]["email"], "a@b.kr");
        assert_eq!(v[1]["email"], Value::Null);
        assert_eq!(v[0]["bio"], forged, "a look-alike elsewhere stays data");
    }

    #[test]
    fn masks_after_decrypting() {
        let k = keys(&["k1"]);
        let ct = k.encrypt("Member.email", "r1", "alice@example.com");
        let mut v = json!({"email": {"c": ct, "i": "r1", "m": "mask.email"}});
        open_result(Some(&k), "q", &[DecryptPath { path: vec!["email".into()], field: "Member.email".into() }], &mut v).expect("opens");
        assert_eq!(v["email"], "a***@example.com");
        assert_eq!(mask("mask.phone", "01012345678").as_deref(), Some("010-****-5678"));
        assert_eq!(mask("mask.other", "x"), None);
    }
}
