//! PostgreSQL ledger of presigned S3 transfers.
//!
//! S3 object bytes never pass through the server, so the server cannot count them as they arrive. The ledger counts
//! them the other way around: it reserves the declared size when a presigned URL or multipart part is issued, charges
//! the reservation to the owner's quota, and later reconciles each reservation with what the store actually holds.
//! A reservation that is never used expires, is checked against the store, and is released or aborted. The owner
//! quota therefore bounds bytes that were signed for, not only bytes that were confirmed.

use tokio_postgres::{Client, Transaction};

const LEDGER: &str = "aip_object_ledger";
const PARTS: &str = "aip_object_parts";
/// Advisory lock namespace for one owner's ledger rows. Distinct from the deployment fence (1095323725),
/// the job collector (1095323726) and the message quota (1095323727).
const OWNER_LOCK: i32 = 1_095_323_728;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSchema,
    InvalidRequest,
    QuotaExceeded,
    /// The multipart upload or object is not in the ledger in the state the request needs.
    NotFound,
    Database(String),
}

impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Database(format!("{error:?}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Put,
    Multipart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Signed for, not yet seen in the store.
    Pending,
    /// Confirmed in the store.
    Complete,
    /// A signed DELETE was issued; the store decides whether it was used.
    Deleting,
}

impl State {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "PENDING" => Some(Self::Pending),
            "COMPLETE" => Some(Self::Complete),
            "DELETING" => Some(Self::Deleting),
            _ => None,
        }
    }
}

/// A reservation whose time ran out. `version` guards `resolve` against a request that touched the row meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub owner: String,
    pub object_id: String,
    pub kind: Kind,
    pub upload_id: Option<String>,
    pub state: State,
    pub bytes: i64,
    version: i64,
}

/// What the store says about a due object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The object exists with this many bytes.
    Stored(u64),
    /// The object does not exist (and a multipart upload, if any, is aborted).
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub objects: u64,
}

#[derive(Clone, Debug)]
pub struct ObjectLedger {
    schema: String,
    max_bytes: u64,
    max_objects: usize,
}

impl ObjectLedger {
    /// `schema` is the infrastructure schema, which keeps these tables out of the application's migration fingerprint.
    pub fn new(schema: impl Into<String>, max_bytes: u64, max_objects: usize) -> Result<Self, Error> {
        let schema = schema.into();
        if !valid_schema(&schema) || max_bytes == 0 || max_objects == 0 || i64::try_from(max_bytes).is_err() {
            return Err(Error::InvalidSchema);
        }
        Ok(Self { schema, max_bytes, max_objects })
    }

    fn ledger(&self) -> String {
        format!("{}.{LEDGER}", self.schema)
    }

    fn parts(&self) -> String {
        format!("{}.{PARTS}", self.schema)
    }

    pub fn ddl(schema: &str) -> Result<String, Error> {
        if !valid_schema(schema) {
            return Err(Error::InvalidSchema);
        }
        Ok(format!(
            "CREATE TABLE IF NOT EXISTS {schema}.{LEDGER} (owner text NOT NULL, object_id text NOT NULL, \
             kind text NOT NULL CHECK (kind IN ('PUT','MULTIPART')), upload_id text, \
             state text NOT NULL CHECK (state IN ('PENDING','COMPLETE','DELETING')), \
             bytes bigint NOT NULL CHECK (bytes >= 0), expires_at timestamptz, version bigint NOT NULL DEFAULT 0, \
             created_at timestamptz NOT NULL DEFAULT clock_timestamp(), PRIMARY KEY(owner,object_id)); \
             CREATE INDEX IF NOT EXISTS aip_object_ledger_due ON {schema}.{LEDGER} (expires_at) WHERE expires_at IS NOT NULL; \
             CREATE TABLE IF NOT EXISTS {schema}.{PARTS} (owner text NOT NULL, object_id text NOT NULL, part_number integer NOT NULL, \
             bytes bigint NOT NULL CHECK (bytes > 0), PRIMARY KEY(owner,object_id,part_number))"
        ))
    }

    /// Reserves `bytes` for a presigned PUT of `object_id`, replacing an earlier reservation or confirmed size of the
    /// same object. The reservation is reconciled once `ttl_secs` have passed.
    pub async fn reserve_put(&self, client: &mut Client, owner: &str, object_id: &str, bytes: u64, ttl_secs: u64) -> Result<(), Error> {
        let bytes = checked_bytes(bytes)?;
        let ttl = checked_secs(ttl_secs)?;
        let tx = client.transaction().await?;
        let result = self.reserve_put_in(&tx, owner, object_id, bytes, ttl).await;
        finish(tx, result).await
    }

