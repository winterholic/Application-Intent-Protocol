//! PostgreSQL ledger of presigned S3 transfers.
//!
//! S3 object bytes never pass through the server, so the server cannot count them as they arrive. The ledger counts
//! them the other way around: it reserves the declared size when a presigned URL or multipart part is issued, charges
//! the reservation to the owner's quota, and later reconciles each reservation with what the store actually holds.
//! A reservation that is never used expires, is checked against the store, and is released or aborted. The owner
//! quota therefore bounds bytes that were signed for, not only bytes that were confirmed.

use crate::objects::{ListPage, owner_prefix};
use std::collections::HashMap;
use tokio_postgres::{Client, Transaction};

const LEDGER: &str = "aip_object_ledger";
const PARTS: &str = "aip_object_parts";
const OWNERS: &str = "aip_object_owners";
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
    /// A PUT whose first check found nothing and whose charge is being held for a second look (see `Outcome::NotYetStored`).
    pub settling: bool,
    version: i64,
}

/// What the store says about a due object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The object exists with this many bytes.
    Stored(u64),
    /// The object does not exist and nothing can still create it (a multipart upload, if any, is aborted).
    Absent,
    /// A PUT whose object does not exist yet. One missing `HEAD` does not prove the transfer is not still running: the signed
    /// URL only has to be valid when the transfer starts. The first time, the charge is kept and the row is looked at again
    /// after `hold_secs`; on the second miss the reservation is released. An upload that is still running after both looks
    /// is not counted (a documented limit, see the release gate).
    NotYetStored { hold_secs: u64 },
}

/// An owner whose bucket prefix is due for a `ListObjectsV2` scan, with the place the previous scan stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanTarget {
    pub owner: String,
    pub owner_prefix: String,
    pub continuation_token: Option<String>,
}

/// What one applied listing page changed. `skipped_*` rows were left exactly as they were.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanPageReport {
    /// Rows inserted, or whose size was raised or corrected.
    pub adopted: usize,
    /// Rows that already said what the listing says; not written, so their `version` did not move.
    pub unchanged: usize,
    /// Keys of an active multipart upload; the upload's own parts decide its size, not a listing.
    pub skipped_multipart: usize,
    /// Rows that changed (or disappeared) after the version snapshot the listing was fenced with.
    pub skipped_fenced: usize,
}

/// A row as it stood when the listing was requested: `(state, bytes, version)`.
pub type RowVersions = HashMap<String, (State, i64, i64)>;

/// Bounds of one scan tick. `max_pages` caps the billed list requests across all owners in the tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanPolicy {
    pub page_keys: u16,
    pub max_pages: u16,
    pub lease_secs: u64,
    /// Seconds before an owner whose listing finished is scanned again.
    pub rescan_secs: u64,
    /// Seconds before an owner whose listing was cut short by the page budget continues.
    pub resume_secs: u64,
}

