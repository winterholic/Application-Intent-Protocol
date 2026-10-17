//! Cloud Storage XML API adapter using Google's documented S3 interoperability.
//!
//! This is an HMAC-key backend, not the native Cloud Storage client library or
//! ADC integration. Requests use the XML API and AWS4-HMAC-SHA256 extensions.

use crate::objects::{CompletedPart, Error, MultipartUpload, ObjectKey, S3Config, S3Store};

/// Credentials for Google's documented S3-compatible XML API.
///
/// The endpoint and signing region are fixed to Google's documented values so
/// callers cannot accidentally treat this as an arbitrary S3-compatible host.
#[derive(Clone)]
pub struct GcsConfig {
    bucket: String,
    access_id: String,
    secret: String,
    owner_quota: Option<(u64, usize)>,
}

impl GcsConfig {
    pub fn new(bucket: impl Into<String>, access_id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self { bucket: bucket.into(), access_id: access_id.into(), secret: secret.into(), owner_quota: None }
    }

    /// Per-owner ceiling on reserved plus stored bytes and on object count, enforced by the server's object ledger.
    pub fn with_owner_quota(mut self, max_total_bytes: u64, max_objects: usize) -> Self {
        self.owner_quota = Some((max_total_bytes, max_objects));
        self
    }
}

/// Google Cloud Storage XML API through S3 HMAC interoperability.
///
/// This backend does not use Application Default Credentials or the native
/// JSON API. Credential changes take effect when a new store is constructed.
#[derive(Clone)]
pub struct GcsStore {
    inner: S3Store,
}

impl GcsStore {
    pub fn new(config: GcsConfig) -> Result<Self, Error> {
        let mut s3 = S3Config::new("https://storage.googleapis.com", config.bucket, "auto", config.access_id, config.secret);
        if let Some((bytes, objects)) = config.owner_quota {
            s3 = s3.with_owner_quota(bytes, objects);
        }
        let inner = S3Store::new(s3)?;
        Ok(Self { inner })
    }

    pub fn for_owner(&self, owner: &str) -> Result<Self, Error> {
        Ok(Self { inner: self.inner.for_owner(owner)? })
    }

    pub fn presign_get(&self, key: &ObjectKey, expires_seconds: u64) -> Result<String, Error> {
        self.inner.presign_get(key, expires_seconds)
    }

    pub fn presign_put_sized(&self, key: &ObjectKey, content_type: &str, size: u64, expires_seconds: u64) -> Result<String, Error> {
        self.inner.presign_put_sized(key, content_type, size, expires_seconds)
    }

    pub fn presign_head(&self, key: &ObjectKey, expires_seconds: u64) -> Result<String, Error> {
        self.inner.presign_head(key, expires_seconds)
    }

    pub fn presign_delete(&self, key: &ObjectKey, expires_seconds: u64) -> Result<String, Error> {
        self.inner.presign_delete(key, expires_seconds)
    }

    pub fn presign_part_sized(&self, key: &ObjectKey, upload_id: &str, part_number: u16, size: u64, expires_seconds: u64) -> Result<String, Error> {
        self.inner.presign_part_sized(key, upload_id, part_number, size, expires_seconds)
    }

    pub async fn create_multipart(&self, content_type: &str) -> Result<MultipartUpload, Error> {
        self.inner.create_multipart(content_type).await
    }

    pub async fn complete_multipart(&self, upload: &MultipartUpload, parts: &[CompletedPart]) -> Result<(), Error> {
        self.inner.complete_multipart(upload, parts).await
    }

    pub async fn abort_multipart(&self, upload: &MultipartUpload) -> Result<(), Error> {
        self.inner.abort_multipart(upload).await
    }

    pub fn s3_compatible_store(&self) -> &S3Store {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ObjectKey;
    use sha2::{Digest, Sha256};

    fn test_store() -> GcsStore {
        GcsStore::new(GcsConfig::new("sample-bucket", "GOOGACCESSID", "a-secret-key")).unwrap()
    }

    #[test]
    fn gcs_uses_google_xml_endpoint_and_aws_interoperability_scope() {
        let store = test_store();
        let key = ObjectKey::generate();
        let put = store.presign_put_sized(&key, "application/octet-stream", 37, 300).unwrap();
        let parsed = reqwest::Url::parse(&put).unwrap();
        assert_eq!(parsed.host_str(), Some("storage.googleapis.com"));
        assert!(parsed.path().starts_with("/sample-bucket/"));
        let credential = parsed.query_pairs().find(|(name, _)| name == "X-Amz-Credential").unwrap().1;
        assert!(credential.contains("/auto/s3/aws4_request"));
        let signed = parsed.query_pairs().find(|(name, _)| name == "X-Amz-SignedHeaders").unwrap().1;
        assert!(signed.split(';').any(|header| header == "content-length"));
    }

    #[test]
    fn owner_scopes_cover_object_and_multipart_signatures() {
        let base = test_store();
        let alice = base.for_owner("principal-alice").unwrap();
        let bob = base.for_owner("principal-bob").unwrap();
        let upload = ObjectKey::generate();
        let alice_put = alice.presign_put_sized(&upload, "text/plain", 12, 300).unwrap();
        let bob_put = bob.presign_put_sized(&upload, "text/plain", 12, 300).unwrap();
        assert_ne!(alice_put, bob_put);
        let alice_part = alice.presign_part_sized(&upload, "upload-id-123", 1, 12, 300).unwrap();
        let bob_part = bob.presign_part_sized(&upload, "upload-id-123", 1, 12, 300).unwrap();
        assert_ne!(alice_part, bob_part);
        let namespace = hex(&Sha256::digest(b"principal-alice"));
        assert!(alice_put.contains(&namespace));
        assert!(alice_part.contains(&namespace));
        assert!(!bob_put.contains(&namespace));
    }

    #[test]
    fn gcs_presigns_supported_object_methods_on_xml_api() {
        let store = test_store();
        let key = ObjectKey::generate();
        for (url, method) in [
            (store.presign_get(&key, 300).unwrap(), "GET"),
            (store.presign_head(&key, 300).unwrap(), "HEAD"),
            (store.presign_delete(&key, 300).unwrap(), "DELETE"),
        ] {
            let parsed = reqwest::Url::parse(&url).unwrap();
            assert_eq!(parsed.host_str(), Some("storage.googleapis.com"));
            assert!(parsed.query_pairs().any(|(name, value)| name == "X-Amz-SignedHeaders" && value == "host"));
            let scope = parsed.query_pairs().find(|(name, _)| name == "X-Amz-Credential").unwrap().1;
            assert!(scope.contains("/auto/s3/aws4_request"), "{method} must use Cloud Storage's documented S3 scope");
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
