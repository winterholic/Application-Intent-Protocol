//! Structured HMAC markers. Verification selects the key recorded with the stored marker, never the new active key.
use crate::cell::{Error, encode_fields, valid_id};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;
use zeroize::Zeroizing;

type Hmac256 = Hmac<Sha256>;

pub struct Keys {
    active: String,
    entries: BTreeMap<String, Zeroizing<[u8; 32]>>,
}
impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FingerprintKeys").field("active", &self.active).field("count", &self.entries.len()).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct Context<'a> {
    pub schema: &'a str,
    pub principal: &'a str,
    pub request_key: &'a str,
    pub resource: &'a str,
    pub field: &'a str,
}

pub struct BlindIndexContext<'a> {
    pub schema: &'a str,
    pub resource: &'a str,
    pub field: &'a str,
    pub scope_kind: &'a str,
    pub scope_id: &'a str,
}

impl Keys {
    pub fn new(active: &str, keys: Vec<(&str, [u8; 32])>) -> Result<Self, Error> {
        if keys.is_empty() || keys.len() > 32 || !valid_id(active) {
            return Err(Error::InvalidInput);
        }
        let mut entries = BTreeMap::new();
        for (id, key) in keys {
            let key = Zeroizing::new(key);
            if !valid_id(id) || entries.insert(id.to_owned(), key).is_some() {
                return Err(Error::InvalidInput);
            }
        }
        if !entries.contains_key(active) {
            return Err(Error::InvalidInput);
        }
        Ok(Self { active: active.into(), entries })
    }

    /// `canonical_value` is the host's validated canonical JSON scalar, not a caller-supplied marker.
    pub fn idem(&self, context: &Context<'_>, canonical_value: &[u8]) -> Result<String, Error> {
        let body = context.body(canonical_value)?;
        let tag = self.mac(&self.active, b"aip.idem.v2", &body)?.finalize().into_bytes();
        Ok(format!("idem_v2:{}:{}", self.active, hex(&tag)))
    }

    /// Missing retained keys are availability failures, not evidence that the two requests differ.
    pub fn verify_idem(&self, context: &Context<'_>, canonical_value: &[u8], stored: &str) -> Result<bool, Error> {
        let (id, tag) = marker(stored, "idem_v2", 32)?;
        let body = context.body(canonical_value)?;
        Ok(self.mac(id, b"aip.idem.v2", &body)?.verify_slice(&tag).is_ok())
    }

    pub fn audit(&self, context: &crate::cell::Context<'_>, canonical_value: &[u8]) -> Result<String, Error> {
        let body = audit_body(context, canonical_value)?;
        let tag = self.mac(&self.active, b"aip.audit.v2", &body)?.finalize().into_bytes();
        Ok(format!("audit_v2:{}:{}", self.active, hex(&tag[..16])))
    }

    pub fn verify_audit(&self, context: &crate::cell::Context<'_>, canonical_value: &[u8], stored: &str) -> Result<bool, Error> {
        let (id, tag) = marker(stored, "audit_v2", 16)?;
        let body = audit_body(context, canonical_value)?;
        Ok(self.mac(id, b"aip.audit.v2", &body)?.verify_truncated_left(&tag).is_ok())
    }

    /// The runtime config supplies a separate blind-index root ring. This generates one tag per retained key;
    /// the query planner still owns scope binding, row policy, post-filtering and scan limits.
    pub fn blind_tags(&self, context: &BlindIndexContext<'_>, canonical_value: &[u8]) -> Result<Vec<String>, Error> {
        let identity = [context.schema, context.resource, context.field, context.scope_kind, context.scope_id];
        validate_identity(&identity)?;
        let fields: Vec<&[u8]> = identity.iter().map(|s| s.as_bytes()).chain(std::iter::once(canonical_value)).collect();
        let body = Zeroizing::new(encode_fields(b"aip.bidx.value.v1", &fields)?);
        self.entries
            .keys()
            .map(|id| {
                let tag = self.mac(id, b"aip.bidx.v1", &body)?.finalize().into_bytes();
                Ok(format!("bidx_v1:{id}:{}", URL_SAFE_NO_PAD.encode(tag)))
            })
            .collect()
    }

    fn mac(&self, id: &str, domain: &[u8], body: &[u8]) -> Result<Hmac256, Error> {
        let root = self.entries.get(id).ok_or(Error::Unavailable)?;
        let key = derive(root, domain)?;
        let mut mac = Hmac256::new_from_slice(key.as_ref()).map_err(|_| Error::InvalidInput)?;
        mac.update(body);
        Ok(mac)
    }
}

impl Context<'_> {
    fn body(&self, canonical_value: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        let identity = [self.schema, self.principal, self.request_key, self.resource, self.field];
        validate_identity(&identity)?;
        let fields: Vec<&[u8]> = identity.iter().map(|s| s.as_bytes()).chain(std::iter::once(canonical_value)).collect();
        encode_fields(b"aip.idem.value.v2", &fields).map(Zeroizing::new)
    }
}

fn audit_body(context: &crate::cell::Context<'_>, value: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let identity = [context.schema, context.resource, context.field, context.row_id, context.scope_kind, context.scope_id];
    validate_identity(&identity)?;
    let fields: Vec<&[u8]> = identity.iter().map(|s| s.as_bytes()).chain(std::iter::once(value)).collect();
    encode_fields(b"aip.audit.value.v2", &fields).map(Zeroizing::new)
}

fn validate_identity(identity: &[&str]) -> Result<(), Error> {
    if identity.iter().any(|s| s.is_empty() || s.len() > 2048) {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

fn derive(root: &[u8; 32], domain: &[u8]) -> Result<Zeroizing<[u8; 32]>, Error> {
    let mut key = Zeroizing::new([0_u8; 32]);
    Hkdf::<Sha256>::new(Some(b"aip.crypto.subkey.v2"), root).expand(domain, key.as_mut()).map_err(|_| Error::InvalidInput)?;
    Ok(key)
}

fn marker<'a>(stored: &'a str, prefix: &str, size: usize) -> Result<(&'a str, Vec<u8>), Error> {
    if stored.len() > 160 {
        return Err(Error::Corrupted);
    }
    let mut parts = stored.split(':');
    let (Some(kind), Some(id), Some(tag), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(Error::Corrupted);
    };
    if kind != prefix || !valid_id(id) || tag.len() != size * 2 || !tag.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(Error::Corrupted);
    }
    let bytes = tag
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |c: u8| if c.is_ascii_digit() { c - b'0' } else { c - b'a' + 10 };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect();
    Ok((id, bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn rfc5869_case_one_and_purpose_subkeys_are_distinct() {
        // RFC 5869 appendix A.1 (SHA-256).
        let ikm = [0x0b; 22];
        let salt: Vec<u8> = (0..=0x0c).collect();
        let info: Vec<u8> = (0xf0..=0xf9).collect();
        let mut output = [0_u8; 42];
        Hkdf::<Sha256>::new(Some(&salt), &ikm).expand(&info, &mut output).unwrap();
        assert_eq!(hex(&output), "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865");
        let root = [7; 32];
        let idem = derive(&root, b"aip.idem.v2").unwrap();
        let audit = derive(&root, b"aip.audit.v2").unwrap();
        let bidx = derive(&root, b"aip.bidx.v1").unwrap();
        assert_ne!(idem, audit);
        assert_ne!(idem, bidx);
        assert_ne!(audit, bidx);
    }
}
