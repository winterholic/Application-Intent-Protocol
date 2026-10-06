//! Object storage for `s3.Object` fields. The development backend writes to a
//! local directory; objects are staged during the transaction and promoted
//! after commit, or removed if the transaction rolls back.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Upload {
    pub filename: String,
    pub content_type: Option<String>,
    pub bytes: bytes::Bytes,
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    pub root: PathBuf,
}

fn safe_bucket(b: &str) -> String {
    b.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

impl ObjectStore {
    pub fn new(root: PathBuf) -> Self {
        ObjectStore { root }
    }

    pub async fn stage(&self, bucket: &str, up: &Upload) -> std::io::Result<String> {
        let ext =
            up.filename.rsplit_once('.').map(|(_, e)| e.to_lowercase()).filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()));
        let key = format!("{}/{}{}", safe_bucket(bucket), uuid::Uuid::new_v4(), ext.map(|e| format!(".{e}")).unwrap_or_default());
        let path = self.root.join(&key);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, &up.bytes).await?;
        Ok(key)
    }

    /// Reserves a new key for a file the runtime writes itself (job output).
    pub async fn create(&self, bucket: &str, ext: &str) -> std::io::Result<(String, PathBuf)> {
        let key = format!("{}/{}.{}", safe_bucket(bucket), uuid::Uuid::new_v4(), safe_bucket(ext));
        let path = self.root.join(&key);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok((key, path))
    }

    pub fn path(&self, key: &str) -> Option<PathBuf> {
        if key.contains("..") || key.starts_with('/') {
            return None;
        }
        Some(self.root.join(key))
    }

    pub async fn remove(&self, key: &str) {
        if let Some(p) = self.path(key) {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

/// Checks size and extension limits declared on the parameter type.
pub fn check_upload(up: &Upload, max_bytes: Option<u64>, types: &[String]) -> Result<(), String> {
    if let Some(m) = max_bytes
        && up.bytes.len() as u64 > m
    {
        return Err(format!("file is larger than {} bytes", m));
    }
    if !types.is_empty() {
        let ext = up.filename.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        if !types.iter().any(|t| t.eq_ignore_ascii_case(&ext)) {
            return Err(format!("file type '{ext}' is not one of {}", types.join(", ")));
        }
    }
    Ok(())
}
