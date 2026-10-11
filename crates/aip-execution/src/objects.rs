//! Streaming object storage with opaque identifiers.
//!
//! The local backend writes into one canonicalized directory and publishes
//! completed objects by atomically renaming a staging directory. S3-compatible
//! clients receive bounded-lifetime SigV4 URLs for data transfer.

use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const COPY_BUFFER: usize = 64 * 1024;
const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const DEFAULT_MAX_OWNER_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const DEFAULT_MAX_OBJECTS: usize = 1000;
pub const MAX_S3_OBJECT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
pub const MAX_S3_PART_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MAX_PRESIGN_SECONDS: u64 = 604_800;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    InvalidConfig,
    InvalidKey,
    InvalidMetadata,
    TooLarge,
    QuotaExceeded,
    Capacity,
    UnsafeObjectPath,
    NotFound,
    InvalidMultipart,
    Io(io::Error),
    Http(String),
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Public object identifier. It is always a generated or validated UUID and
/// never contains a caller-controlled path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectKey(Uuid);

impl ObjectKey {
    pub fn parse(value: &str) -> Result<Self, Error> {
        let id = Uuid::parse_str(value).map_err(|_| Error::InvalidKey)?;
        if id.to_string() != value.to_ascii_lowercase() {
            return Err(Error::InvalidKey);
        }
        Ok(Self(id))
    }
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }
    fn new() -> Self {
        Self::generate()
    }
}
impl fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectMetadata {
    pub size: u64,
    /// Lowercase hexadecimal SHA-256, also used as the local ETag.
    pub etag: String,
    pub content_type: String,
}

#[derive(Clone, Debug)]
/// Local filesystem storage. Configure `root` as a directory writable only by
/// the service account. Object operations stay relative to an open root handle
/// so replacing the configured pathname cannot redirect active stores.
pub struct LocalConfig {
    root: PathBuf,
    max_bytes: u64,
    max_owner_bytes: u64,
    max_objects: usize,
}
impl LocalConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), max_bytes: DEFAULT_MAX_BYTES, max_owner_bytes: DEFAULT_MAX_OWNER_BYTES, max_objects: DEFAULT_MAX_OBJECTS }
    }
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }
    pub fn with_owner_quota(mut self, max_total_bytes: u64, max_objects: usize) -> Self {
        self.max_owner_bytes = max_total_bytes;
        self.max_objects = max_objects;
        self
    }
}

#[derive(Clone, Debug)]
pub struct LocalStore {
    root: PathBuf,
    root_dir: std::sync::Arc<File>,
    max_bytes: u64,
    max_owner_bytes: u64,
    max_objects: usize,
}
impl LocalStore {
    pub fn new(config: LocalConfig) -> Result<Self, Error> {
        if config.max_bytes == 0 || config.max_owner_bytes == 0 || config.max_objects == 0 {
            return Err(Error::InvalidConfig);
        }
        fs::create_dir_all(&config.root)?;
        let root = fs::canonicalize(&config.root)?;
        if !fs::metadata(&root)?.is_dir() {
            return Err(Error::InvalidConfig);
        }
        let root_dir = std::sync::Arc::new(open_directory(&root)?);
        set_private_dir_handle(&root_dir)?;
        ensure_quota_lock(&root_dir)?;
        Ok(Self { root, root_dir, max_bytes: config.max_bytes, max_owner_bytes: config.max_owner_bytes, max_objects: config.max_objects })
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Creates a storage namespace for a trusted principal identifier. The
    /// identifier itself is hashed so it never appears in the filesystem path.
    pub fn for_owner(&self, owner: &str) -> Result<Self, Error> {
        if owner.is_empty() || owner.len() > 2048 || owner.chars().any(char::is_control) {
            return Err(Error::InvalidConfig);
        }
        let namespace = hex(&Sha256::digest(owner.as_bytes()));
        create_directory_at(&self.root_dir, &namespace)?;
        let root_dir = std::sync::Arc::new(open_directory_at(&self.root_dir, &namespace)?);
        set_private_dir_handle(&root_dir)?;
        ensure_quota_lock(&root_dir)?;
        Ok(Self {
            root: self.root.join(namespace),
            root_dir,
            max_bytes: self.max_bytes,
            max_owner_bytes: self.max_owner_bytes,
            max_objects: self.max_objects,
        })
    }

    /// Streams into an unpublished staging directory and returns an opaque key
    /// only after all bytes and metadata have been flushed and atomically moved.
    pub fn put<R: Read>(&self, mut reader: R, content_type: &str) -> Result<(ObjectKey, ObjectMetadata), Error> {
        let mut upload = self.begin_upload(content_type)?;
        let mut buffer = [0u8; COPY_BUFFER];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            upload.write_chunk(&buffer[..n])?;
        }
        upload.finish()
    }