    async fn reserve_put_in(&self, tx: &Transaction<'_>, owner: &str, object_id: &str, bytes: i64, ttl: f64) -> Result<(), Error> {
        self.lock(tx, owner).await?;
        let existing = tx
            .query_opt(&format!("SELECT kind FROM {} WHERE owner=$1 AND object_id=$2", self.ledger()), &[&owner, &object_id])
            .await?
            .map(|row| row.get::<_, String>(0));
        if existing.as_deref() == Some("MULTIPART") {
            return Err(Error::InvalidRequest);
        }
        self.check_quota(tx, owner, object_id, bytes, existing.is_none()).await?;
        tx.execute(
            &format!(
                "INSERT INTO {} (owner,object_id,kind,state,bytes,expires_at) VALUES ($1,$2,'PUT','PENDING',$3,clock_timestamp()+make_interval(secs=>$4)) \
                 ON CONFLICT (owner,object_id) DO UPDATE SET state='PENDING',bytes=$3,expires_at=clock_timestamp()+make_interval(secs=>$4),version={0}.version+1",
                self.ledger()
            ),
            &[&owner, &object_id, &bytes, &ttl],
        )
        .await?;
        Ok(())
    }

    /// Records a multipart upload the store just created. No bytes are reserved until parts are signed.
    pub async fn start_multipart(&self, client: &mut Client, owner: &str, object_id: &str, upload_id: &str, idle_secs: u64) -> Result<(), Error> {
        let idle = checked_secs(idle_secs)?;
        let tx = client.transaction().await?;
        let result = self.start_multipart_in(&tx, owner, object_id, upload_id, idle).await;
        finish(tx, result).await
    }

    async fn start_multipart_in(&self, tx: &Transaction<'_>, owner: &str, object_id: &str, upload_id: &str, idle: f64) -> Result<(), Error> {
        self.lock(tx, owner).await?;
        self.check_quota(tx, owner, object_id, 0, true).await?;
        tx.execute(
            &format!(
                "INSERT INTO {} (owner,object_id,kind,upload_id,state,bytes,expires_at) VALUES ($1,$2,'MULTIPART',$3,'PENDING',0,clock_timestamp()+make_interval(secs=>$4))",
                self.ledger()
            ),
            &[&owner, &object_id, &upload_id, &idle],
        )
        .await?;
        Ok(())
    }

