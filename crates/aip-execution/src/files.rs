//! Managed, bounded file bytes stored in PostgreSQL. Handles are opaque UUIDs;
//! callers never receive a filesystem path or an object-store URL.

use std::fmt;
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub const MAX_FILE_BYTES: usize = 1024 * 1024;
const TABLE: &str = "aip_files";

#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidSchema,
    InvalidFile,
    FileConflict,
    Database(String),
}

impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Database(format!("{error:?}"))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedFile {
    pub handle: Uuid,
    pub bucket: String,
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Store {
    schema: String,
}

impl Store {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        if !valid_schema(&schema) {
            return Err(Error::InvalidSchema);
        }
        Ok(Self { schema })
    }

    fn table(&self) -> String {
        format!("{}.{}", self.schema, TABLE)
    }

    pub fn ddl(&self) -> String {
        format!(
            "CREATE TABLE IF NOT EXISTS {} (\
             handle uuid PRIMARY KEY, owner text NOT NULL, bucket text NOT NULL, name text NOT NULL, object_key text NOT NULL DEFAULT '', \
             media_type text NOT NULL, content bytea NOT NULL CHECK (octet_length(content) <= {MAX_FILE_BYTES}), \
             expires_at timestamptz, created_at timestamptz NOT NULL DEFAULT clock_timestamp(), \
             UNIQUE (owner,bucket,name,object_key))",
            self.table()
        )
    }

    /// Stores a file under a caller-owned bucket and name. The owner comes from
    /// the trusted server identity, never from an extension request.
    #[allow(clippy::too_many_arguments)]
    pub async fn put<C: GenericClient>(
        &self,
        client: &C,
        owner: &str,
        bucket: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
        expires_at: Option<i64>,
    ) -> Result<ManagedFile, Error> {
        self.put_scoped(client, owner, bucket, name, media_type, bytes, expires_at, "").await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn put_scoped<C: GenericClient>(
        &self,
        client: &C,
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
        let handle = Uuid::new_v4();
        let table = self.table();
        let inserted = client
            .query_opt(
                &format!(
                    "INSERT INTO {table} (handle,owner,bucket,name,media_type,content,expires_at,object_key) \
                     VALUES ($1,$2,$3,$4,$5,$6,CASE WHEN $7::bigint IS NULL THEN NULL ELSE to_timestamp($7::double precision) END,$8) \
                     ON CONFLICT (owner,bucket,name,object_key) DO UPDATE SET handle={table}.handle \
                     WHERE {table}.media_type=EXCLUDED.media_type AND {table}.content=EXCLUDED.content \
                       AND {table}.expires_at IS NOT DISTINCT FROM EXCLUDED.expires_at RETURNING handle"
                ),
                &[&handle, &owner, &bucket, &name, &media_type, &bytes, &expires_at, &object_key],
            )
            .await?;
        let Some(inserted) = inserted else {
            return Err(Error::FileConflict);
        };
        let handle: Uuid = inserted.get(0);
        Ok(ManagedFile {
            handle,
            bucket: bucket.to_owned(),
            name: name.to_owned(),
            media_type: media_type.to_owned(),
            bytes: bytes.to_vec(),
            expires_at,
        })
    }

    /// Fetches bytes only after checking the current owner and expiry in the
    /// database on every call.
    pub async fn get<C: GenericClient>(&self, client: &C, owner: &str, handle: &Uuid) -> Result<Option<ManagedFile>, Error> {
        if !valid_owner(owner) {
            return Ok(None);
        }
        let row = client
            .query_opt(
                &format!(
                    "SELECT handle,bucket,name,media_type,content,extract(epoch from expires_at)::bigint \
                     FROM {} WHERE handle=$1 AND owner=$2 AND (expires_at IS NULL OR expires_at > clock_timestamp())",
                    self.table()
                ),
                &[handle, &owner],
            )
            .await?;
        Ok(row.map(|row| ManagedFile {
            handle: row.get(0),
            bucket: row.get(1),
            name: row.get(2),
            media_type: row.get(3),
            bytes: row.get(4),
            expires_at: row.get(5),
        }))
    }

    /// Removes only a file owned by the current principal.
    pub async fn delete<C: GenericClient>(&self, client: &C, owner: &str, handle: &Uuid) -> Result<bool, Error> {
        if !valid_owner(owner) {
            return Ok(false);
        }
        let n = client.execute(&format!("DELETE FROM {} WHERE handle=$1 AND owner=$2", self.table()), &[handle, &owner]).await?;
        Ok(n == 1)
    }
}

fn valid_schema(value: &str) -> bool {
    value.len() <= 63
        && value.starts_with("aip_")
        && value.as_bytes().get(4).is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
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
