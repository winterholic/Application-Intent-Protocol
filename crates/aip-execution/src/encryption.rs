//! Envelope keys and a transactional cell boundary for the PostgreSQL product path.
//!
//! DDL is an explicit provisioning operation. First creation and KEK-only rewrap are serialized by
//! the scope row in the caller's transaction. Cell usage is reserved on a separate connection and
//! committed before sealing, so resource rollback cannot replenish the random-nonce budget.
//! Resource policy, journal visitors and durable rekey must still be connected before activation.
use aip_crypto::cell::{self, Context as CellContext, DataKey};
use aip_crypto::provider::{KekProvider, WrapContext, WrappedDek};
use tokio_postgres::{Client, GenericClient, Row, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    NotFound,
    Corrupted,
    Crypto,
    Database,
    UsageExhausted,
    StaleWriter,
}

/// Operational per-DEK ceiling on random-nonce cell seals (including failed/rolled-back attempts).
/// This is a limit on calls, not a claim of nonce uniqueness or a cryptographic security proof.
pub const MAX_DEK_SEALS: u64 = 1 << 20;
const MAX_RESERVATION: u32 = 500;

/// Validated AAD location. Callers must authorize the row/field and validate its scalar before use.
#[derive(Debug)]
pub struct CellLocation {
    resource: String,
    field: String,
    row_id: String,
}
impl CellLocation {
    pub fn new(resource: &str, field: &str, row_id: &str) -> Result<Self, Error> {
        if !valid_resource(resource) || !valid_resource(field) || !canonical_id(row_id) {
            return Err(Error::InvalidInput);
        }
        Ok(Self { resource: resource.into(), field: field.into(), row_id: row_id.into() })
    }
}

