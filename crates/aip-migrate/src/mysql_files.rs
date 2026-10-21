//! MySQL 8.4 / MariaDB 11.4 managed file bytes (WBS 7.2.1.3): the PostgreSQL `aip_execution::files::Store` contract over InnoDB.
//!
//! Same contract, same types: bounded file bytes in the database under `(owner, bucket, name, object key)`, opaque UUID handles
//! (callers never see a path or a URL), a re-put of identical bytes converges on the same handle while different bytes under the
//! same name are a conflict, and every read re-checks the owner and the expiry on the database clock. What changes is how InnoDB
//! gets there: the conditional upsert is an `INSERT IGNORE` followed by a locking read of the row the key already has.
use aip_execution::files::{Error, MAX_FILE_BYTES, ManagedFile};
use mysql_async::Value as MyValue;
use mysql_async::prelude::Queryable;
use uuid::Uuid;

fn db(error: mysql_async::Error) -> Error {
    Error::Database(match &error {
        mysql_async::Error::Server(server) => format!("mysql errno {}", server.code),
        _ => "mysql connection error".to_string(),
    })
}

/// A Unix-seconds instant as a UTC `DATETIME(6)`, independent of the session time zone.
const FROM_UNIX: &str = "DATE_ADD('1970-01-01 00:00:00', INTERVAL ? SECOND)";
const TO_UNIX: &str = "CAST(TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', expires_at) AS SIGNED)";

/// handle, bucket, name, media type, bytes, expiry as Unix seconds.
type FileRow = (String, String, String, String, Vec<u8>, Option<i64>);

#[derive(Clone, Debug)]
pub struct MysqlFileStore {
    schema: String,
}

