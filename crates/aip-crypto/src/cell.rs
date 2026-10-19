//! Bounded v2 cell frames. The host supplies a stable DEK; wrapping-KEK rotation never changes this frame's identity.
use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, Generate, KeyInit, Nonce, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use zeroize::Zeroizing;

pub const MAX_CELL_BYTES: usize = 1024 * 1024;
const MAX_FRAME_BYTES: usize = 4 * (MAX_CELL_BYTES + 16).div_ceil(3) + 128;

pub struct DataKey {
    id: String,
    version: u32,
    key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataKey").field("id", &self.id).field("version", &self.version).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct Context<'a> {
    pub schema: &'a str,
    pub resource: &'a str,
    pub field: &'a str,
    pub row_id: &'a str,
    pub scope_kind: &'a str,
    pub scope_id: &'a str,
}

/// Intentionally contains no plaintext, ciphertext, key bytes or provider diagnostics.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Corrupted,
    Unavailable,
}

impl DataKey {
    pub fn new(id: &str, version: u32, key: [u8; 32]) -> Result<Self, Error> {
        let key = Zeroizing::new(key);
        if !valid_id(id) || version == 0 {
            return Err(Error::InvalidInput);
        }
        Ok(Self { id: id.into(), version, key })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn seal(&self, context: &Context<'_>, plaintext: &[u8]) -> Result<String, Error> {
        if plaintext.len() > MAX_CELL_BYTES {
            return Err(Error::InvalidInput);
        }
        let tag = context.tag()?;
        let aad = self.aad(context, tag)?;
        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| Error::InvalidInput)?;
        let nonce = Nonce::<Aes256Gcm>::try_generate().map_err(|_| Error::Unavailable)?;
        let body = cipher.encrypt(&nonce, Payload { msg: plaintext, aad: &aad }).map_err(|_| Error::Unavailable)?;
        Ok(format!("v2:{tag}:{}:{}:{}:{}", self.id, self.version, STANDARD.encode(nonce.as_slice()), STANDARD.encode(body)))
    }

    pub fn open(&self, context: &Context<'_>, frame: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(Error::Corrupted);
        }
        let mut parts = frame.split(':');
        let (Some("v2"), Some(tag), Some(id), Some(version), Some(nonce), Some(body), None) =
            (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Corrupted);
        };
        if id != self.id || version != self.version.to_string() || tag != context.tag().map_err(|_| Error::Corrupted)? {
            return Err(Error::Corrupted);
        }
        let nonce = STANDARD.decode(nonce).map_err(|_| Error::Corrupted)?;
        let nonce = Nonce::<Aes256Gcm>::try_from(nonce.as_slice()).map_err(|_| Error::Corrupted)?;
        let body = STANDARD.decode(body).map_err(|_| Error::Corrupted)?;
        if !(16..=MAX_CELL_BYTES + 16).contains(&body.len()) {
            return Err(Error::Corrupted);
        }
        let aad = self.aad(context, tag).map_err(|_| Error::Corrupted)?;
        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| Error::Corrupted)?;
        cipher.decrypt(&nonce, Payload { msg: &body, aad: &aad }).map(Zeroizing::new).map_err(|_| Error::Corrupted)
    }

    fn aad(&self, c: &Context<'_>, tag: &str) -> Result<Vec<u8>, Error> {
        let fields = [c.schema, c.resource, c.field, c.row_id, c.scope_kind, c.scope_id, &self.id, &self.version.to_string(), tag];
        if fields.iter().any(|field| field.is_empty() || field.len() > 256) {
            return Err(Error::InvalidInput);
        }
        encode_fields(b"aip.enc.v2", &fields.map(str::as_bytes))
    }
}

impl Context<'_> {
    fn tag(&self) -> Result<&'static str, Error> {
        match self.scope_kind {
            "service" if self.scope_id == "global" => Ok("s"),
            kind if kind.strip_prefix("ref:").is_some_and(|name| !name.is_empty()) => Ok("r"),
            kind if kind.strip_prefix("owner:").is_some_and(|name| !name.is_empty()) => Ok("o"),
            _ => Err(Error::InvalidInput),
        }
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// Bounded, unambiguous domain and component framing. Does not normalize scalar values.
pub(crate) fn encode_fields(domain: &[u8], fields: &[&[u8]]) -> Result<Vec<u8>, Error> {
    if domain.is_empty() || domain.len() > 128 || fields.len() > 32 {
        return Err(Error::InvalidInput);
    }
    let mut output = Vec::new();
    for field in std::iter::once(domain).chain(fields.iter().copied()) {
        if field.len() > MAX_CELL_BYTES || output.len() + 4 + field.len() > 2 * MAX_CELL_BYTES {
            return Err(Error::InvalidInput);
        }
        output.extend_from_slice(&(field.len() as u32).to_be_bytes());
        output.extend_from_slice(field);
    }
    Ok(output)
}