/// Non-cloneable budget committed independently of any resource transaction. Dropping or cancelling
/// it intentionally wastes unused reservations. It contains neither raw keys nor resource values.
#[derive(Debug)]
pub struct SealReservation {
    deployment: String,
    scope: Scope,
    epoch: u64,
    dek_id: Uuid,
    version: u32,
    remaining: u32,
}
impl From<tokio_postgres::Error> for Error {
    fn from(_: tokio_postgres::Error) -> Self {
        Self::Database
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    kind: String,
    id: String,
}
impl Scope {
    pub fn service() -> Self {
        Self { kind: "service".into(), id: "global".into() }
    }
    pub fn reference(resource: &str, id: &str) -> Result<Self, Error> {
        Self::bound("ref", resource, id)
    }
    pub fn owner(resource: &str, id: &str) -> Result<Self, Error> {
        Self::bound("owner", resource, id)
    }
    fn bound(kind: &str, resource: &str, id: &str) -> Result<Self, Error> {
        if !valid_resource(resource) || !canonical_id(id) {
            return Err(Error::InvalidInput);
        }
        Ok(Self { kind: format!("{kind}:{resource}"), id: id.into() })
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Parses a writer/resolver-produced scope pair back into a validated [`Scope`].
/// Shared by the seal and open boundaries so neither invents its own spelling.
pub(crate) fn resolve_scope(kind: &str, id: &str) -> Result<Scope, Error> {
    if kind == "service" {
        if id == "global" {
            return Ok(Scope::service());
        }
        return Err(Error::InvalidInput);
    }
    let (kind, resource) = kind.split_once(':').ok_or(Error::InvalidInput)?;
    match kind {
        "ref" => Scope::reference(resource, id),
        "owner" => Scope::owner(resource, id),
        _ => Err(Error::InvalidInput),
    }
}

/// Wrapped material only. Debug and errors intentionally omit the provider's payload.
#[derive(Clone)]
pub struct Dek {
    id: Uuid,
    version: u32,
    wrapped: WrappedDek,
    id_text: String,
}
impl std::fmt::Debug for Dek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dek").field("id", &self.id).field("version", &self.version).finish_non_exhaustive()
    }
}
impl Dek {
    pub fn id(&self) -> &str {
        &self.id_text
    }
    pub fn uuid(&self) -> Uuid {
        self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn wrapped(&self) -> &WrappedDek {
        &self.wrapped
    }
    fn from_row(row: Row) -> Result<Self, Error> {
        let id: Uuid = row.try_get("dek_id")?;
        let version: i32 = row.try_get("version")?;
        let wrapped = WrappedDek { kek_id: row.try_get("kek_id")?, sealed: row.try_get("wrapped_dek")? };
        if version <= 0 || !valid_wrapped(&wrapped) {
            return Err(Error::Corrupted);
        }
        Ok(Self { id, id_text: id.to_string(), version: version as u32, wrapped })
    }
}

/// Current database identity for a resource writer. Scope and DEK shared locks
/// belong to the caller's transaction, not to this value; use this DEK while that
/// transaction is live. This does not reserve nonce usage or authorize a row.
#[derive(Debug)]
pub struct WriteDek {
    epoch: u64,
    dek: Dek,
}
impl WriteDek {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn dek(&self) -> &Dek {
        &self.dek
    }
}

#[derive(Clone, Debug)]
pub struct DekStore {
    deployment: String,
    infra: String,
}
impl DekStore {
    pub fn new(deployment: &str) -> Result<Self, Error> {
        if deployment.len() < 5 || !deployment.as_bytes()[4].is_ascii_lowercase() {
            return Err(Error::InvalidInput);
        }
        let infra = crate::infrastructure_schema(deployment).map_err(|_| Error::InvalidInput)?;
        Ok(Self { deployment: deployment.into(), infra })
    }
    pub fn deployment_schema(&self) -> &str {
        &self.deployment
    }
    pub fn infrastructure_schema(&self) -> &str {
        &self.infra
    }
    /// Migration role only: callers must not run DDL on a runtime request.
    pub fn ddl(&self) -> String {
        let s = &self.infra;
        format!(
            "CREATE SCHEMA IF NOT EXISTS {s};
             CREATE TABLE IF NOT EXISTS {s}.aip_encryption_scopes (
               scope_kind text NOT NULL CHECK (octet_length(scope_kind) BETWEEN 1 AND 69),
               scope_id text NOT NULL CHECK (octet_length(scope_id) BETWEEN 1 AND 36),
               write_epoch bigint NOT NULL DEFAULT 1 CHECK (write_epoch > 0),
               PRIMARY KEY(scope_kind,scope_id),
               CHECK ((scope_kind='service' AND scope_id='global') OR
                 (scope_kind ~ '^(ref|owner):[A-Za-z][A-Za-z0-9_]{{0,62}}$' AND
                   (scope_id ~ '^(0|[1-9][0-9]{{0,18}})$' OR scope_id ~ '^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$'))));
             CREATE TABLE IF NOT EXISTS {s}.aip_encryption_deks (
               dek_id uuid PRIMARY KEY,
               scope_kind text NOT NULL, scope_id text NOT NULL,
               version integer NOT NULL CHECK (version > 0),
               state text NOT NULL CHECK (state IN ('ACTIVE','READABLE','RETIRED')),
               kek_id text NOT NULL CHECK (kek_id ~ '^[A-Za-z0-9._-]{{1,64}}$'),
               wrapped_dek bytea NOT NULL CHECK (octet_length(wrapped_dek)=60),
               created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
               rewrapped_at timestamptz,
               FOREIGN KEY(scope_kind,scope_id) REFERENCES {s}.aip_encryption_scopes(scope_kind,scope_id),
               UNIQUE(scope_kind,scope_id,version));
             CREATE UNIQUE INDEX IF NOT EXISTS aip_encryption_one_active ON {s}.aip_encryption_deks(scope_kind,scope_id) WHERE state='ACTIVE';"
        )
        + &format!(
            "ALTER TABLE {s}.aip_encryption_deks ADD COLUMN IF NOT EXISTS seal_count bigint NOT NULL DEFAULT 0 CHECK(seal_count BETWEEN 0 AND {MAX_DEK_SEALS});"
        )
        + &format!(
            "CREATE TABLE IF NOT EXISTS {s}.aip_rekey_leases (
               target_key text PRIMARY KEY,
               holder text NOT NULL,
               epoch bigint NOT NULL CHECK (epoch > 0),
               leased_until timestamptz NOT NULL);"
        )
    }
    async fn lock_scope(&self, tx: &Transaction<'_>, scope: &Scope) -> Result<(), Error> {
        tx.query_one(
            &format!("SELECT write_epoch FROM {}.aip_encryption_scopes WHERE scope_kind=$1 AND scope_id=$2 FOR UPDATE", self.infra),
            &[&scope.kind, &scope.id],
        )
        .await?;
        Ok(())
    }
    /// Returns the winning committed-or-caller-owned key, never a losing provisional key. The
    /// caller must commit before publishing its identity or ciphertext to another transaction.
    /// The caller retains the scope lock until commit/rollback; this is not an autonomous commit.
    pub async fn ensure_active_in(&self, tx: &Transaction<'_>, scope: &Scope, provider: &dyn KekProvider) -> Result<Dek, Error> {
        tx.execute(
            &format!("INSERT INTO {}.aip_encryption_scopes(scope_kind,scope_id) VALUES($1,$2) ON CONFLICT DO NOTHING", self.infra),
            &[&scope.kind, &scope.id],
        )
        .await?;
        self.lock_scope(tx, scope).await?;
        if let Some(row) = tx
            .query_opt(
                &format!(
                    "SELECT dek_id,version,kek_id,wrapped_dek FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND state='ACTIVE' FOR UPDATE",
                    self.infra
                ),
                &[&scope.kind, &scope.id],
            )
            .await?
        {
            return Dek::from_row(row);
        }
        // A scope with prior versions must be handled by explicit rekey/recovery, not silently
        // reset to version 1 after a key was retired or its metadata was damaged.
        let exists: bool = tx
            .query_one(
                &format!("SELECT EXISTS(SELECT 1 FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2)", self.infra),
                &[&scope.kind, &scope.id],
            )
            .await?
            .get(0);
        if exists {
            return Err(Error::Corrupted);
        }
        let id = Uuid::new_v4();
        let id_text = id.to_string();
        let mut raw = Zeroizing::new([0_u8; 32]);
        getrandom::fill(raw.as_mut()).map_err(|_| Error::Crypto)?;
        let context = WrapContext { schema: &self.deployment, scope_kind: scope.kind(), scope_id: scope.id(), dek_id: &id_text, version: 1 };
        let wrapped = tokio::time::timeout(std::time::Duration::from_secs(5), provider.wrap(&context, &raw))
            .await
            .map_err(|_| Error::Crypto)?
            .map_err(|_| Error::Crypto)?;
        if !valid_wrapped(&wrapped) {
            return Err(Error::Crypto);
        }
        tx.execute(
            &format!(
                "INSERT INTO {}.aip_encryption_deks(dek_id,scope_kind,scope_id,version,state,kek_id,wrapped_dek) VALUES($1,$2,$3,1,'ACTIVE',$4,$5)",
                self.infra
            ),
            &[&id, &scope.kind, &scope.id, &wrapped.kek_id, &wrapped.sealed],
        )
        .await?;
        Ok(Dek { id, id_text, version: 1, wrapped })
    }
    /// Locks the scope against promotion until commit/rollback, sharing it with
    /// other writers. Cached active pointers are insufficient: select ACTIVE in
    /// this same transaction and use the returned immutable identity. Missing or
    /// retired keys fail closed; this lookup does not create metadata or run DDL.
    pub async fn active_for_write_in(&self, tx: &Transaction<'_>, scope: &Scope) -> Result<WriteDek, Error> {
        let row = tx
            .query_opt(
                &format!("SELECT write_epoch FROM {}.aip_encryption_scopes WHERE scope_kind=$1 AND scope_id=$2 FOR SHARE", self.infra),
                &[&scope.kind, &scope.id],
            )
            .await?
            .ok_or(Error::NotFound)?;
        let epoch: i64 = row.try_get("write_epoch")?;
        if epoch <= 0 {
            return Err(Error::Corrupted);
        }
        let row=tx.query_opt(&format!("SELECT dek_id,version,kek_id,wrapped_dek FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND state='ACTIVE' FOR KEY SHARE",self.infra),&[&scope.kind,&scope.id]).await?.ok_or(Error::NotFound)?;
        Ok(WriteDek { epoch: epoch as u64, dek: Dek::from_row(row)? })
    }