    /// Starts a bounded streaming upload. Keep calls off async executor threads;
    /// the object can be written from a `spawn_blocking` task.
    pub fn begin_upload(&self, content_type: &str) -> Result<LocalUpload, Error> {
        if !valid_content_type(content_type) {
            return Err(Error::InvalidMetadata);
        }
        let key = ObjectKey::new();
        let stage = format!(".stage-{}", Uuid::new_v4());
        let quota_lock = acquire_quota_lock(&self.root_dir)?;
        let usage = self.scan_usage()?;
        if usage.objects >= self.max_objects {
            return Err(Error::QuotaExceeded);
        }
        create_directory_at(&self.root_dir, &stage)?;
        let stage_dir = match open_directory_at(&self.root_dir, &stage) {
            Ok(directory) => std::sync::Arc::new(directory),
            Err(error) => {
                let _ = unlink_directory_at(&self.root_dir, &stage);
                return Err(error.into());
            }
        };
        if let Err(error) = set_private_dir_handle(&stage_dir) {
            let _ = unlink_directory_at(&self.root_dir, &stage);
            return Err(error);
        }
        let output = match create_private_file_at(&stage_dir, "data") {
            Ok(file) => file,
            Err(error) => {
                let _ = unlink_directory_at(&self.root_dir, &stage);
                return Err(error);
            }
        };
        Ok(LocalUpload {
            root_dir: self.root_dir.clone(),
            stage_dir,
            quota_lock: Some(quota_lock),
            committed_bytes: usage.bytes,
            max_owner_bytes: self.max_owner_bytes,
            stage,
            key,
            content_type: content_type.to_owned(),
            max_bytes: self.max_bytes,
            size: 0,
            hash: Sha256::new(),
            output: Some(output),
            too_large: false,
            committed: false,
        })
    }

    /// Copies an object to the supplied writer with a fixed 64 KiB buffer.
    pub fn get<W: Write>(&self, key: &ObjectKey, mut writer: W) -> Result<ObjectMetadata, Error> {
        let (mut input, metadata) = self.open(key)?;
        let mut buffer = [0u8; COPY_BUFFER];
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buffer[..n])?;
        }
        Ok(metadata)
    }

    /// Opens a checked object file so async hosts can wrap it in their runtime's
    /// file type and stream it without buffering the object in memory.
    pub fn open(&self, key: &ObjectKey) -> Result<(File, ObjectMetadata), Error> {
        let directory = self.open_object_dir(key)?;
        let input = open_regular_at(&directory, "data")?;
        let metadata_file = open_regular_at(&directory, "metadata.json")?;
        let metadata = read_metadata_file(metadata_file)?;
        if input.metadata()?.len() != metadata.size {
            return Err(Error::Protocol("object metadata size mismatch".into()));
        }
        Ok((input, metadata))
    }

    pub fn stat(&self, key: &ObjectKey) -> Result<Option<ObjectMetadata>, Error> {
        let directory = match self.open_object_dir(key) {
            Ok(directory) => directory,
            Err(Error::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata_file = open_regular_at(&directory, "metadata.json")?;
        let metadata = read_metadata_file(metadata_file)?;
        let data = open_regular_at(&directory, "data")?;
        if data.metadata()?.len() != metadata.size {
            return Err(Error::Protocol("object metadata size mismatch".into()));
        }
        Ok(Some(metadata))
    }

    pub fn delete(&self, key: &ObjectKey) -> Result<bool, Error> {
        let _quota_lock = acquire_quota_lock(&self.root_dir)?;
        let name = key.as_str();
        let directory = match open_directory_at(&self.root_dir, &name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotADirectory || error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(Error::UnsafeObjectPath);
            }
            Err(error) => return Err(error.into()),
        };
        drop(directory);
        for entry in ["data", "metadata.json"] {
            unlink_file_at(&self.root_dir, &name, entry)?;
        }
        unlink_directory_at(&self.root_dir, &name)?;
        self.root_dir.sync_all()?;
        Ok(true)
    }
    fn scan_usage(&self) -> Result<Usage, Error> {
        let mut usage = Usage::default();
        let names = directory_names(&self.root_dir, self.max_objects.saturating_add(16))?;
        for name in names {
            if name == ".quota-lock" {
                continue;
            }
            if name.starts_with(".stage-") {
                unlink_directory_at(&self.root_dir, &name)?;
                continue;
            }
            let key = ObjectKey::parse(&name).map_err(|_| Error::UnsafeObjectPath)?;
            let directory = self.open_object_dir(&key)?;
            let metadata_file = open_regular_at(&directory, "metadata.json")?;
            let metadata = read_metadata_file(metadata_file)?;
            let data = open_regular_at(&directory, "data")?;
            if data.metadata()?.len() != metadata.size {
                return Err(Error::Protocol("object metadata size mismatch".into()));
            }
            usage.bytes = usage.bytes.checked_add(metadata.size).ok_or(Error::QuotaExceeded)?;
            usage.objects = usage.objects.checked_add(1).ok_or(Error::QuotaExceeded)?;
            if usage.bytes > self.max_owner_bytes || usage.objects > self.max_objects {
                return Err(Error::QuotaExceeded);
            }
        }
        Ok(usage)
    }
    fn open_object_dir(&self, key: &ObjectKey) -> Result<File, Error> {
        open_directory_at(&self.root_dir, &key.as_str()).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::NotFound
            } else if error.kind() == io::ErrorKind::NotADirectory
                || error.kind() == io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(libc::ELOOP)
            {
                Error::UnsafeObjectPath
            } else {
                Error::Io(error)
            }
        })
    }
}