impl MysqlFileStore {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        let bytes = schema.as_bytes();
        if bytes.len() > 63
            || !bytes.starts_with(b"aip_")
            || !bytes.get(4).is_some_and(u8::is_ascii_lowercase)
            || !bytes[5..].iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        {
            return Err(Error::InvalidSchema);
        }
        Ok(Self { schema })
    }

    fn table(&self) -> String {
        format!("`{}`.`aip_managed_files`", self.schema)
    }

    /// Stores a file under a caller-owned bucket and name. The owner comes from the trusted server identity, never from an
    /// extension request. `object_key` scopes a job's own copy of a name so a retry converges on it.
    #[allow(clippy::too_many_arguments)]
    pub async fn put_scoped<Q: Queryable>(
        &self,
        conn: &mut Q,
        owner: &str,
        bucket: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
        expires_at: Option<i64>,
        object_key: &str,
    ) -> Result<ManagedFile, Error> {
        if object_key.len() > 100 || object_key.chars().any(char::is_control) {
            return Err(Error::InvalidFile);
        }
        if !valid_owner(owner) || !valid_component(bucket) || !valid_component(name) || !valid_media_type(media_type) || bytes.len() > MAX_FILE_BYTES
        {
            return Err(Error::InvalidFile);
        }
        let table = self.table();
        let handle = Uuid::new_v4().to_string();
        let expiry = if expires_at.is_some() { FROM_UNIX } else { "NULL" };
        let mut params: Vec<MyValue> =
            vec![handle.clone().into(), owner.into(), bucket.into(), name.into(), media_type.into(), bytes.to_vec().into(), object_key.into()];
        if let Some(at) = expires_at {
            params.push(at.into());
        }
        conn.exec_drop(
            format!(
                "INSERT IGNORE INTO {table} (handle, owner, bucket, name, media_type, content, object_key, expires_at, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, {expiry}, UTC_TIMESTAMP(6))"
            ),
            params,
        )
        .await
        .map_err(db)?;
        let inserted: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        let handle = if inserted == Some(1) {
            handle
        } else {
            // The name already has a file: the same bytes under it are the same file, anything else is a conflict. A locking
            // read, because a plain one in a repeatable-read transaction would not see a row committed after its snapshot.
            let row: Option<(String, String, Vec<u8>, Option<i64>)> = conn
                .exec_first(
                    format!(
                        "SELECT handle, media_type, content, {TO_UNIX} FROM {table} WHERE owner = ? AND bucket = ? AND name = ? AND object_key = ? FOR UPDATE"
                    ),
                    (owner, bucket, name, object_key),
                )
                .await
                .map_err(db)?;
            let (existing, old_type, old_bytes, old_expiry) = row.ok_or_else(|| Error::Database("conflicting file disappeared".into()))?;
            if old_type != media_type || old_bytes != bytes || old_expiry != expires_at {
                return Err(Error::FileConflict);
            }
            existing
        };
        Ok(ManagedFile {
            handle: Uuid::parse_str(&handle).map_err(|_| Error::InvalidFile)?,
            bucket: bucket.to_owned(),
            name: name.to_owned(),
            media_type: media_type.to_owned(),
            bytes: bytes.to_vec(),
            expires_at,
        })
    }

    /// Fetches bytes only after checking the current owner and expiry in the database on every call. A bucket narrows the
    /// match when the caller expects one.
    pub async fn get<Q: Queryable>(&self, conn: &mut Q, owner: &str, handle: &Uuid, bucket: Option<&str>) -> Result<Option<ManagedFile>, Error> {
        if !valid_owner(owner) || bucket.is_some_and(|bucket| !valid_component(bucket)) {
            return Ok(None);
        }
        let row: Option<FileRow> = conn
            .exec_first(
                format!(
                    "SELECT handle, bucket, name, media_type, content, {TO_UNIX} FROM {} WHERE handle = ? AND owner = ? \
                     AND (? IS NULL OR bucket = ?) AND (expires_at IS NULL OR expires_at > UTC_TIMESTAMP(6))",
                    self.table()
                ),
                (handle.to_string(), owner, bucket, bucket),
            )
            .await
            .map_err(db)?;
        row.map(|row| {
            Ok(ManagedFile {
                handle: Uuid::parse_str(&row.0).map_err(|_| Error::InvalidFile)?,
                bucket: row.1,
                name: row.2,
                media_type: row.3,
                bytes: row.4,
                expires_at: row.5,
            })
        })
        .transpose()
    }

    /// Removes only a file owned by the current principal.
    pub async fn delete<Q: Queryable>(&self, conn: &mut Q, owner: &str, handle: &Uuid) -> Result<bool, Error> {
        if !valid_owner(owner) {
            return Ok(false);
        }
        conn.exec_drop(format!("DELETE FROM {} WHERE handle = ? AND owner = ?", self.table()), (handle.to_string(), owner)).await.map_err(db)?;
        let deleted: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        Ok(deleted == Some(1))
    }

    /// Drops an owner's expired files, so they stop counting against the quota.
    pub async fn expire_owner<Q: Queryable>(&self, conn: &mut Q, owner: &str) -> Result<(), Error> {
        conn.exec_drop(format!("DELETE FROM {} WHERE owner = ? AND expires_at <= UTC_TIMESTAMP(6)", self.table()), (owner,)).await.map_err(db)
    }

    /// Deletes expired files of every owner in one bounded batch; repeated calls drain a large backlog. Returns how many went.
    pub async fn cleanup_expired<Q: Queryable>(&self, conn: &mut Q, limit: u32) -> Result<u64, Error> {
        conn.exec_drop(format!("DELETE FROM {} WHERE expires_at IS NOT NULL AND expires_at <= UTC_TIMESTAMP(6) LIMIT {limit}", self.table()), ())
            .await
            .map_err(db)?;
        let deleted: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        Ok(deleted.unwrap_or(0))
    }

    /// The owner's live files: how many, how many bytes, and whether this exact name already exists (a re-put is never a new file).
    pub async fn usage<Q: Queryable>(
        &self,
        conn: &mut Q,
        owner: &str,
        bucket: &str,
        name: &str,
        object_key: &str,
    ) -> Result<(i64, i64, bool), Error> {
        let row: Option<(i64, i64, i64)> = conn
            .exec_first(
                format!(
                    "SELECT COUNT(*), CAST(COALESCE(SUM(LENGTH(content)), 0) AS SIGNED), \
                       CAST(COALESCE(MAX(bucket = ? AND name = ? AND object_key = ?), 0) AS SIGNED) FROM {} WHERE owner = ?",
                    self.table()
                ),
                (bucket, name, object_key, owner),
            )
            .await
            .map_err(db)?;
        let (count, bytes, existing) = row.ok_or_else(|| Error::Database("usage".into()))?;
        Ok((count, bytes, existing != 0))
    }

    /// When the named file expires, so a re-put keeps its original deadline instead of extending it.
    pub async fn expiry_of<Q: Queryable>(&self, conn: &mut Q, owner: &str, bucket: &str, name: &str, object_key: &str) -> Result<Option<i64>, Error> {
        let row: Option<Option<i64>> = conn
            .exec_first(
                format!("SELECT {TO_UNIX} FROM {} WHERE owner = ? AND bucket = ? AND name = ? AND object_key = ?", self.table()),
                (owner, bucket, name, object_key),
            )
            .await
            .map_err(db)?;
        Ok(row.flatten())
    }
}

/// AIP-owned additive infrastructure shared by bootstrap and adoption. Named `aip_managed_files` so it cannot be mistaken for the
/// PostgreSQL `aip_files`.
pub(crate) fn file_store_ddl(meta_schema: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_managed_files` (\
           handle CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY, \
           owner VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, bucket VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           name VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, object_key VARCHAR(100) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT '', \
           media_type VARCHAR(127) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, content LONGBLOB NOT NULL, expires_at DATETIME(6) NULL, \
           created_at DATETIME(6) NOT NULL, UNIQUE KEY aip_managed_files_name (owner, bucket, name, object_key), \
           KEY aip_managed_files_expiry (expires_at), CONSTRAINT aip_managed_files_size CHECK (LENGTH(content) <= {MAX_FILE_BYTES})) ENGINE=InnoDB"
    )
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn valid_media_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else { return false };
    !kind.is_empty()
        && !subtype.is_empty()
        && value.len() <= 127
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'+' | b'.' | b'-'))
}