    /// Reserve at most 500 cell seals on a dedicated, otherwise-idle connection. Invoke this
    /// on an independent connection; never use the resource connection here. Scope fencing
    /// prevents promotion while cell writers are live, but their key-share lock permits
    /// non-key counter reservations for another field/row of the same transaction.
    /// No raw key or frame is returned before the budget transaction commits. Cancellation after
    /// an uncertain commit wastes budget, rather than allowing an uncounted encryption attempt.
    pub async fn reserve_for_write(&self, budget: &mut Client, scope: &Scope, cells: u32) -> Result<SealReservation, Error> {
        if cells == 0 || cells > MAX_RESERVATION {
            return Err(Error::InvalidInput);
        }
        let tx = budget.transaction().await?;
        let row = tx
            .query_opt(
                &format!("SELECT write_epoch FROM {}.aip_encryption_scopes WHERE scope_kind=$1 AND scope_id=$2 FOR SHARE", self.infra),
                &[&scope.kind, &scope.id],
            )
            .await?
            .ok_or(Error::NotFound)?;
        let epoch: i64 = row.try_get("write_epoch")?;
        if epoch <= 0 {
            return Err(Error::Corrupted);
        }
        // Serialize counter reservations without a key-changing lock. Cell writers hold KEY SHARE
        // on this row, so another field in their transaction can reserve its own budget.
        // The scope SHARE lock above still conflicts with promotion's exclusive scope lock.
        let row = tx.query_opt(
            &format!("SELECT dek_id,version,kek_id,wrapped_dek,seal_count FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND state='ACTIVE' FOR NO KEY UPDATE",self.infra),
            &[&scope.kind, &scope.id],
        ).await?.ok_or(Error::NotFound)?;
        let used: i64 = row.try_get("seal_count")?;
        if used < 0 || used as u64 > MAX_DEK_SEALS {
            return Err(Error::Corrupted);
        }
        if cells as u64 > MAX_DEK_SEALS - used as u64 {
            return Err(Error::UsageExhausted);
        }
        let dek = Dek::from_row(row)?;
        tx.execute(&format!("UPDATE {}.aip_encryption_deks SET seal_count=seal_count+$2 WHERE dek_id=$1", self.infra), &[&dek.id, &(cells as i64)])
            .await?;
        tx.commit().await?;
        Ok(SealReservation {
            deployment: self.deployment.clone(),
            scope: scope.clone(),
            epoch: epoch as u64,
            dek_id: dek.id,
            version: dek.version,
            remaining: cells,
        })
    }