#[derive(Default)]
struct Usage {
    bytes: u64,
    objects: usize,
}

/// An unpublished object upload. Dropping it removes its staging directory.
pub struct LocalUpload {
    root_dir: std::sync::Arc<File>,
    stage_dir: std::sync::Arc<File>,
    quota_lock: Option<File>,
    committed_bytes: u64,
    max_owner_bytes: u64,
    stage: String,
    key: ObjectKey,
    content_type: String,
    max_bytes: u64,
    size: u64,
    hash: Sha256,
    output: Option<File>,
    too_large: bool,
    committed: bool,
}

impl LocalUpload {
    pub fn key(&self) -> &ObjectKey {
        &self.key
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let new_size = self.size.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
        if new_size > self.max_bytes {
            self.too_large = true;
            return Err(Error::TooLarge);
        }
        if self.committed_bytes.checked_add(new_size).is_none_or(|total| total > self.max_owner_bytes) {
            self.too_large = true;
            return Err(Error::QuotaExceeded);
        }
        self.output.as_mut().ok_or(Error::InvalidConfig)?.write_all(bytes)?;
        self.hash.update(bytes);
        self.size = new_size;
        Ok(())
    }
    pub fn finish(mut self) -> Result<(ObjectKey, ObjectMetadata), Error> {
        if self.too_large {
            return Err(Error::TooLarge);
        }
        let mut output = self.output.take().ok_or(Error::InvalidConfig)?;
        output.flush()?;
        output.sync_all()?;
        let metadata = ObjectMetadata { size: self.size, etag: hex(&self.hash.clone().finalize()), content_type: self.content_type.clone() };
        let mut meta = create_private_file_at(&self.stage_dir, "metadata.json")?;
        serde_json::to_writer(&mut meta, &metadata).map_err(|e| Error::Protocol(e.to_string()))?;
        meta.sync_all()?;
        rename_at(&self.root_dir, &self.stage, &self.key.as_str())?;
        self.committed = true;
        self.root_dir.sync_all()?;
        self.quota_lock.take();
        Ok((self.key.clone(), metadata))
    }
}
impl Write for LocalUpload {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.write_chunk(buf) {
            Ok(()) => Ok(buf.len()),
            Err(Error::TooLarge) => Err(io::Error::new(io::ErrorKind::FileTooLarge, "object size limit exceeded")),
            Err(error) => Err(io::Error::other(error.to_string())),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.as_mut().ok_or_else(|| io::Error::other("upload is finalized"))?.flush()
    }
}
impl Drop for LocalUpload {
    fn drop(&mut self) {
        if !self.committed {
            let _ = unlink_directory_at(&self.root_dir, &self.stage);
        }
    }
}