    /// Charges a signed part to the upload. Signing the same part number again replaces its size, so a client that
    /// re-signs after a failed attempt is not charged twice. Every call restarts the idle timer.
    #[allow(clippy::too_many_arguments)]
    pub async fn reserve_part(
        &self,
        client: &mut Client,
        owner: &str,
        object_id: &str,
        upload_id: &str,
        part_number: u16,
        bytes: u64,
        idle_secs: u64,
    ) -> Result<(), Error> {
        let bytes = checked_bytes(bytes)?;
        let idle = checked_secs(idle_secs)?;
        let part = i32::from(part_number);
        let tx = client.transaction().await?;
        let result = self.reserve_part_in(&tx, owner, object_id, upload_id, part, bytes, idle).await;
        finish(tx, result).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn reserve_part_in(
        &self,
        tx: &Transaction<'_>,
        owner: &str,
        object_id: &str,
        upload_id: &str,
        part: i32,
        bytes: i64,
        idle: f64,
    ) -> Result<(), Error> {
        self.lock(tx, owner).await?;
        self.pending_multipart(tx, owner, object_id, upload_id).await?;
        let own_parts: i64 = tx
            .query_one(
                &format!("SELECT COALESCE(sum(bytes),0)::bigint FROM {} WHERE owner=$1 AND object_id=$2 AND part_number<>$3", self.parts()),
                &[&owner, &object_id, &part],
            )
            .await?
            .get(0);
        // The upload's own row is excluded from the owner total and added back as its parts after the replacement.
        self.check_quota(tx, owner, object_id, own_parts.checked_add(bytes).ok_or(Error::QuotaExceeded)?, false).await?;
        tx.execute(
            &format!("INSERT INTO {} (owner,object_id,part_number,bytes) VALUES ($1,$2,$3,$4) ON CONFLICT (owner,object_id,part_number) DO UPDATE SET bytes=$4", self.parts()),
            &[&owner, &object_id, &part, &bytes],
        )
        .await?;
        tx.execute(
            &format!(
                "UPDATE {} SET bytes=$3,expires_at=clock_timestamp()+make_interval(secs=>$4),version=version+1 WHERE owner=$1 AND object_id=$2",
                self.ledger()
            ),
            &[&owner, &object_id, &(own_parts + bytes), &idle],
        )
        .await?;
        Ok(())
    }

    /// Whether `upload_id` is a multipart upload this owner started and has not finished. Checked before the store is
    /// asked to complete it, so an upload the ledger never charged cannot become an uncounted object.
    pub async fn require_open_multipart(&self, client: &mut Client, owner: &str, object_id: &str, upload_id: &str) -> Result<(), Error> {
        let tx = client.transaction().await?;
        let result = self.pending_multipart(&tx, owner, object_id, upload_id).await;
        finish(tx, result).await
    }

    /// The store completed the upload: its parts become a stored object of the summed size.
    pub async fn complete_multipart(&self, client: &mut Client, owner: &str, object_id: &str, upload_id: &str) -> Result<(), Error> {
        let tx = client.transaction().await?;
        let result = self.complete_multipart_in(&tx, owner, object_id, upload_id).await;
        finish(tx, result).await
    }

    async fn complete_multipart_in(&self, tx: &Transaction<'_>, owner: &str, object_id: &str, upload_id: &str) -> Result<(), Error> {
        self.lock(tx, owner).await?;
        self.pending_multipart(tx, owner, object_id, upload_id).await?;
        tx.execute(
            &format!("UPDATE {} SET state='COMPLETE',expires_at=NULL,version=version+1 WHERE owner=$1 AND object_id=$2", self.ledger()),
            &[&owner, &object_id],
        )
        .await?;
        tx.execute(&format!("DELETE FROM {} WHERE owner=$1 AND object_id=$2", self.parts()), &[&owner, &object_id]).await?;
        Ok(())
    }

    /// The store aborted (or no longer knows) a multipart upload this owner started: drop its row and charged parts.
    /// Returns false when the ledger has no such open upload. A plain object's row is never touched, so a caller
    /// cannot release the quota of a stored object by "aborting" it with an invented upload id.
    pub async fn abort_multipart(&self, client: &mut Client, owner: &str, object_id: &str, upload_id: &str) -> Result<bool, Error> {
        let tx = client.transaction().await?;
        let result = async {
            self.lock(&tx, owner).await?;
            match self.pending_multipart(&tx, owner, object_id, upload_id).await {
                Ok(()) => self.delete_rows(&tx, owner, object_id, None).await,
                Err(Error::NotFound) => Ok(false),
                Err(error) => Err(error),
            }
        }
        .await;
        finish(tx, result).await
    }

    /// A signed DELETE was issued. The row keeps counting against the quota until the store confirms the object is
    /// gone, because the server cannot see whether the caller used the URL. An object the ledger does not know
    /// (uploaded before the ledger existed) has nothing to release.
    pub async fn delete_signed(&self, client: &mut Client, owner: &str, object_id: &str, ttl_secs: u64) -> Result<(), Error> {
        let ttl = checked_secs(ttl_secs)?;
        client
            .execute(
                &format!(
                    "UPDATE {} SET state='DELETING',expires_at=clock_timestamp()+make_interval(secs=>$3),version=version+1 \
                     WHERE owner=$1 AND object_id=$2 AND state='COMPLETE'",
                    self.ledger()
                ),
                &[&owner, &object_id, &ttl],
            )
            .await?;
        Ok(())
    }

    /// Takes up to `limit` rows whose time ran out and leases them for `lease_secs`, so concurrent instances and
    /// retries after a failed store call do not work on the same rows at once.
    pub async fn claim_due(&self, client: &mut Client, limit: i64, lease_secs: u64) -> Result<Vec<Due>, Error> {
        let lease = checked_secs(lease_secs)?;
        let rows = client
            .query(
                &format!(
                    "UPDATE {0} l SET expires_at=clock_timestamp()+make_interval(secs=>$2) \
                     FROM (SELECT owner,object_id FROM {0} WHERE expires_at<=clock_timestamp() ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED) due \
                     WHERE l.owner=due.owner AND l.object_id=due.object_id \
                     RETURNING l.owner,l.object_id,l.kind,l.upload_id,l.state,l.bytes,l.version",
                    self.ledger()
                ),
                &[&limit, &lease],
            )
            .await?;
        let mut due = Vec::with_capacity(rows.len());
        for row in rows {
            let kind = match row.get::<_, String>(2).as_str() {
                "PUT" => Kind::Put,
                _ => Kind::Multipart,
            };
            let state = State::parse(&row.get::<_, String>(4)).ok_or(Error::InvalidRequest)?;
            due.push(Due { owner: row.get(0), object_id: row.get(1), kind, upload_id: row.get(3), state, bytes: row.get(5), version: row.get(6) });
        }
        Ok(due)
    }

    /// Applies what the store said about a due row. Returns false when the row changed after it was claimed; the
    /// newer request owns it and its own timer will bring it back.
    pub async fn resolve(&self, client: &mut Client, due: &Due, outcome: Outcome) -> Result<bool, Error> {
        let size = match outcome {
            Outcome::Stored(size) => Some(checked_bytes(size)?),
            Outcome::Absent => None,
        };
        let tx = client.transaction().await?;
        let result = self.resolve_in(&tx, due, size).await;
        finish(tx, result).await
    }

    async fn resolve_in(&self, tx: &Transaction<'_>, due: &Due, size: Option<i64>) -> Result<bool, Error> {
        self.lock(tx, &due.owner).await?;
        match size {
            None => self.delete_rows(tx, &due.owner, &due.object_id, Some(due.version)).await,
            Some(size) => {
                let updated = tx
                    .execute(
                        &format!(
                            "UPDATE {} SET state='COMPLETE',bytes=$4,expires_at=NULL,version=version+1 WHERE owner=$1 AND object_id=$2 AND version=$3",
                            self.ledger()
                        ),
                        &[&due.owner, &due.object_id, &due.version, &size],
                    )
                    .await?;
                if updated > 0 {
                    tx.execute(&format!("DELETE FROM {} WHERE owner=$1 AND object_id=$2", self.parts()), &[&due.owner, &due.object_id]).await?;
                }
                Ok(updated > 0)
            }
        }
    }

    pub async fn usage(&self, client: &Client, owner: &str) -> Result<Usage, Error> {
        let row =
            client.query_one(&format!("SELECT COALESCE(sum(bytes),0)::bigint,count(*) FROM {} WHERE owner=$1", self.ledger()), &[&owner]).await?;
        Ok(Usage { bytes: row.get::<_, i64>(0) as u64, objects: row.get::<_, i64>(1) as u64 })
    }

    async fn lock(&self, tx: &Transaction<'_>, owner: &str) -> Result<(), Error> {
        tx.query_one("SELECT pg_advisory_xact_lock($1,hashtext($2))", &[&OWNER_LOCK, &format!("{}:{owner}", self.schema)]).await?;
        Ok(())
    }

    /// Rejects the request when `extra` bytes (replacing `except`'s current charge) or one more object would pass
    /// the owner's ceiling. Counts every ledger state: signed-for bytes are charged until they are released.
    async fn check_quota(&self, tx: &Transaction<'_>, owner: &str, except: &str, extra: i64, new_object: bool) -> Result<(), Error> {
        let row = tx
            .query_one(
                &format!("SELECT COALESCE(sum(bytes),0)::bigint,count(*) FROM {} WHERE owner=$1 AND object_id<>$2", self.ledger()),
                &[&owner, &except],
            )
            .await?;
        let others: i64 = row.get(0);
        let objects: i64 = row.get(1);
        let max_bytes = self.max_bytes as i64;
        let total = others.checked_add(extra).ok_or(Error::QuotaExceeded)?;
        let count = objects + i64::from(new_object);
        if total > max_bytes || (new_object && count as u64 > self.max_objects as u64) {
            return Err(Error::QuotaExceeded);
        }
        Ok(())
    }

    async fn pending_multipart(&self, tx: &Transaction<'_>, owner: &str, object_id: &str, upload_id: &str) -> Result<(), Error> {
        let found = tx
            .query_opt(
                &format!(
                    "SELECT 1 FROM {} WHERE owner=$1 AND object_id=$2 AND kind='MULTIPART' AND state='PENDING' AND upload_id=$3 FOR UPDATE",
                    self.ledger()
                ),
                &[&owner, &object_id, &upload_id],
            )
            .await?;
        found.map(|_| ()).ok_or(Error::NotFound)
    }

    async fn delete_rows(&self, tx: &Transaction<'_>, owner: &str, object_id: &str, version: Option<i64>) -> Result<bool, Error> {
        let removed = match version {
            Some(version) => {
                tx.execute(&format!("DELETE FROM {} WHERE owner=$1 AND object_id=$2 AND version=$3", self.ledger()), &[&owner, &object_id, &version])
                    .await?
            }
            None => tx.execute(&format!("DELETE FROM {} WHERE owner=$1 AND object_id=$2", self.ledger()), &[&owner, &object_id]).await?,
        };
        if removed > 0 {
            tx.execute(&format!("DELETE FROM {} WHERE owner=$1 AND object_id=$2", self.parts()), &[&owner, &object_id]).await?;
        }
        Ok(removed > 0)
    }
}

/// Commits on success and rolls back explicitly on failure, so a rejected request never leaves its locks to the
/// connection's drop handling.
async fn finish<T>(tx: Transaction<'_>, result: Result<T, Error>) -> Result<T, Error> {
    match result {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

fn checked_bytes(bytes: u64) -> Result<i64, Error> {
    i64::try_from(bytes).map_err(|_| Error::QuotaExceeded)
}

fn checked_secs(secs: u64) -> Result<f64, Error> {
    if secs == 0 || secs > 7 * 24 * 3600 {
        return Err(Error::InvalidRequest);
    }
    Ok(secs as f64)
}

fn valid_schema(value: &str) -> bool {
    value.len() <= 63
        && value.starts_with("aip_")
        && value.as_bytes().get(4).is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