    /// Seal after policy/scalar validation and row-ID allocation, in the SAME transaction as the
    /// resource write. Locks the current DB epoch/key through commit; a cached old reservation is
    /// rejected after promotion. Rewrap is transparent because current wrapped metadata is loaded.
    /// A reservation is consumed before any provider/AEAD await, even if this future is cancelled.
    pub async fn seal_in(
        &self,
        tx: &Transaction<'_>,
        reservation: &mut SealReservation,
        location: &CellLocation,
        plaintext: &[u8],
        provider: &dyn KekProvider,
    ) -> Result<String, Error> {
        if reservation.deployment != self.deployment || plaintext.len() > cell::MAX_CELL_BYTES {
            return Err(Error::InvalidInput);
        }
        if reservation.remaining == 0 {
            return Err(Error::UsageExhausted);
        }
        reservation.remaining -= 1;
        let current = self.active_for_write_in(tx, &reservation.scope).await?;
        if current.epoch != reservation.epoch || current.dek.id != reservation.dek_id || current.dek.version != reservation.version {
            return Err(Error::StaleWriter);
        }
        let key = self.unwrap_key(&reservation.scope, &current.dek, provider).await?;
        key.seal(&self.cell_context(&reservation.scope, location), plaintext).map_err(|_| Error::Crypto)
    }