/// Adapter boundary for custom local or network storage providers. Implementors
/// receive streams instead of whole object buffers.
pub trait ObjectBackend: Send + Sync {
    fn put(&self, reader: &mut dyn Read, content_type: &str) -> Result<(ObjectKey, ObjectMetadata), Error>;
    fn get(&self, key: &ObjectKey, writer: &mut dyn Write) -> Result<ObjectMetadata, Error>;
    fn stat(&self, key: &ObjectKey) -> Result<Option<ObjectMetadata>, Error>;
    fn delete(&self, key: &ObjectKey) -> Result<bool, Error>;
}
impl ObjectBackend for LocalStore {
    fn put(&self, reader: &mut dyn Read, content_type: &str) -> Result<(ObjectKey, ObjectMetadata), Error> {
        LocalStore::put(self, reader, content_type)
    }
    fn get(&self, key: &ObjectKey, writer: &mut dyn Write) -> Result<ObjectMetadata, Error> {
        LocalStore::get(self, key, writer)
    }
    fn stat(&self, key: &ObjectKey) -> Result<Option<ObjectMetadata>, Error> {
        LocalStore::stat(self, key)
    }
    fn delete(&self, key: &ObjectKey) -> Result<bool, Error> {
        LocalStore::delete(self, key)
    }
}

#[derive(Clone)]
pub struct S3Config {
    endpoint: String,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}
impl S3Config {
    pub fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
            region: region.into(),
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            session_token: None,
        }
    }
    pub fn with_session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(token.into());
        self
    }
}

