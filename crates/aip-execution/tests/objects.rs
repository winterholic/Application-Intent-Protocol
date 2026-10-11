use aip_execution::objects::{Error, LocalConfig, LocalStore, ObjectKey, S3Config, S3Store};
use std::io::{self, Read};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct GeneratedReader {
    remaining: usize,
    chunk: usize,
    calls: Arc<AtomicUsize>,
}

impl Read for GeneratedReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        let n = output.len().min(self.chunk).min(self.remaining);
        output[..n].fill(b'x');
        self.remaining -= n;
        Ok(n)
    }
}

fn temp_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("aip-objects-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn object_entries(path: &std::path::Path) -> usize {
    std::fs::read_dir(path).unwrap().filter_map(Result::ok).filter(|entry| entry.file_name() != ".quota-lock").count()
}

#[test]
fn local_store_streams_large_objects_and_returns_only_opaque_keys() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir).with_max_bytes(3 * 1024 * 1024)).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let reader = GeneratedReader { remaining: 2 * 1024 * 1024 + 17, chunk: 8191, calls: calls.clone() };

    let (key, metadata) = store.put(reader, "application/octet-stream").unwrap();
    assert!(calls.load(Ordering::Relaxed) > 200);
    assert_eq!(metadata.size, 2 * 1024 * 1024 + 17);
    assert!(!key.to_string().contains(dir.to_str().unwrap()));
    assert_eq!(store.stat(&key).unwrap().unwrap(), metadata);

    let mut output = Vec::new();
    store.get(&key, &mut output).unwrap();
    assert_eq!(output.len(), metadata.size as usize);
    assert!(output.iter().all(|byte| *byte == b'x'));
    assert!(store.delete(&key).unwrap());
    assert!(store.stat(&key).unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_store_enforces_size_limit_and_does_not_publish_partial_objects() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir).with_max_bytes(1024)).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let reader = GeneratedReader { remaining: 1025, chunk: 127, calls };
    assert!(matches!(store.put(reader, "application/octet-stream"), Err(Error::TooLarge)));
    assert_eq!(object_entries(&dir), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_store_rejects_symlinked_object_and_invalid_opaque_keys() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir)).unwrap();
    let key = ObjectKey::parse("00000000-0000-4000-8000-000000000001").unwrap();
    let outside = dir.with_extension("outside");
    std::fs::write(&outside, b"outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, dir.join(key.as_str())).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, dir.join(key.as_str())).unwrap();

    assert!(matches!(store.stat(&key), Err(Error::UnsafeObjectPath)));
    assert!(ObjectKey::parse("../../etc/passwd").is_err());
    assert!(ObjectKey::parse("not-a-uuid").is_err());
    std::fs::remove_file(dir.join(key.as_str())).unwrap();
    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn local_store_keeps_operations_anchored_to_open_root_after_path_replacement() {
    use std::{fs, os::unix::fs::symlink};
    let temp = temp_dir();
    let root = temp.join("objects");
    let moved = temp.join("objects-moved");
    let outside = temp.join("outside");
    fs::create_dir(&outside).unwrap();
    let store = LocalStore::new(LocalConfig::new(&root)).unwrap();
    fs::rename(&root, &moved).unwrap();
    symlink(&outside, &root).unwrap();

    let (key, _) = store.put(&b"safe"[..], "application/octet-stream").unwrap();
    assert_eq!(store.get(&key, Vec::new()).unwrap().size, 4);
    assert!(moved.join(key.as_str()).is_dir());
    assert!(!outside.join(key.as_str()).exists());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn s3_presigns_multipart_parts_with_bounded_expiry_and_explicit_https_endpoint() {
    let config = S3Config::new("https://objects.example.test", "large-files", "ap-northeast-2", "explicit-access-key", "explicit-secret-key");
    let s3 = S3Store::new(config).unwrap();
    let key = ObjectKey::parse("00000000-0000-4000-8000-000000000001").unwrap();
    let url = s3.presign_part(&key, "upload-id+opaque", 1, 900).unwrap();
    assert!(url.starts_with("https://objects.example.test/large-files/"));
    assert!(url.contains("partNumber=1"));
    assert!(url.contains("uploadId=upload-id%2Bopaque"));
    assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
    assert!(url.contains("X-Amz-Signature="));
    assert!(s3.presign_part(&key, "upload-id", 1, 604801).is_err());
    let put_url = s3.presign_put(&key, "application/octet-stream", 60).unwrap();
    assert!(put_url.contains("X-Amz-SignedHeaders=content-type%3Bhost"));
    let sized = s3.presign_put_sized(&key, "application/octet-stream", 1024, 60).unwrap();
    assert!(sized.contains("X-Amz-SignedHeaders=content-length%3Bcontent-type%3Bhost"));
    assert!(s3.presign_put_sized(&key, "application/octet-stream", u64::MAX, 60).is_err());
    let part = s3.presign_part_sized(&key, "upload-id", 1, 2048, 60).unwrap();
    assert!(part.contains("X-Amz-SignedHeaders=content-length%3Bhost"));
    assert!(S3Store::new(S3Config::new("http://objects.example.test", "bucket", "us-east-1", "a", "b")).is_err());
}

#[test]
fn s3_owner_namespace_scopes_generated_object_urls() {
    let s3 = S3Store::new(S3Config::new("https://objects.example.test", "large-files", "ap-northeast-2", "ak", "sk")).unwrap();
    let key = ObjectKey::generate();
    let alice = s3.for_owner("issuer|alice").unwrap();
    let bob = s3.for_owner("issuer|bob").unwrap();
    let alice_url = alice.presign_get(&key, 60).unwrap();
    let bob_url = bob.presign_get(&key, 60).unwrap();

    assert_ne!(alice_url, bob_url);
    assert!(alice_url.contains("/large-files/"));
    assert!(alice.for_owner("\n").is_err());
    assert!(alice.for_owner(&"x".repeat(2049)).is_err());
}

#[test]
fn local_store_rejects_invalid_content_type_and_upload_key_is_generated() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir)).unwrap();
    assert!(store.put(&b"data"[..], "text/plain\r\nX-Evil: 1").is_err());
    let (key, _) = store.put(&b"data"[..], "text/plain").unwrap();
    assert_eq!(ObjectKey::parse(&key.as_str()).unwrap(), key);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_upload_drop_discards_staging_and_owner_namespaces_are_isolated() {
    let dir = temp_dir();
    let base = LocalStore::new(LocalConfig::new(&dir)).unwrap();
    {
        let mut pending = base.begin_upload("text/plain").unwrap();
        pending.write_chunk(b"not published").unwrap();
    }
    assert_eq!(object_entries(&dir), 0);

    let alice = base.for_owner("issuer|alice").unwrap();
    let bob = base.for_owner("issuer|bob").unwrap();
    let (key, _) = alice.put(&b"private"[..], "text/plain").unwrap();
    assert_eq!(bob.stat(&key).unwrap(), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_upload_writer_enforces_limit_and_cleans_failed_staging() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir).with_max_bytes(3)).unwrap();
    let mut upload = store.begin_upload("text/plain").unwrap();
    assert!(upload.write_chunk(b"four").is_err());
    assert!(matches!(upload.finish(), Err(Error::TooLarge)));
    assert_eq!(object_entries(&dir), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_store_enforces_owner_byte_and_object_quotas() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir).with_max_bytes(8).with_owner_quota(5, 2)).unwrap();
    let (key, _) = store.put(&b"1234"[..], "text/plain").unwrap();
    assert!(matches!(store.put(&b"12"[..], "text/plain"), Err(Error::QuotaExceeded)));
    assert_eq!(store.stat(&key).unwrap().unwrap().size, 4);
    std::fs::remove_dir_all(dir).unwrap();

    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir).with_owner_quota(100, 1)).unwrap();
    store.put(&b""[..], "text/plain").unwrap();
    assert!(matches!(store.begin_upload("text/plain"), Err(Error::QuotaExceeded)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_store_rejects_a_second_same_namespace_upload_without_blocking() {
    let dir = temp_dir();
    let store = LocalStore::new(LocalConfig::new(&dir)).unwrap();
    let other_store = LocalStore::new(LocalConfig::new(&dir)).unwrap();
    let pending = store.begin_upload("text/plain").unwrap();
    assert!(matches!(other_store.begin_upload("text/plain"), Err(Error::Capacity)));
    drop(pending);
    let next = other_store.begin_upload("text/plain");
    assert!(next.is_ok());
    drop(next);
    std::fs::remove_dir_all(dir).unwrap();
}