    /// Open only an already-authorized, non-null cell. No plaintext/v1 fallback and no scope-only
    /// key cache. The full frame identity is looked up inside this deployment/scope. RETIRED keys
    /// cannot be read through this boundary, even if a KEK provider still has their material.
    pub async fn open_cell(
        &self,
        client: &impl GenericClient,
        scope: &Scope,
        location: &CellLocation,
        frame: &str,
        provider: &dyn KekProvider,
    ) -> Result<Zeroizing<Vec<u8>>, Error> {
        let header = cell::parse_header(frame).map_err(|_| Error::Corrupted)?;
        let (id, version) = checked_identity(header.dek_id, header.version).map_err(|_| Error::Corrupted)?;
        let row = client.query_opt(
            &format!("SELECT dek_id,version,kek_id,wrapped_dek FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND dek_id=$3 AND version=$4 AND state IN ('ACTIVE','READABLE')",self.infra),
            &[&scope.kind, &scope.id, &id, &version],
        ).await?.ok_or(Error::NotFound)?;
        let dek = Dek::from_row(row)?;
        let key = self.unwrap_key(scope, &dek, provider).await?;
        key.open(&self.cell_context(scope, location), frame).map_err(|_| Error::Corrupted)
    }

    /// Start a new data-key generation, leaving the previous key READABLE for durable rekey.
    /// Takes an exclusive scope fence, waiting for all current writers, then increments epoch
    /// and version atomically with the ACTIVE switch. This does not retire keys or claim rekey
    /// completion. The caller must commit before handing the new identity to another transaction.
    pub async fn promote_in(&self, tx: &Transaction<'_>, scope: &Scope, expected_epoch: u64, provider: &dyn KekProvider) -> Result<WriteDek, Error> {
        if expected_epoch == 0 || expected_epoch >= i64::MAX as u64 {
            return Err(Error::InvalidInput);
        }
        let row = tx
            .query_opt(
                &format!("SELECT write_epoch FROM {}.aip_encryption_scopes WHERE scope_kind=$1 AND scope_id=$2 FOR UPDATE", self.infra),
                &[&scope.kind, &scope.id],
            )
            .await?
            .ok_or(Error::NotFound)?;
        let epoch: i64 = row.try_get("write_epoch")?;
        if epoch <= 0 {
            return Err(Error::Corrupted);
        }
        if epoch as u64 != expected_epoch {
            return Err(Error::StaleWriter);
        }
        let before = self.active_for_write_in(tx, scope).await?;
        let version = before.dek.version.checked_add(1).filter(|v| *v <= i32::MAX as u32).ok_or(Error::Corrupted)?;
        let id = Uuid::new_v4();
        let id_text = id.to_string();
        let mut raw = Zeroizing::new([0_u8; 32]);
        getrandom::fill(raw.as_mut()).map_err(|_| Error::Crypto)?;
        let context = WrapContext { schema: &self.deployment, scope_kind: scope.kind(), scope_id: scope.id(), dek_id: &id_text, version };
        let wrapped = tokio::time::timeout(std::time::Duration::from_secs(5), provider.wrap(&context, &raw))
            .await
            .map_err(|_| Error::Crypto)?
            .map_err(|_| Error::Crypto)?;
        if !valid_wrapped(&wrapped) {
            return Err(Error::Crypto);
        }
        tx.execute(&format!("UPDATE {}.aip_encryption_deks SET state='READABLE' WHERE dek_id=$1", self.infra), &[&before.dek.id]).await?;
        tx.execute(
            &format!(
                "INSERT INTO {}.aip_encryption_deks(dek_id,scope_kind,scope_id,version,state,kek_id,wrapped_dek) VALUES($1,$2,$3,$4,'ACTIVE',$5,$6)",
                self.infra
            ),
            &[&id, &scope.kind, &scope.id, &(version as i32), &wrapped.kek_id, &wrapped.sealed],
        )
        .await?;
        tx.execute(
            &format!("UPDATE {}.aip_encryption_scopes SET write_epoch=write_epoch+1 WHERE scope_kind=$1 AND scope_id=$2", self.infra),
            &[&scope.kind, &scope.id],
        )
        .await?;
        Ok(WriteDek { epoch: expected_epoch + 1, dek: Dek { id, id_text, version, wrapped } })
    }