#[derive(Clone)]
pub struct S3Store {
    config: S3Config,
    endpoint: Url,
    client: reqwest::Client,
    owner_prefix: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultipartUpload {
    pub key: ObjectKey,
    pub upload_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedPart {
    pub number: u16,
    pub etag: String,
}

impl S3Store {
    pub fn new(config: S3Config) -> Result<Self, Error> {
        let endpoint = Url::parse(&config.endpoint).map_err(|_| Error::InvalidConfig)?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !valid_bucket(&config.bucket)
            || !valid_segment(&config.region)
            || config.access_key.is_empty()
            || config.secret_key.is_empty()
            || config.access_key.contains(':')
            || config.access_key.chars().any(char::is_control)
            || config.secret_key.chars().any(char::is_control)
            || config.session_token.as_ref().is_some_and(|v| v.is_empty() || v.chars().any(char::is_control))
        {
            return Err(Error::InvalidConfig);
        }
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self { config, endpoint, client, owner_prefix: None })
    }

    /// Returns an owner-scoped view. The caller's principal identifier is
    /// hashed before it becomes part of the object path.
    pub fn for_owner(&self, owner: &str) -> Result<Self, Error> {
        if owner.is_empty() || owner.len() > 2048 || owner.chars().any(char::is_control) {
            return Err(Error::InvalidConfig);
        }
        let prefix = hex(&Sha256::digest(owner.as_bytes()));
        let mut scoped = self.clone();
        scoped.owner_prefix = Some(prefix);
        Ok(scoped)
    }

    /// Starts a multipart upload. Credentials are supplied at construction and
    /// used only by the server side request; they are never returned.
    pub async fn create_multipart(&self, content_type: &str) -> Result<MultipartUpload, Error> {
        if !valid_content_type(content_type) {
            return Err(Error::InvalidMetadata);
        }
        let key = ObjectKey::new();
        let mut url = self.object_url(&key)?;
        url.query_pairs_mut().append_pair("uploads", "");
        let response = self.signed_request(Method::POST, url, vec![("content-type", content_type.to_owned())], Vec::new()).await?;
        let body = bounded_body(response).await?;
        let xml = String::from_utf8(body).map_err(|e| Error::Protocol(e.to_string()))?;
        let upload_id = xml_tag(&xml, "UploadId").ok_or_else(|| Error::Protocol("multipart response omitted UploadId".into()))?;
        if !valid_upload_id(&upload_id) {
            return Err(Error::Protocol("invalid multipart upload id".into()));
        }
        Ok(MultipartUpload { key, upload_id })
    }

    pub fn presign_part(&self, key: &ObjectKey, upload_id: &str, part_number: u16, expires_seconds: u64) -> Result<String, Error> {
        if !(1..=10_000).contains(&part_number) || !valid_upload_id(upload_id) {
            return Err(Error::InvalidMultipart);
        }
        self.presign("PUT", key, vec![("partNumber", part_number.to_string()), ("uploadId", upload_id.to_owned())], expires_seconds)
    }

    pub fn presign_part_sized(&self, key: &ObjectKey, upload_id: &str, part_number: u16, size: u64, expires_seconds: u64) -> Result<String, Error> {
        if !(1..=10_000).contains(&part_number) || !valid_upload_id(upload_id) {
            return Err(Error::InvalidMultipart);
        }
        if size == 0 || size > MAX_S3_PART_BYTES {
            return Err(Error::TooLarge);
        }
        self.presign_with_headers(
            "PUT",
            key,
            vec![("partNumber", part_number.to_string()), ("uploadId", upload_id.to_owned())],
            vec![("content-length", size.to_string())],
            expires_seconds,
        )
    }

    pub fn presign_get(&self, key: &ObjectKey, expires_seconds: u64) -> Result<String, Error> {
        self.presign("GET", key, vec![("response-content-disposition", "attachment".to_owned())], expires_seconds)
    }

    pub fn presign_put(&self, key: &ObjectKey, content_type: &str, expires_seconds: u64) -> Result<String, Error> {
        if !valid_content_type(content_type) {
            return Err(Error::InvalidMetadata);
        }
        self.presign_with_headers("PUT", key, Vec::new(), vec![("content-type", content_type.to_owned())], expires_seconds)
    }

    /// Presigns a PUT with both content type and length bound into SigV4.
    pub fn presign_put_sized(&self, key: &ObjectKey, content_type: &str, size: u64, expires_seconds: u64) -> Result<String, Error> {
        if !valid_content_type(content_type) {
            return Err(Error::InvalidMetadata);
        }
        if size > MAX_S3_OBJECT_BYTES {
            return Err(Error::TooLarge);
        }
        self.presign_with_headers(
            "PUT",
            key,
            Vec::new(),
            vec![("content-length", size.to_string()), ("content-type", content_type.to_owned())],
            expires_seconds,
        )
    }

    pub fn presign_head(&self, key: &ObjectKey, expires_seconds: u64) -> Result<String, Error> {
        self.presign("HEAD", key, Vec::new(), expires_seconds)
    }

    pub fn presign_delete(&self, key: &ObjectKey, expires_seconds: u64) -> Result<String, Error> {
        self.presign("DELETE", key, Vec::new(), expires_seconds)
    }

    pub async fn complete_multipart(&self, upload: &MultipartUpload, parts: &[CompletedPart]) -> Result<(), Error> {
        validate_parts(&upload.upload_id, parts)?;
        let mut url = self.object_url(&upload.key)?;
        url.query_pairs_mut().append_pair("uploadId", &upload.upload_id);
        let body = complete_xml(parts);
        let response = self.signed_request(Method::POST, url, vec![("content-type", "application/xml".to_owned())], body.into_bytes()).await?;
        let body = bounded_body(response).await?;
        let xml = String::from_utf8(body).map_err(|e| Error::Protocol(e.to_string()))?;
        if xml.contains("<Error>") {
            return Err(Error::Protocol("object store rejected multipart completion".into()));
        }
        Ok(())
    }

    pub async fn abort_multipart(&self, upload: &MultipartUpload) -> Result<(), Error> {
        if !valid_upload_id(&upload.upload_id) {
            return Err(Error::InvalidMultipart);
        }
        let mut url = self.object_url(&upload.key)?;
        url.query_pairs_mut().append_pair("uploadId", &upload.upload_id);
        let response = self.signed_request(Method::DELETE, url, Vec::new(), Vec::new()).await?;
        if !response.status().is_success() {
            return Err(Error::Http(format!("object store returned {}", response.status())));
        }
        Ok(())
    }

    fn presign(&self, method: &str, key: &ObjectKey, params: Vec<(&str, String)>, expires_seconds: u64) -> Result<String, Error> {
        self.presign_with_headers(method, key, params, Vec::new(), expires_seconds)
    }

    fn presign_with_headers(
        &self,
        method: &str,
        key: &ObjectKey,
        mut params: Vec<(&str, String)>,
        headers: Vec<(&str, String)>,
        expires_seconds: u64,
    ) -> Result<String, Error> {
        if !(1..=MAX_PRESIGN_SECONDS).contains(&expires_seconds) {
            return Err(Error::InvalidConfig);
        }
        let url = self.object_url(key)?;
        let host = url.host_str().ok_or(Error::InvalidConfig)?;
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        let mut signed_headers = vec![("host".to_owned(), host)];
        signed_headers.extend(headers.into_iter().map(|(name, value)| (name.to_owned(), collapse_spaces(&value))));
        signed_headers.sort_by(|a, b| a.0.cmp(&b.0));
        let canonical_headers = signed_headers.iter().map(|(name, value)| format!("{name}:{value}\n")).collect::<String>();
        let signed_headers_list = signed_headers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(";");
        let (date, short_date) = aws_time(SystemTime::now())?;
        let scope = format!("{short_date}/{}/s3/aws4_request", self.config.region);
        params.extend([
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_owned()),
            ("X-Amz-Credential", format!("{}/{}", self.config.access_key, scope)),
            ("X-Amz-Date", date.clone()),
            ("X-Amz-Expires", expires_seconds.to_string()),
            ("X-Amz-SignedHeaders", signed_headers_list.clone()),
        ]);
        if let Some(token) = &self.config.session_token {
            params.push(("X-Amz-Security-Token", token.clone()));
        }
        let mut encoded: Vec<(String, String)> = params.into_iter().map(|(k, v)| (aws_encode(k), aws_encode(&v))).collect();
        encoded.sort();
        let canonical_query = encoded.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
        let canonical = format!("{method}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_headers_list}\nUNSIGNED-PAYLOAD", url.path());
        let to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
        let signature = hex(&hmac(&signing_key(&self.config.secret_key, &short_date, &self.config.region), to_sign.as_bytes()));
        let mut final_url = url;
        final_url.set_query(Some(&format!("{canonical_query}&X-Amz-Signature={signature}")));
        Ok(final_url.to_string())
    }

