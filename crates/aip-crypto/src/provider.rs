//! Local KEK providers. Secret sources are runtime inputs; errors never echo them.
use crate::cell::{Error, encode_fields, valid_id};
use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, Generate, KeyInit, Nonce, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hkdf::Hkdf;
use sha2::Sha256;
use std::{collections::BTreeMap, path::Path};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct WrapContext<'a> {
    pub schema: &'a str,
    pub scope_kind: &'a str,
    pub scope_id: &'a str,
    pub dek_id: &'a str,
    pub version: u32,
}
#[derive(Clone, Debug)]
pub struct WrappedDek {
    pub kek_id: String,
    /// 12 nonce bytes followed by the 32-byte encrypted DEK and 16-byte authentication tag.
    pub sealed: Vec<u8>,
}

pub type ProviderFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Error>> + Send + 'a>>;
pub trait KekProvider: Send + Sync {
    fn wrap<'a>(&'a self, context: &'a WrapContext<'_>, dek: &'a [u8; 32]) -> ProviderFuture<'a, WrappedDek>;
    fn unwrap<'a>(&'a self, context: &'a WrapContext<'_>, wrapped: &'a WrappedDek) -> ProviderFuture<'a, Zeroizing<[u8; 32]>>;
}

pub struct LocalProvider {
    active: String,
    keys: BTreeMap<String, Zeroizing<[u8; 32]>>,
}
impl std::fmt::Debug for LocalProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalKekProvider").field("count", &self.keys.len()).finish_non_exhaustive()
    }
}

impl LocalProvider {
    pub fn from_spec(spec: &str, active: &str) -> Result<Self, Error> {
        if spec.is_empty() || spec.len() > 8192 || !valid_id(active) {
            return Err(Error::InvalidInput);
        }
        let mut keys = BTreeMap::new();
        for entry in spec.trim().split(',') {
            let (id, value) = entry.trim().split_once(':').ok_or(Error::InvalidInput)?;
            if keys.len() == 32 || !valid_id(id) {
                return Err(Error::InvalidInput);
            }
            let raw = Zeroizing::new(STANDARD.decode(value).map_err(|_| Error::InvalidInput)?);
            let key = Zeroizing::new(<[u8; 32]>::try_from(raw.as_slice()).map_err(|_| Error::InvalidInput)?);
            if keys.insert(id.to_owned(), key).is_some() {
                return Err(Error::InvalidInput);
            }
        }
        if !keys.contains_key(active) {
            return Err(Error::InvalidInput);
        }
        Ok(Self { active: active.into(), keys })
    }

    pub fn from_env(name: &str, active: &str) -> Result<Self, Error> {
        if name.is_empty() || name.len() > 128 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(Error::InvalidInput);
        }
        let spec = Zeroizing::new(std::env::var(name).map_err(|_| Error::Unavailable)?);
        Self::from_spec(&spec, active)
    }

    pub fn from_file(path: &Path, active: &str) -> Result<Self, Error> {
        let spec = read_key_file(path)?;
        Self::from_spec(&spec, active)
    }

    fn cipher(&self, id: &str) -> Result<Aes256Gcm, Error> {
        let root = self.keys.get(id).ok_or(Error::Corrupted)?;
        let mut key = Zeroizing::new([0_u8; 32]);
        Hkdf::<Sha256>::new(Some(b"aip.crypto.subkey.v2"), root.as_ref()).expand(b"aip.wrap.v2", key.as_mut()).map_err(|_| Error::InvalidInput)?;
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| Error::InvalidInput)
    }
}

impl KekProvider for LocalProvider {
    fn wrap<'a>(&'a self, context: &'a WrapContext<'_>, dek: &'a [u8; 32]) -> ProviderFuture<'a, WrappedDek> {
        Box::pin(async move {
            let aad = context.aad(&self.active)?;
            let nonce = Nonce::<Aes256Gcm>::try_generate().map_err(|_| Error::Unavailable)?;
            let body = self.cipher(&self.active)?.encrypt(&nonce, Payload { msg: dek, aad: &aad }).map_err(|_| Error::Unavailable)?;
            let mut sealed = nonce.to_vec();
            sealed.extend_from_slice(&body);
            Ok(WrappedDek { kek_id: self.active.clone(), sealed })
        })
    }
    fn unwrap<'a>(&'a self, context: &'a WrapContext<'_>, wrapped: &'a WrappedDek) -> ProviderFuture<'a, Zeroizing<[u8; 32]>> {
        Box::pin(async move {
            if wrapped.sealed.len() != 60 || !valid_id(&wrapped.kek_id) {
                return Err(Error::Corrupted);
            }
            let aad = context.aad(&wrapped.kek_id).map_err(|_| Error::Corrupted)?;
            let nonce = Nonce::<Aes256Gcm>::try_from(&wrapped.sealed[..12]).map_err(|_| Error::Corrupted)?;
            let plain = Zeroizing::new(
                self.cipher(&wrapped.kek_id)?.decrypt(&nonce, Payload { msg: &wrapped.sealed[12..], aad: &aad }).map_err(|_| Error::Corrupted)?,
            );
            <[u8; 32]>::try_from(plain.as_slice()).map(Zeroizing::new).map_err(|_| Error::Corrupted)
        })
    }
}

impl WrapContext<'_> {
    fn aad(&self, kek_id: &str) -> Result<Vec<u8>, Error> {
        if !valid_id(self.dek_id) || self.version == 0 {
            return Err(Error::InvalidInput);
        }
        let version = self.version.to_string();
        let fields = [self.schema, self.scope_kind, self.scope_id, self.dek_id, &version, kek_id];
        if fields.iter().any(|field| field.is_empty() || field.len() > 256) {
            return Err(Error::InvalidInput);
        }
        encode_fields(b"aip.dek.v2", &fields.map(str::as_bytes))
    }
}

#[cfg(unix)]
fn read_key_file(path: &Path) -> Result<Zeroizing<String>, Error> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat};
    use std::{io::Read, path::Component};
    // Walk by directory descriptors: checking parent symlinks before a later path open would have a TOCTOU race.
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir = open(if path.is_absolute() { "/" } else { "." }, flags, Mode::empty()).map_err(|_| Error::Unavailable)?;
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => names.push(name),
            Component::RootDir | Component::CurDir => {}
            _ => return Err(Error::InvalidInput),
        }
    }
    let (leaf, parents) = names.split_last().ok_or(Error::InvalidInput)?;
    for parent in parents {
        dir = openat(&dir, *parent, flags, Mode::empty()).map_err(|_| Error::InvalidInput)?;
    }
    let fd = openat(&dir, *leaf, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())
        .map_err(|_| Error::InvalidInput)?;
    let metadata = fstat(&fd).map_err(|_| Error::Unavailable)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || !matches!(metadata.st_mode & 0o7777, 0o400 | 0o600)
        || metadata.st_uid != rustix::process::geteuid().as_raw()
        || metadata.st_size < 0
        || metadata.st_size > 8192
    {
        return Err(Error::InvalidInput);
    }
    let mut text = Zeroizing::new(String::new());
    std::fs::File::from(fd).take(8193).read_to_string(&mut text).map_err(|_| Error::InvalidInput)?;
    if text.len() > 8192 {
        return Err(Error::InvalidInput);
    }
    Ok(text)
}
#[cfg(not(unix))]
fn read_key_file(_path: &Path) -> Result<Zeroizing<String>, Error> {
    Err(Error::Unavailable)
}
