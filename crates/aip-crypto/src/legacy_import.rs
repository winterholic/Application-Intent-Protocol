//! Explicit offline PoC v1 import. This module is not compiled into normal product builds.
//! The operator supplies source identity and destination scope; no runtime fallback or raw plaintext API exists.
use crate::cell::{Context, DataKey, Error, MAX_CELL_BYTES, valid_id};
use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, KeyInit, Nonce, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use zeroize::Zeroizing;

pub struct LegacyKey {
    id: String,
    material: Zeroizing<[u8; 32]>,
}
impl LegacyKey {
    pub fn new(id: &str, material: [u8; 32]) -> Result<Self, Error> {
        let material = Zeroizing::new(material);
        if !valid_id(id) {
            return Err(Error::InvalidInput);
        }
        Ok(Self { id: id.into(), material })
    }

    pub fn import_cell(
        &self,
        source_field: &str,
        source_row: &str,
        frame: &str,
        destination: &DataKey,
        context: &Context<'_>,
    ) -> Result<String, Error> {
        // Legacy delimiter identity has no schema or scope binding. Do not invent that guarantee during import.
        if [source_field, source_row].iter().any(|value| value.is_empty() || value.len() > 256 || value.contains('|')) {
            return Err(Error::InvalidInput);
        }
        if frame.len() > 4 * (MAX_CELL_BYTES + 16).div_ceil(3) + 128 {
            return Err(Error::Corrupted);
        }
        let mut parts = frame.split(':');
        let (Some("v1"), Some(id), Some(nonce), Some(body), None) = (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(Error::Corrupted);
        };
        if id != self.id {
            return Err(Error::Corrupted);
        }
        let nonce = STANDARD.decode(nonce).map_err(|_| Error::Corrupted)?;
        let nonce = Nonce::<Aes256Gcm>::try_from(nonce.as_slice()).map_err(|_| Error::Corrupted)?;
        let body = STANDARD.decode(body).map_err(|_| Error::Corrupted)?;
        if !(16..=MAX_CELL_BYTES + 16).contains(&body.len()) {
            return Err(Error::Corrupted);
        }
        let aad = format!("aip.enc.v1|{source_field}|{source_row}");
        let cipher = Aes256Gcm::new_from_slice(self.material.as_ref()).map_err(|_| Error::InvalidInput)?;
        let plain = Zeroizing::new(cipher.decrypt(&nonce, Payload { msg: &body, aad: aad.as_bytes() }).map_err(|_| Error::Corrupted)?);
        destination.seal(context, &plain)
    }
}