    async fn signed_request(&self, method: Method, url: Url, extra_headers: Vec<(&str, String)>, body: Vec<u8>) -> Result<reqwest::Response, Error> {
        let (date, short_date) = aws_time(SystemTime::now())?;
        let payload_hash = hex(&Sha256::digest(&body));
        let host = format!("{}{}", url.host_str().ok_or(Error::InvalidConfig)?, url.port().map(|v| format!(":{v}")).unwrap_or_default());
        let mut headers =
            vec![("host".to_owned(), host), ("x-amz-content-sha256".to_owned(), payload_hash.clone()), ("x-amz-date".to_owned(), date.clone())];
        if let Some(token) = &self.config.session_token {
            headers.push(("x-amz-security-token".into(), token.clone()));
        }
        for (name, value) in &extra_headers {
            headers.push((name.to_string(), value.trim().to_owned()));
        }
        headers.sort_by(|a, b| a.0.cmp(&b.0));
        let canonical_headers = headers.iter().map(|(k, v)| format!("{k}:{}\n", collapse_spaces(v))).collect::<String>();
        let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
        let canonical_query = canonical_query(&url);
        let canonical =
            format!("{}\n{}\n{}\n{}\n{}\n{}", method.as_str(), url.path(), canonical_query, canonical_headers, signed_headers, payload_hash);
        let scope = format!("{short_date}/{}/s3/aws4_request", self.config.region);
        let to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
        let signature = hex(&hmac(&signing_key(&self.config.secret_key, &short_date, &self.config.region), to_sign.as_bytes()));
        let authorization =
            format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}", self.config.access_key);
        let mut request = self.client.request(method, url).body(body).header("authorization", authorization);
        for (name, value) in headers {
            if name != "host" {
                request = request.header(name, value);
            }
        }
        let response = request.send().await.map_err(|e| Error::Http(e.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Http(format!("object store returned {}", response.status())));
        }
        Ok(response)
    }

    fn object_url(&self, key: &ObjectKey) -> Result<Url, Error> {
        let mut url = self.endpoint.clone();
        let base = url.path().trim_end_matches('/');
        let owner = self.owner_prefix.as_ref().map(|prefix| format!("{prefix}/")).unwrap_or_default();
        url.set_path(&format!("{base}/{}/{owner}{}", aws_encode(&self.config.bucket), key.as_str()));
        Ok(url)
    }
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, Error> {
    if response.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
        return Err(Error::Protocol("object-store response exceeded limit".into()));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| Error::Http(e.to_string()))? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(Error::Protocol("object-store response exceeded limit".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn read_metadata_file(file: File) -> Result<ObjectMetadata, Error> {
    if file.metadata()?.len() > 4096 {
        return Err(Error::Protocol("object metadata exceeded limit".into()));
    }
    let mut bytes = Vec::with_capacity(4096);
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(Error::Protocol("object metadata exceeded limit".into()));
    }
    let metadata: ObjectMetadata = serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(e.to_string()))?;
    if !valid_content_type(&metadata.content_type) || metadata.etag.len() != 64 || !metadata.etag.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Protocol("invalid object metadata".into()));
    }
    Ok(metadata)
}
fn open_directory(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}
fn open_directory_at(parent: &File, name: &str) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid path component"))?;
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, name);
        Err(io::Error::new(io::ErrorKind::Unsupported, "descriptor-relative object access requires Unix"))
    }
}
fn create_directory_at(parent: &File, name: &str) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let name = CString::new(name).map_err(|_| Error::UnsafeObjectPath)?;
        let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        let directory = open_directory_at(parent, name.to_str().map_err(|_| Error::UnsafeObjectPath)?)?;
        set_private_dir_handle(&directory)
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, name);
        Err(Error::UnsafeObjectPath)
    }
}
fn open_regular_at(parent: &File, name: &str) -> Result<File, Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = CString::new(name).map_err(|_| Error::UnsafeObjectPath)?;
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ELOOP) {
                Error::UnsafeObjectPath
            } else {
                error.into()
            });
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(Error::UnsafeObjectPath);
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, name);
        Err(Error::UnsafeObjectPath)
    }
}
fn ensure_quota_lock(root: &File) -> Result<(), Error> {
    let lock = open_quota_lock(root)?;
    set_private_file_handle(&lock)
}
fn acquire_quota_lock(root: &File) -> Result<File, Error> {
    use std::os::fd::AsRawFd;
    let lock = open_quota_lock(root)?;
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::WouldBlock { Err(Error::Capacity) } else { Err(error.into()) };
    }
    Ok(lock)
}
fn open_quota_lock(root: &File) -> Result<File, Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = CString::new(".quota-lock").expect("static path segment");
        let fd = unsafe { libc::openat(root.as_raw_fd(), name.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(Error::UnsafeObjectPath);
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err(Error::UnsafeObjectPath)
    }
}
fn set_private_file_handle(file: &File) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}
fn directory_names(directory: &File, max_entries: usize) -> Result<Vec<String>, Error> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        use std::os::fd::AsRawFd;
        let current = std::ffi::CString::new(".").expect("static path segment");
        let duplicate = unsafe { libc::openat(directory.as_raw_fd(), current.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            unsafe { libc::close(duplicate) };
            return Err(io::Error::last_os_error().into());
        }
        let mut entries = Vec::new();
        loop {
            clear_errno();
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                if error.raw_os_error().is_some_and(|errno| errno != 0) {
                    unsafe { libc::closedir(stream) };
                    return Err(error.into());
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            let name = name.to_string_lossy();
            if name == "." || name == ".." {
                continue;
            }
            if entries.len() >= max_entries {
                unsafe { libc::closedir(stream) };
                return Err(Error::QuotaExceeded);
            }
            entries.push(name.into_owned());
        }
        unsafe { libc::closedir(stream) };
        Ok(entries)
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, max_entries);
        Err(Error::UnsafeObjectPath)
    }
}
#[cfg(target_os = "macos")]
fn clear_errno() {
    unsafe { *libc::__error() = 0 };
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn clear_errno() {
    unsafe { *libc::__errno_location() = 0 };
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn clear_errno() {}
fn create_private_file_at(parent: &File, name: &str) -> Result<File, Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = CString::new(name).map_err(|_| Error::UnsafeObjectPath)?;
        let fd = unsafe {
            libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600)
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, name);
        Err(Error::UnsafeObjectPath)
    }
}
fn unlink_file_at(parent: &File, directory: &str, name: &str) -> Result<(), Error> {
    let directory = open_directory_at(parent, directory)?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let name = CString::new(name).map_err(|_| Error::UnsafeObjectPath)?;
        let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, name);
        Err(Error::UnsafeObjectPath)
    }
}
fn unlink_directory_at(parent: &File, name: &str) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let directory = match open_directory_at(parent, name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in ["data", "metadata.json"] {
            let entry = CString::new(entry).expect("static path segment");
            if unsafe { libc::unlinkat(directory.as_raw_fd(), entry.as_ptr(), 0) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error.into());
                }
            }
        }
        drop(directory);
        let name = CString::new(name).map_err(|_| Error::UnsafeObjectPath)?;
        let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(error.into()) }
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, name);
        Err(Error::UnsafeObjectPath)
    }
}
fn rename_at(parent: &File, source: &str, destination: &str) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        let source = CString::new(source).map_err(|_| Error::UnsafeObjectPath)?;
        let destination = CString::new(destination).map_err(|_| Error::UnsafeObjectPath)?;
        if unsafe { libc::renameat(parent.as_raw_fd(), source.as_ptr(), parent.as_raw_fd(), destination.as_ptr()) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, source, destination);
        Err(Error::UnsafeObjectPath)
    }
}
fn set_private_dir_handle(directory: &File) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}
fn valid_content_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else { return false };
    !kind.is_empty()
        && !subtype.is_empty()
        && value.len() <= 127
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'+' | b'.' | b'-'))
}
fn valid_bucket(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}
fn valid_segment(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn valid_upload_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}
fn validate_parts(upload_id: &str, parts: &[CompletedPart]) -> Result<(), Error> {
    if !valid_upload_id(upload_id) || parts.is_empty() || parts.len() > 10_000 {
        return Err(Error::InvalidMultipart);
    }
    let mut previous = 0;
    for part in parts {
        if part.number == 0
            || part.number > 10_000
            || part.number <= previous
            || part.etag.is_empty()
            || part.etag.len() > 256
            || part.etag.chars().any(char::is_control)
        {
            return Err(Error::InvalidMultipart);
        }
        previous = part.number;
    }
    Ok(())
}
fn complete_xml(parts: &[CompletedPart]) -> String {
    let mut xml = String::from("<CompleteMultipartUpload>");
    for part in parts {
        xml.push_str(&format!("<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>", part.number, xml_escape(&part.etag)));
    }
    xml.push_str("</CompleteMultipartUpload>");
    xml
}
fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}
fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}
fn canonical_query(url: &Url) -> String {
    let mut pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (aws_encode(&k), aws_encode(&v))).collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}