#[derive(Debug, PartialEq, Eq)]
pub struct CellHeader<'a> {
    pub tag: &'a str,
    pub dek_id: &'a str,
    pub version: u32,
    pub nonce_len: usize,
    pub ciphertext_and_tag_len: usize,
}

/// Inspects and parses a v2 cell frame header without decrypting or requiring the data key.
/// Checks frame length, v2 prefix, scope tag, dek_id identifier rules, canonical version format,
/// nonce size (12 bytes), and ciphertext minimum tag length (>= 16 bytes).
/// This structural inspection does not authenticate ciphertext, AAD, scope or key availability.
pub fn parse_header<'a>(frame: &'a str) -> Result<CellHeader<'a>, Error> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(Error::Corrupted);
    }
    let mut parts = frame.split(':');
    let (Some("v2"), Some(tag), Some(id), Some(version), Some(nonce), Some(body), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Corrupted);
    };
    if !matches!(tag, "s" | "r" | "o") || !valid_id(id) {
        return Err(Error::Corrupted);
    }
    let ver = version.parse::<u32>().map_err(|_| Error::Corrupted)?;
    if ver == 0 || version != ver.to_string() {
        return Err(Error::Corrupted);
    }
    let nonce_bytes = STANDARD.decode(nonce).map_err(|_| Error::Corrupted)?;
    if nonce_bytes.len() != 12 {
        return Err(Error::Corrupted);
    }
    let body_bytes = STANDARD.decode(body).map_err(|_| Error::Corrupted)?;
    if !(16..=MAX_CELL_BYTES + 16).contains(&body_bytes.len()) {
        return Err(Error::Corrupted);
    }
    Ok(CellHeader { tag, dek_id: id, version: ver, nonce_len: nonce_bytes.len(), ciphertext_and_tag_len: body_bytes.len() })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn hex(s: &str) -> Vec<u8> {
        s.as_bytes().as_chunks::<2>().0.iter().map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
    }
    #[test]
    fn nist_cavs_aes_256_gcm_vector() {
        // NIST gcmEncryptExtIV256.rsp, also published in RustCrypto/AEADs aes-gcm/tests/aes256gcm.rs.
        let cipher = Aes256Gcm::new_from_slice(&hex("b52c505a37d78eda5dd34f20c22540ea1b58963cf8e5bf8ffa85f9f2492505b4")).unwrap();
        let nonce = Nonce::<Aes256Gcm>::try_from(hex("516c33929df5a3284ff463d7").as_slice()).unwrap();
        let sealed = cipher.encrypt(&nonce, Payload { msg: b"", aad: b"" }).unwrap();
        assert_eq!(sealed, hex("bdc1ac884d332457a1d2664f168c76f0"));
        assert_eq!(cipher.decrypt(&nonce, sealed.as_slice()).unwrap(), b"");
    }
    #[test]
    fn framing_separates_component_boundaries_and_domains() {
        assert_ne!(encode_fields(b"aip.enc.v2", &[b"a|b", b"c"]).unwrap(), encode_fields(b"aip.enc.v2", &[b"a", b"b|c"]).unwrap());
        assert_ne!(encode_fields(b"aip.enc.v2", &[b"a", b"b"]).unwrap(), encode_fields(b"aip.dek.v2", &[b"a", b"b"]).unwrap());
    }
    #[test]
    fn parse_header_validates_v2_cell_structure() {
        let key = DataKey::new("dek1", 1, [0x42; 32]).unwrap();
        let ctx = Context { schema: "s", resource: "Secret", field: "value", row_id: "1", scope_kind: "service", scope_id: "global" };
        let frame = key.seal(&ctx, b"secret data").unwrap();
        let header = parse_header(&frame).unwrap();
        assert_eq!(header.tag, "s");
        assert_eq!(header.dek_id, "dek1");
        assert_eq!(header.version, 1);
        assert_eq!(header.nonce_len, 12);
        assert_eq!(header.ciphertext_and_tag_len, 11 + 16);

        // Negative: malformed or corrupted frames
        assert_eq!(parse_header("v1:s:dek1:1:nonce:body"), Err(Error::Corrupted));
        assert_eq!(parse_header("v2:s:bad-dek:0:nonce:body"), Err(Error::Corrupted));
        assert_eq!(parse_header("not a valid frame"), Err(Error::Corrupted));
    }
}