/// Totals of one scan tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanTickReport {
    pub pages: usize,
    pub owners_completed: usize,
    /// Owners whose list call or page application failed; their lease keeps them quiet until it ends.
    pub owners_failed: usize,
    pub skipped_keys: usize,
    pub rows: ScanPageReport,
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

    fn owners(&self) -> String {
        format!("{}.{OWNERS}", self.schema)
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
             created_at timestamptz NOT NULL DEFAULT clock_timestamp(), settling boolean NOT NULL DEFAULT false, PRIMARY KEY(owner,object_id)); \
             ALTER TABLE {schema}.{LEDGER} ADD COLUMN IF NOT EXISTS settling boolean NOT NULL DEFAULT false; \
             CREATE INDEX IF NOT EXISTS aip_object_ledger_due ON {schema}.{LEDGER} (expires_at) WHERE expires_at IS NOT NULL; \
             CREATE TABLE IF NOT EXISTS {schema}.{PARTS} (owner text NOT NULL, object_id text NOT NULL, part_number integer NOT NULL, \
             bytes bigint NOT NULL CHECK (bytes > 0), PRIMARY KEY(owner,object_id,part_number)); \
             CREATE TABLE IF NOT EXISTS {schema}.{OWNERS} (owner text PRIMARY KEY, owner_prefix text NOT NULL UNIQUE CHECK (length(owner_prefix) = 64), \
             continuation_token text, next_scan_at timestamptz NOT NULL DEFAULT clock_timestamp(), lease_until timestamptz, \
             last_scanned_at timestamptz, registered_at timestamptz NOT NULL DEFAULT clock_timestamp()); \
             CREATE INDEX IF NOT EXISTS aip_object_owners_scan_due ON {schema}.{OWNERS} (next_scan_at); \
             INSERT INTO {schema}.{OWNERS} (owner, owner_prefix) SELECT DISTINCT owner, encode(sha256(convert_to(owner,'UTF8')),'hex') FROM {schema}.{LEDGER} \
             ON CONFLICT DO NOTHING"
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
        self.register_in(tx, owner).await?;
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
                 ON CONFLICT (owner,object_id) DO UPDATE SET state='PENDING',settling=false,bytes=$3,expires_at=clock_timestamp()+make_interval(secs=>$4),version={0}.version+1",
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
        self.register_in(tx, owner).await?;
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
                     RETURNING l.owner,l.object_id,l.kind,l.upload_id,l.state,l.bytes,l.version,l.settling",
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
            due.push(Due {
                owner: row.get(0),
                object_id: row.get(1),
                kind,
                upload_id: row.get(3),
                state,
                bytes: row.get(5),
                version: row.get(6),
                settling: row.get(7),
            });
        }
        Ok(due)
    }

    /// Applies what the store said about a due row. Returns false when the row changed after it was claimed; the
    /// newer request owns it and its own timer will bring it back.
    pub async fn resolve(&self, client: &mut Client, due: &Due, outcome: Outcome) -> Result<bool, Error> {
        let (size, hold) = match outcome {
            Outcome::Stored(size) => (Some(checked_bytes(size)?), None),
            Outcome::Absent => (None, None),
            Outcome::NotYetStored { hold_secs } if due.kind == Kind::Put && due.state == State::Pending && !due.settling => {
                (None, Some(checked_secs(hold_secs)?))
            }
            Outcome::NotYetStored { .. } => (None, None),
        };
        let tx = client.transaction().await?;
        let result = match hold {
            Some(hold) => self.hold_in(&tx, due, hold).await,
            None => self.resolve_in(&tx, due, size).await,
        };
        finish(tx, result).await
    }

    /// Keeps the charge and moves the timer, guarded by `version` like every other resolution.
    async fn hold_in(&self, tx: &Transaction<'_>, due: &Due, hold: f64) -> Result<bool, Error> {
        let updated = tx
            .execute(
                &format!(
                    "UPDATE {} SET settling=true,expires_at=clock_timestamp()+make_interval(secs=>$4),version=version+1 \
                     WHERE owner=$1 AND object_id=$2 AND version=$3 AND kind='PUT' AND state='PENDING' AND NOT settling",
                    self.ledger()
                ),
                &[&due.owner, &due.object_id, &due.version, &hold],
            )
            .await?;
        Ok(updated > 0)
    }

    async fn resolve_in(&self, tx: &Transaction<'_>, due: &Due, size: Option<i64>) -> Result<bool, Error> {
        self.lock(tx, &due.owner).await?;
        match size {
            None => self.delete_rows(tx, &due.owner, &due.object_id, Some(due.version)).await,
            Some(size) => {
                let updated = tx
                    .execute(
                        &format!(
                            "UPDATE {} SET state='COMPLETE',settling=false,bytes=$4,expires_at=NULL,version=version+1 WHERE owner=$1 AND object_id=$2 AND version=$3",
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

    /// Records a size observed by the trusted provider HEAD, including objects that predate the ledger.
    /// Existing bytes must be recorded even above quota: rejecting an observation would undercount reality.
    /// This does not prove no older signed PUT/DELETE is in flight, so their timers and conservative charge
    /// survive. Active multipart reservations cannot be finalized through a HEAD of an older object.
    pub async fn adopt_object(&self, client: &mut Client, owner: &str, object_id: &str, bytes: u64) -> Result<(), Error> {
        let bytes = i64::try_from(bytes).map_err(|_| Error::InvalidRequest)?;
        let tx = client.transaction().await?;
        let result = self.adopt_object_in(&tx, owner, object_id, bytes).await;
        finish(tx, result).await
    }

    async fn adopt_object_in(&self, tx: &Transaction<'_>, owner: &str, object_id: &str, bytes: i64) -> Result<(), Error> {
        self.lock(tx, owner).await?;
        self.register_in(tx, owner).await?;
        let active = tx
            .query_opt(
                &format!("SELECT 1 FROM {} WHERE owner=$1 AND object_id=$2 AND kind='MULTIPART' AND state='PENDING' FOR UPDATE", self.ledger()),
                &[&owner, &object_id],
            )
            .await?;
        if active.is_some() {
            return Err(Error::InvalidRequest);
        }
        tx.execute(
            &format!(
                "INSERT INTO {} (owner,object_id,kind,state,bytes) VALUES ($1,$2,'PUT','COMPLETE',$3) \
                 ON CONFLICT (owner,object_id) DO UPDATE SET bytes=CASE WHEN {0}.state IN ('PENDING','DELETING') \
                 THEN GREATEST({0}.bytes,$3) ELSE $3 END,version={0}.version+1",
                self.ledger()
            ),
            &[&owner, &object_id, &bytes],
        )
        .await?;
        Ok(())
    }

    /// Records `owner` and the bucket prefix its objects live under, so a scan can name the owner of a prefix. The
    /// prefix is a one-way hash, so this table is the only way back; every ledger write registers its owner, which
    /// keeps the row after the owner's last ledger row is released. Returns the prefix.
    pub async fn register_owner(&self, client: &mut Client, owner: &str) -> Result<String, Error> {
        let tx = client.transaction().await?;
        let result = async {
            self.lock(&tx, owner).await?;
            self.register_in(&tx, owner).await
        }
        .await;
        finish(tx, result).await
    }

    async fn register_in(&self, tx: &Transaction<'_>, owner: &str) -> Result<String, Error> {
        let prefix = owner_prefix(owner).map_err(|_| Error::InvalidRequest)?;
        tx.execute(&format!("INSERT INTO {} (owner,owner_prefix) VALUES ($1,$2) ON CONFLICT (owner) DO NOTHING", self.owners()), &[&owner, &prefix])
            .await?;
        Ok(prefix)
    }

    /// The owner registered for a bucket prefix, if any.
    pub async fn owner_for_prefix(&self, client: &Client, owner_prefix: &str) -> Result<Option<String>, Error> {
        let row = client.query_opt(&format!("SELECT owner FROM {} WHERE owner_prefix=$1", self.owners()), &[&owner_prefix]).await?;
        Ok(row.map(|row| row.get(0)))
    }

    /// Takes up to `limit` registered owners whose scan is due and leases them for `lease_secs`, so concurrent instances
    /// do not list the same prefix, and so a failing owner is retried after the lease rather than every tick.
    pub async fn claim_scan_targets(&self, client: &mut Client, limit: i64, lease_secs: u64) -> Result<Vec<ScanTarget>, Error> {
        let lease = checked_secs(lease_secs)?;
        if limit <= 0 {
            return Ok(Vec::new());
        }
        let rows = client
            .query(
                &format!(
                    "UPDATE {0} o SET lease_until=clock_timestamp()+make_interval(secs=>$2) \
                     FROM (SELECT owner FROM {0} WHERE next_scan_at<=clock_timestamp() AND (lease_until IS NULL OR lease_until<=clock_timestamp()) \
                     ORDER BY next_scan_at LIMIT $1 FOR UPDATE SKIP LOCKED) due \
                     WHERE o.owner=due.owner RETURNING o.owner,o.owner_prefix,o.continuation_token",
                    self.owners()
                ),
                &[&limit, &lease],
            )
            .await?;
        Ok(rows.into_iter().map(|row| ScanTarget { owner: row.get(0), owner_prefix: row.get(1), continuation_token: row.get(2) }).collect())
    }

    /// The owner's rows as they are now. Taken before a listing is requested and handed back to `apply_scan_page`, which
    /// uses it to tell a row the listing could not know about from one it can describe.
    pub async fn snapshot_owner_versions(&self, client: &Client, owner: &str) -> Result<RowVersions, Error> {
        let rows = client.query(&format!("SELECT object_id,state,bytes,version FROM {} WHERE owner=$1", self.ledger()), &[&owner]).await?;
        let mut versions = HashMap::with_capacity(rows.len());
        for row in rows {
            let state = State::parse(&row.get::<_, String>(1)).ok_or(Error::InvalidRequest)?;
            versions.insert(row.get(0), (state, row.get(2), row.get(3)));
        }
        Ok(versions)
    }

    /// One bounded scan tick: claims due owners, lists at most `policy.max_pages` pages across all of them (a page is a
    /// billed request), applies each page and persists where each owner's listing stopped. Pages are taken round-robin so
    /// every claimed owner advances before any owner gets a second page. `list` receives the owner, the token to continue
    /// from and the page size. A failed list call or page leaves that owner leased, so a broken prefix is retried when
    /// the lease ends instead of on every tick.
    pub async fn scan_tick<F, Fut>(&self, client: &mut Client, policy: &ScanPolicy, mut list: F) -> Result<ScanTickReport, Error>
    where
        F: FnMut(ScanTarget, Option<String>, u16) -> Fut,
        Fut: std::future::Future<Output = Result<ListPage, crate::objects::Error>>,
    {
        struct Cursor {
            target: ScanTarget,
            token: Option<String>,
            before: RowVersions,
            done: bool,
        }
        let mut report = ScanTickReport::default();
        let targets = self.claim_scan_targets(client, i64::from(policy.max_pages), policy.lease_secs).await?;
        let mut cursors = Vec::with_capacity(targets.len());
        for target in targets {
            // One snapshot per owner and tick, taken before its first list call: every row changed since then is fenced.
            let before = self.snapshot_owner_versions(client, &target.owner).await?;
            cursors.push(Cursor { token: target.continuation_token.clone(), target, before, done: false });
        }
        let mut budget = usize::from(policy.max_pages);
        while budget > 0 && cursors.iter().any(|cursor| !cursor.done) {
            for cursor in cursors.iter_mut().filter(|cursor| !cursor.done) {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                report.pages += 1;
                let Ok(page) = list(cursor.target.clone(), cursor.token.clone(), policy.page_keys).await else {
                    cursor.done = true;
                    report.owners_failed += 1;
                    continue;
                };
                let items: Vec<(String, u64)> = page.items.iter().map(|(key, size)| (key.as_str(), *size)).collect();
                let applied = self
                    .apply_scan_page(
                        client,
                        &cursor.target.owner,
                        &cursor.before,
                        &items,
                        page.next_token.as_deref(),
                        policy.rescan_secs,
                        policy.resume_secs,
                    )
                    .await;
                let Ok(applied) = applied else {
                    cursor.done = true;
                    report.owners_failed += 1;
                    continue;
                };
                report.skipped_keys += page.skipped_keys;
                report.rows.adopted += applied.adopted;
                report.rows.unchanged += applied.unchanged;
                report.rows.skipped_multipart += applied.skipped_multipart;
                report.rows.skipped_fenced += applied.skipped_fenced;
                cursor.token = page.next_token;
                if cursor.token.is_none() {
                    cursor.done = true;
                    report.owners_completed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Applies one listing page and the cursor that continues after it, in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_scan_page(
        &self,
        client: &mut Client,
        owner: &str,
        pre_versions: &RowVersions,
        items: &[(String, u64)],
        next_token: Option<&str>,
        rescan_interval_secs: u64,
        page_delay_secs: u64,
    ) -> Result<ScanPageReport, Error> {
        let rescan = checked_secs(rescan_interval_secs)?;
        let delay = checked_secs(page_delay_secs)?;
        let tx = client.transaction().await?;
        let result = self.apply_scan_page_in(&tx, owner, pre_versions, items, next_token, rescan, delay).await;
        finish(tx, result).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_scan_page_in(
        &self,
        tx: &Transaction<'_>,
        owner: &str,
        pre_versions: &RowVersions,
        items: &[(String, u64)],
        next_token: Option<&str>,
        rescan: f64,
        delay: f64,
    ) -> Result<ScanPageReport, Error> {
        self.lock(tx, owner).await?;
        self.register_in(tx, owner).await?;
        let mut report = ScanPageReport::default();
        for (object_id, listed) in items {
            let listed = i64::try_from(*listed).map_err(|_| Error::InvalidRequest)?;
            let row = tx
                .query_opt(
                    &format!("SELECT kind,state,bytes,version FROM {} WHERE owner=$1 AND object_id=$2 FOR UPDATE", self.ledger()),
                    &[&owner, object_id],
                )
                .await?;
            let Some(row) = row else {
                // A row the snapshot knew and the ledger no longer has was released after the listing was requested (a
                // reconciled DELETE, an aborted upload). The listing predates that, so it must not bring the row back.
                if pre_versions.contains_key(object_id) {
                    report.skipped_fenced += 1;
                } else {
                    tx.execute(
                        &format!("INSERT INTO {} (owner,object_id,kind,state,bytes) VALUES ($1,$2,'PUT','COMPLETE',$3)", self.ledger()),
                        &[&owner, object_id, &listed],
                    )
                    .await?;
                    report.adopted += 1;
                }
                continue;
            };
            let (kind, state, bytes, version): (String, String, i64, i64) = (row.get(0), row.get(1), row.get(2), row.get(3));
            let state = State::parse(&state).ok_or(Error::InvalidRequest)?;
            if kind == "MULTIPART" && state == State::Pending {
                // Its parts decide its size; completion or abort resolves it. One such key must not stop the page.
                report.skipped_multipart += 1;
            } else if pre_versions.get(object_id).map(|(_, _, seen)| *seen) != Some(version) {
                // Created or changed after the snapshot: a newer writer owns it and the listing cannot describe it.
                report.skipped_fenced += 1;
            } else if bytes == listed || (state != State::Complete && bytes >= listed) {
                // Nothing to learn. Writing anyway would move `version` and fail a claimed reconcile of this very row.
                report.unchanged += 1;
            } else {
                let size = if state == State::Complete { listed } else { bytes.max(listed) };
                tx.execute(
                    &format!("UPDATE {} SET bytes=$3,version=version+1 WHERE owner=$1 AND object_id=$2", self.ledger()),
                    &[&owner, object_id, &size],
                )
                .await?;
                report.adopted += 1;
            }
        }
        match next_token {
            Some(token) => {
                tx.execute(
                    &format!(
                        "UPDATE {} SET continuation_token=$2,next_scan_at=clock_timestamp()+make_interval(secs=>$3),lease_until=NULL WHERE owner=$1",
                        self.owners()
                    ),
                    &[&owner, &token, &delay],
                )
                .await?;
            }
            None => {
                tx.execute(
                    &format!(
                        "UPDATE {} SET continuation_token=NULL,next_scan_at=clock_timestamp()+make_interval(secs=>$2),last_scanned_at=clock_timestamp(),lease_until=NULL WHERE owner=$1",
                        self.owners()
                    ),
                    &[&owner, &rescan],
                )
                .await?;
            }
        }
        Ok(report)
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