fn aws_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}
fn collapse_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = [0x36u8; 64];
    let mut outer_key = [0x5cu8; 64];
    for i in 0..64 {
        inner_key[i] ^= block[i];
        outer_key[i] ^= block[i];
    }
    let mut inner = Sha256::new();
    inner.update(inner_key);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_key);
    outer.update(inner);
    outer.finalize().into()
}
fn signing_key(secret: &str, date: &str, region: &str) -> [u8; 32] {
    let first = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let second = hmac(&first, region.as_bytes());
    let third = hmac(&second, b"s3");
    hmac(&third, b"aws4_request")
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn aws_time(time: SystemTime) -> Result<(String, String), Error> {
    let seconds = time.duration_since(UNIX_EPOCH).map_err(|_| Error::InvalidConfig)?.as_secs();
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3600;
    let minute = day_seconds % 3600 / 60;
    let second = day_seconds % 60;
    let short = format!("{year:04}{month:02}{day:02}");
    Ok((format!("{short}T{hour:02}{minute:02}{second:02}Z"), short))
}
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod sigv4_tests {
    use super::*;

    #[test]
    fn presigned_get_defaults_to_attachment_without_overriding_object_content_type() {
        let store = S3Store::new(S3Config::new("https://objects.example.test", "bucket", "us-east-1", "ak", "sk")).unwrap();
        let key = ObjectKey::parse("00000000-0000-4000-8000-000000000001").unwrap();
        let url = store.presign_get(&key, 300).unwrap();
        assert!(url.contains("response-content-disposition=attachment"));
        assert!(url.contains("X-Amz-Signature="));
        assert!(!url.contains("response-content-type="));
    }

    #[test]
    fn signing_key_matches_the_official_s3_presign_example() {
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let canonical = "GET\n/test.txt\nX-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\nhost:examplebucket.s3.amazonaws.com\n\nhost\nUNSIGNED-PAYLOAD";
        let to_sign =
            format!("AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n{}", hex(&Sha256::digest(canonical.as_bytes())));
        let signature = hex(&hmac(&signing_key(secret, "20130524", "us-east-1"), to_sign.as_bytes()));
        assert_eq!(signature, "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404");
    }
}