    fn cell_context<'a>(&'a self, scope: &'a Scope, location: &'a CellLocation) -> CellContext<'a> {
        CellContext {
            schema: &self.deployment,
            resource: &location.resource,
            field: &location.field,
            row_id: &location.row_id,
            scope_kind: scope.kind(),
            scope_id: scope.id(),
        }
    }
    async fn unwrap_key(&self, scope: &Scope, dek: &Dek, provider: &dyn KekProvider) -> Result<DataKey, Error> {
        let context =
            WrapContext { schema: &self.deployment, scope_kind: scope.kind(), scope_id: scope.id(), dek_id: dek.id(), version: dek.version };
        let raw = tokio::time::timeout(std::time::Duration::from_secs(5), provider.unwrap(&context, &dek.wrapped))
            .await
            .map_err(|_| Error::Crypto)?
            .map_err(|_| Error::Crypto)?;
        DataKey::new(dek.id(), dek.version, *raw).map_err(|_| Error::Crypto)
    }
    /// Looks up the full immutable identity within its scope. This neither loads plaintext key
    /// material nor certifies that an old key is allowed for a new resource write.
    pub async fn find(&self, client: &impl GenericClient, scope: &Scope, id: &str, version: u32) -> Result<Dek, Error> {
        let (id, version) = checked_identity(id, version)?;
        let row = client.query_opt(&format!("SELECT dek_id,version,kek_id,wrapped_dek FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND dek_id=$3 AND version=$4", self.infra), &[&scope.kind, &scope.id, &id, &version]).await?.ok_or(Error::NotFound)?;
        Dek::from_row(row)
    }
    /// KEK-only rotation preserves DEK id/version and all resource cell frames. Failure leaves
    /// metadata untouched; the caller decides whether to roll back its surrounding transaction.
    pub async fn rewrap_in(
        &self,
        tx: &Transaction<'_>,
        scope: &Scope,
        id: &str,
        version: u32,
        previous: &dyn KekProvider,
        next: &dyn KekProvider,
    ) -> Result<Dek, Error> {
        self.lock_scope(tx, scope).await?;
        let (id, version) = checked_identity(id, version)?;
        let row = tx.query_opt(&format!("SELECT dek_id,version,kek_id,wrapped_dek FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND dek_id=$3 AND version=$4 FOR UPDATE", self.infra), &[&scope.kind, &scope.id, &id, &version]).await?.ok_or(Error::NotFound)?;
        let before = Dek::from_row(row)?;
        let context =
            WrapContext { schema: &self.deployment, scope_kind: scope.kind(), scope_id: scope.id(), dek_id: before.id(), version: before.version };
        let raw = tokio::time::timeout(std::time::Duration::from_secs(5), previous.unwrap(&context, &before.wrapped))
            .await
            .map_err(|_| Error::Crypto)?
            .map_err(|_| Error::Crypto)?;
        let wrapped = tokio::time::timeout(std::time::Duration::from_secs(5), next.wrap(&context, &raw))
            .await
            .map_err(|_| Error::Crypto)?
            .map_err(|_| Error::Crypto)?;
        if !valid_wrapped(&wrapped) {
            return Err(Error::Crypto);
        }
        tx.execute(&format!("UPDATE {}.aip_encryption_deks SET kek_id=$4,wrapped_dek=$5,rewrapped_at=clock_timestamp() WHERE scope_kind=$1 AND scope_id=$2 AND dek_id=$3", self.infra), &[&scope.kind, &scope.id, &before.id, &wrapped.kek_id, &wrapped.sealed]).await?;
        Ok(Dek { wrapped, ..before })
    }
    /// Lists all READABLE DEK generations for the scope, ordered by version.
    /// Used by durable rekey to discover keys that need backfill before retirement.
    pub async fn list_readable_in(&self, tx: &Transaction<'_>, scope: &Scope) -> Result<Vec<Dek>, Error> {
        let rows = tx
            .query(
                &format!(
                    "SELECT dek_id,version,kek_id,wrapped_dek FROM {}.aip_encryption_deks WHERE scope_kind=$1 AND scope_id=$2 AND state='READABLE' ORDER BY version ASC",
                    self.infra
                ),
                &[&scope.kind, &scope.id],
            )
            .await?;
        let mut deks = Vec::with_capacity(rows.len());
        for row in rows {
            deks.push(Dek::from_row(row)?);
        }
        Ok(deks)
    }
    /// Transitions a READABLE DEK generation to RETIRED in the caller's transaction.
    /// Fails closed if the key is not in READABLE state, or if the scope's write_epoch has changed
    /// from expected_epoch (preventing stale workers from retiring keys across promotions).
    pub async fn retire_dek_in(&self, tx: &Transaction<'_>, scope: &Scope, dek_id: &str, version: u32, expected_epoch: u64) -> Result<(), Error> {
        self.lock_scope(tx, scope).await?;
        let (id, version_i32) = checked_identity(dek_id, version)?;
        let row = tx
            .query_opt(
                &format!("SELECT write_epoch FROM {}.aip_encryption_scopes WHERE scope_kind=$1 AND scope_id=$2 FOR UPDATE", self.infra),
                &[&scope.kind, &scope.id],
            )
            .await?
            .ok_or(Error::NotFound)?;
        let epoch: i64 = row.try_get("write_epoch")?;
        if epoch <= 0 {
            return Err(Error::Corrupted);
        }
        if epoch as u64 != expected_epoch {
            return Err(Error::StaleWriter);
        }
        let updated = tx
            .execute(
                &format!(
                    "UPDATE {}.aip_encryption_deks SET state='RETIRED' WHERE scope_kind=$1 AND scope_id=$2 AND dek_id=$3 AND version=$4 AND state='READABLE'",
                    self.infra
                ),
                &[&scope.kind, &scope.id, &id, &version_i32],
            )
            .await?;
        if updated != 1 {
            return Err(Error::NotFound);
        }
        Ok(())
    }
}
fn checked_identity(id: &str, version: u32) -> Result<(Uuid, i32), Error> {
    let uuid = Uuid::parse_str(id).map_err(|_| Error::InvalidInput)?;
    if id != uuid.to_string() || version == 0 {
        return Err(Error::InvalidInput);
    }
    Ok((uuid, i32::try_from(version).map_err(|_| Error::InvalidInput)?))
}
fn canonical_id(id: &str) -> bool {
    id.parse::<i64>().is_ok_and(|v| v >= 0 && v.to_string() == id) || Uuid::parse_str(id).is_ok_and(|v| v.to_string() == id)
}
fn valid_resource(name: &str) -> bool {
    !name.is_empty() && name.len() <= 63 && name.as_bytes()[0].is_ascii_alphabetic() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn valid_wrapped(wrapped: &WrappedDek) -> bool {
    !wrapped.kek_id.is_empty()
        && wrapped.kek_id.len() <= 64
        && wrapped.kek_id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && wrapped.sealed.len() == 60
}
