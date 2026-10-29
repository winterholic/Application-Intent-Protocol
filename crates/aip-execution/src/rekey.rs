//! Durable, multi-pass rekey execution boundary for PostgreSQL field encryption.
//!
//! Re-encrypts stored cells from `READABLE` DEK generations to the currently `ACTIVE` DEK
//! generation using keyset cursor pagination, atomic compare-and-set updates, and durable leases.
//! Retires old DEKs only when a full verification pass proves zero remaining cells (`final 0`).
//!
//! Stale writers and concurrent rekey workers are locked out via epoch verification and lease
//! timeouts. Stale row mutations during rekey are safely detected via CAS (`WHERE col = <old_frame>`)
//! and handled through bounded multi-pass re-scans.

use crate::encryption::{CellLocation, Dek, DekStore, Error as StoreError, Scope, WriteDek};
use aip_crypto::cell;
use aip_crypto::provider::KekProvider;
use tokio_postgres::{Client, GenericClient};

/// Batch size for keyset cursor pagination.
pub const REKEY_BATCH_SIZE: u32 = 100;
/// Maximum re-scan passes for raced rows before reporting partial completion.
pub const MAX_REKEY_PASSES: u32 = 3;
/// Default lease time-to-live in seconds.
pub const DEFAULT_LEASE_TTL_SECS: u64 = 60;

/// Specification of a database column holding encrypted cell frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RekeyTarget {
    pub table: String,
    pub column: String,
    pub id_column: String,
    pub resource: String,
    pub field: String,
    pub scope: Scope,
}

impl RekeyTarget {
    pub fn new(
        table: impl Into<String>,
        column: impl Into<String>,
        id_column: impl Into<String>,
        resource: impl Into<String>,
        field: impl Into<String>,
        scope: Scope,
    ) -> Self {
        Self { table: table.into(), column: column.into(), id_column: id_column.into(), resource: resource.into(), field: field.into(), scope }
    }

    pub fn target_key(&self) -> String {
        format!("{}:{}:{}:{}", self.scope.kind(), self.scope.id(), self.table, self.column)
    }
}

/// Statistics for a single target column rekey operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RekeyTargetReport {
    pub table: String,
    pub column: String,
    pub scanned: u64,
    pub rekeyed: u64,
    pub already_current: u64,
    pub raced: u64,
    pub failed_ids: Vec<String>,
    pub failed_count: u64,
    pub remaining_old: u64,
}

/// High-level report for a DEK rekey cycle across multiple targets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RekeyReport {
    pub old_dek_id: String,
    pub old_version: u32,
    pub active_dek_id: String,
    pub active_version: u32,
    pub target_reports: Vec<RekeyTargetReport>,
    pub fully_retired: bool,
}

/// Context for a single rekey execution session across target columns.
pub struct RekeyContext<'a> {
    pub store: &'a DekStore,
    pub old_dek: &'a Dek,
    pub active_dek: &'a WriteDek,
    pub holder_id: &'a str,
    pub provider: &'a dyn KekProvider,
}

/// Manages durable leases in the infrastructure schema to serialize rekey workers.
pub struct RekeyLease;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RekeyCheckpoint {
    phase: String,
    pass_no: u32,
    cursor_id: String,
    remaining_old: u64,
    failed_count: u64,
}

struct LeaseIdentity<'a> {
    infra_schema: &'a str,
    target_key: &'a str,
    holder: &'a str,
    epoch: u64,
    old_dek_id: &'a str,
    old_version: u32,
    active_dek_id: &'a str,
    active_version: u32,
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

impl RekeyLease {
    /// Acquires a lease and resumes its durable cursor when the key generations still match.
    /// Returns `None` when another unexpired worker owns the target.
    async fn acquire(client: &impl GenericClient, identity: &LeaseIdentity<'_>, ttl_secs: u64) -> Result<Option<RekeyCheckpoint>, StoreError> {
        let ttl_i64 = ttl_secs.min(3600) as i64;
        let epoch_i64 = identity.epoch as i64;
        let statement = format!(
            "INSERT INTO {}.aip_rekey_leases
               (target_key, holder, epoch, leased_until, phase, pass_no, cursor_id, old_dek_id, old_version, active_dek_id, active_version, remaining_old, failed_count)
             VALUES ($1, $2, $3, clock_timestamp() + ($4 || ' seconds')::interval, 'sweep', 1, '', $5::uuid, $6, $7::uuid, $8, 0, 0)
             ON CONFLICT (target_key) DO UPDATE
             SET holder = EXCLUDED.holder, epoch = EXCLUDED.epoch, leased_until = EXCLUDED.leased_until
                , phase = CASE WHEN aip_rekey_leases.old_dek_id = $5::uuid AND aip_rekey_leases.old_version = $6
                                    AND aip_rekey_leases.active_dek_id = $7::uuid AND aip_rekey_leases.active_version = $8
                               THEN aip_rekey_leases.phase ELSE 'sweep' END
                , pass_no = CASE WHEN aip_rekey_leases.old_dek_id = $5::uuid AND aip_rekey_leases.old_version = $6
                                      AND aip_rekey_leases.active_dek_id = $7::uuid AND aip_rekey_leases.active_version = $8
                                 THEN aip_rekey_leases.pass_no ELSE 1 END
                , cursor_id = CASE WHEN aip_rekey_leases.old_dek_id = $5::uuid AND aip_rekey_leases.old_version = $6
                                        AND aip_rekey_leases.active_dek_id = $7::uuid AND aip_rekey_leases.active_version = $8
                                   THEN aip_rekey_leases.cursor_id ELSE '' END
                , remaining_old = CASE WHEN aip_rekey_leases.old_dek_id = $5::uuid AND aip_rekey_leases.old_version = $6
                                            AND aip_rekey_leases.active_dek_id = $7::uuid AND aip_rekey_leases.active_version = $8
                                       THEN aip_rekey_leases.remaining_old ELSE 0 END
                , failed_count = CASE WHEN aip_rekey_leases.old_dek_id = $5::uuid AND aip_rekey_leases.old_version = $6
                                           AND aip_rekey_leases.active_dek_id = $7::uuid AND aip_rekey_leases.active_version = $8
                                      THEN aip_rekey_leases.failed_count ELSE 0 END
                , old_dek_id = $5::uuid, old_version = $6, active_dek_id = $7::uuid, active_version = $8
             WHERE aip_rekey_leases.leased_until < clock_timestamp()
                OR (aip_rekey_leases.holder = EXCLUDED.holder AND aip_rekey_leases.epoch = EXCLUDED.epoch)
             RETURNING phase, pass_no, cursor_id, remaining_old, failed_count",
            identity.infra_schema
        );
        let row = client
            .query_opt(
                &statement,
                &[
                    &identity.target_key,
                    &identity.holder,
                    &epoch_i64,
                    &ttl_i64.to_string(),
                    &identity.old_dek_id,
                    &(identity.old_version as i32),
                    &identity.active_dek_id,
                    &(identity.active_version as i32),
                ],
            )
            .await?;
        row.map(|row| {
            let phase: String = row.try_get("phase")?;
            let pass_no: i32 = row.try_get("pass_no")?;
            let remaining_old: i64 = row.try_get("remaining_old")?;
            let failed_count: i64 = row.try_get("failed_count")?;
            if !matches!(phase.as_str(), "sweep" | "verify")
                || !(1..=MAX_REKEY_PASSES as i32).contains(&pass_no)
                || remaining_old < 0
                || failed_count < 0
            {
                return Err(StoreError::Corrupted);
            }
            Ok(RekeyCheckpoint {
                phase,
                pass_no: pass_no as u32,
                cursor_id: row.try_get("cursor_id")?,
                remaining_old: remaining_old as u64,
                failed_count: failed_count as u64,
            })
        })
        .transpose()
    }

    async fn renew(client: &impl GenericClient, identity: &LeaseIdentity<'_>, ttl_secs: u64) -> Result<bool, StoreError> {
        let ttl_i64 = ttl_secs.min(3600) as i64;
        let epoch_i64 = identity.epoch as i64;
        let sql = format!(
            "UPDATE {}.aip_rekey_leases SET leased_until=clock_timestamp() + ($1 || ' seconds')::interval
             WHERE target_key=$2 AND holder=$3 AND epoch=$4 AND old_dek_id=$5::uuid AND old_version=$6
               AND active_dek_id=$7::uuid AND active_version=$8 AND leased_until > clock_timestamp()",
            identity.infra_schema
        );
        let changed = client
            .execute(
                &sql,
                &[
                    &ttl_i64.to_string(),
                    &identity.target_key,
                    &identity.holder,
                    &epoch_i64,
                    &identity.old_dek_id,
                    &(identity.old_version as i32),
                    &identity.active_dek_id,
                    &(identity.active_version as i32),
                ],
            )
            .await?;
        Ok(changed == 1)
    }

    /// Releases only the exact lease generation owned by this worker.
    async fn release(client: &impl GenericClient, identity: &LeaseIdentity<'_>) -> Result<(), StoreError> {
        let epoch_i64 = identity.epoch as i64;
        let statement = format!(
            "DELETE FROM {}.aip_rekey_leases
             WHERE target_key=$1 AND holder=$2 AND epoch=$3 AND old_dek_id=$4::uuid AND old_version=$6
               AND active_dek_id=$5::uuid AND active_version=$7 AND leased_until > clock_timestamp()",
            identity.infra_schema
        );
        let changed = client
            .execute(
                &statement,
                &[
                    &identity.target_key,
                    &identity.holder,
                    &epoch_i64,
                    &identity.old_dek_id,
                    &identity.active_dek_id,
                    &(identity.old_version as i32),
                    &(identity.active_version as i32),
                ],
            )
            .await?;
        if changed != 1 {
            return Err(StoreError::StaleWriter);
        }
        Ok(())
    }

    async fn checkpoint(
        client: &impl GenericClient,
        identity: &LeaseIdentity<'_>,
        checkpoint: &RekeyCheckpoint,
        ttl_secs: u64,
    ) -> Result<(), StoreError> {
        let ttl_i64 = ttl_secs.min(3600) as i64;
        let epoch_i64 = identity.epoch as i64;
        let pass_no = checkpoint.pass_no as i32;
        let remaining_old = i64::try_from(checkpoint.remaining_old).map_err(|_| StoreError::InvalidInput)?;
        let failed_count = i64::try_from(checkpoint.failed_count).map_err(|_| StoreError::InvalidInput)?;
        let sql = format!(
            "UPDATE {}.aip_rekey_leases
             SET phase=$1, pass_no=$2, cursor_id=$3, remaining_old=$4, failed_count=$5,
                 leased_until=clock_timestamp() + ($6 || ' seconds')::interval
             WHERE target_key=$7 AND holder=$8 AND epoch=$9 AND old_dek_id=$10::uuid AND old_version=$12
               AND active_dek_id=$11::uuid AND active_version=$13
               AND leased_until > clock_timestamp()",
            identity.infra_schema
        );
        let changed = client
            .execute(
                &sql,
                &[
                    &checkpoint.phase,
                    &pass_no,
                    &checkpoint.cursor_id,
                    &remaining_old,
                    &failed_count,
                    &ttl_i64.to_string(),
                    &identity.target_key,
                    &identity.holder,
                    &epoch_i64,
                    &identity.old_dek_id,
                    &identity.active_dek_id,
                    &(identity.old_version as i32),
                    &(identity.active_version as i32),
                ],
            )
            .await?;
        if changed != 1 {
            return Err(StoreError::StaleWriter);
        }
        Ok(())
    }

    async fn yield_for_resume(client: &impl GenericClient, identity: &LeaseIdentity<'_>) -> Result<(), StoreError> {
        let epoch_i64 = identity.epoch as i64;
        let sql = format!(
            "UPDATE {}.aip_rekey_leases SET leased_until=clock_timestamp()
             WHERE target_key=$1 AND holder=$2 AND epoch=$3 AND old_dek_id=$4::uuid AND old_version=$6
               AND active_dek_id=$5::uuid AND active_version=$7 AND leased_until > clock_timestamp()",
            identity.infra_schema
        );
        let changed = client
            .execute(
                &sql,
                &[
                    &identity.target_key,
                    &identity.holder,
                    &epoch_i64,
                    &identity.old_dek_id,
                    &identity.active_dek_id,
                    &(identity.old_version as i32),
                    &(identity.active_version as i32),
                ],
            )
            .await?;
        if changed != 1 {
            return Err(StoreError::StaleWriter);
        }
        Ok(())
    }
}

/// Executes durable rekeying for a single column target.
pub async fn rekey_target_column(
    db: &mut Client,
    budget: &mut Client,
    ctx: &RekeyContext<'_>,
    target: &RekeyTarget,
) -> Result<RekeyTargetReport, StoreError> {
    let mut report = RekeyTargetReport { table: target.table.clone(), column: target.column.clone(), ..Default::default() };
    let target_key = target.target_key();
    let infra = ctx.store.infrastructure_schema();
    let epoch = ctx.active_dek.epoch();
    let old_dek_id = ctx.old_dek.id();
    let active_dek_id = ctx.active_dek.dek().id();
    let identity = LeaseIdentity {
        infra_schema: infra,
        target_key: &target_key,
        holder: ctx.holder_id,
        epoch,
        old_dek_id,
        old_version: ctx.old_dek.version(),
        active_dek_id,
        active_version: ctx.active_dek.dek().version(),
    };
    let mut checkpoint = RekeyLease::acquire(db, &identity, DEFAULT_LEASE_TTL_SECS).await?.ok_or(StoreError::StaleWriter)?;

    let table = quote_ident(&target.table);
    let column = quote_ident(&target.column);
    let id_column = quote_ident(&target.id_column);
    loop {
        if !RekeyLease::renew(db, &identity, DEFAULT_LEASE_TTL_SECS).await? {
            return Err(StoreError::StaleWriter);
        }

        let select_sql = format!(
            "SELECT {id_column}::text, {column} FROM {table} WHERE {column} IS NOT NULL AND {id_column}::text > $1 ORDER BY {id_column}::text ASC LIMIT $2"
        );
        let rows = db.query(&select_sql, &[&checkpoint.cursor_id, &(REKEY_BATCH_SIZE as i64)]).await?;
        if !rows.is_empty() {
            for row in &rows {
                let id: String = row.try_get(0)?;
                let frame: String = row.try_get(1)?;
                report.scanned += 1;
                checkpoint.cursor_id = id.clone();

                if checkpoint.phase == "verify" {
                    if cell::parse_header(&frame).is_ok_and(|h| h.dek_id == old_dek_id && h.version == ctx.old_dek.version()) {
                        checkpoint.remaining_old = checkpoint.remaining_old.checked_add(1).ok_or(StoreError::InvalidInput)?;
                    }
                    RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                    continue;
                }

                let header = match cell::parse_header(&frame) {
                    Ok(header) => header,
                    Err(_) => {
                        report.failed_ids.push(id);
                        checkpoint.failed_count = checkpoint.failed_count.checked_add(1).ok_or(StoreError::InvalidInput)?;
                        RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                        continue;
                    }
                };
                let is_old = header.dek_id == old_dek_id && header.version == ctx.old_dek.version();
                let is_active = header.dek_id == active_dek_id && header.version == ctx.active_dek.dek().version();
                if is_active {
                    report.already_current += 1;
                    RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                    continue;
                }
                if !is_old {
                    RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                    continue;
                }

                let location = match CellLocation::new(&target.resource, &target.field, &id) {
                    Ok(location) => location,
                    Err(_) => {
                        report.failed_ids.push(id);
                        checkpoint.failed_count = checkpoint.failed_count.checked_add(1).ok_or(StoreError::InvalidInput)?;
                        RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                        continue;
                    }
                };
                let plaintext = match ctx.store.open_cell(db, &target.scope, &location, &frame, ctx.provider).await {
                    Ok(plaintext) => plaintext,
                    Err(_) => {
                        report.failed_ids.push(id);
                        checkpoint.failed_count = checkpoint.failed_count.checked_add(1).ok_or(StoreError::InvalidInput)?;
                        RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                        continue;
                    }
                };
                let mut reservation = ctx.store.reserve_for_write(budget, &target.scope, 1).await?;
                let tx = db.transaction().await?;
                let sealed_frame = ctx.store.seal_in(&tx, &mut reservation, &location, &plaintext, ctx.provider).await?;
                let update_sql = format!("UPDATE {table} SET {column}=$1 WHERE {id_column}::text=$2 AND {column}=$3");
                let updated = tx.execute(&update_sql, &[&sealed_frame, &id, &frame]).await?;
                RekeyLease::checkpoint(&tx, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
                tx.commit().await?;
                if updated == 1 {
                    report.rekeyed += 1;
                } else {
                    report.raced += 1;
                }
            }
            continue;
        }

        if checkpoint.phase == "sweep" {
            checkpoint.phase = "verify".to_owned();
            checkpoint.cursor_id.clear();
            checkpoint.remaining_old = 0;
            RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
            continue;
        }

        report.remaining_old = checkpoint.remaining_old;
        report.failed_count = checkpoint.failed_count;
        if checkpoint.remaining_old == 0 && checkpoint.failed_count == 0 {
            RekeyLease::release(db, &identity).await?;
            return Ok(report);
        }
        if checkpoint.pass_no < MAX_REKEY_PASSES {
            checkpoint.phase = "sweep".to_owned();
            checkpoint.pass_no += 1;
            checkpoint.cursor_id.clear();
            checkpoint.remaining_old = 0;
            checkpoint.failed_count = 0;
            RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
            continue;
        }
        checkpoint.phase = "sweep".to_owned();
        checkpoint.pass_no = 1;
        checkpoint.cursor_id.clear();
        checkpoint.remaining_old = 0;
        checkpoint.failed_count = 0;
        RekeyLease::checkpoint(db, &identity, &checkpoint, DEFAULT_LEASE_TTL_SECS).await?;
        RekeyLease::yield_for_resume(db, &identity).await?;
        return Ok(report);
    }
}

/// Runs a full rekey cycle for all READABLE DEK generations in `scope`.
/// When all targets have 0 remaining old cells and no failures, the old DEK is safely transitioned to `RETIRED`.
pub async fn rekey_scope(
    db: &mut Client,
    budget: &mut Client,
    store: &DekStore,
    scope: &Scope,
    targets: &[RekeyTarget],
    holder_id: &str,
    provider: &dyn KekProvider,
) -> Result<Vec<RekeyReport>, StoreError> {
    let tx = db.transaction().await?;
    let readable_deks = store.list_readable_in(&tx, scope).await?;
    let active_dek = store.active_for_write_in(&tx, scope).await?;
    tx.rollback().await?;

    if readable_deks.is_empty() {
        return Ok(vec![]);
    }

    let mut reports = Vec::new();
    for old_dek in readable_deks {
        let mut report = RekeyReport {
            old_dek_id: old_dek.id().to_string(),
            old_version: old_dek.version(),
            active_dek_id: active_dek.dek().id().to_string(),
            active_version: active_dek.dek().version(),
            target_reports: Vec::new(),
            fully_retired: false,
        };

        let ctx = RekeyContext { store, old_dek: &old_dek, active_dek: &active_dek, holder_id, provider };

        let mut all_zero = true;
        for target in targets {
            if target.scope != *scope {
                continue;
            }
            let target_rep = rekey_target_column(db, budget, &ctx, target).await?;

            if target_rep.remaining_old > 0 || target_rep.failed_count > 0 || !target_rep.failed_ids.is_empty() {
                all_zero = false;
            }
            report.target_reports.push(target_rep);
        }

        // Final 0 condition met: retire the old DEK
        if all_zero && !report.target_reports.is_empty() {
            let tx = db.transaction().await?;
            store.retire_dek_in(&tx, scope, old_dek.id(), old_dek.version(), active_dek.epoch()).await?;
            tx.commit().await?;
            report.fully_retired = true;
        }

        reports.push(report);
    }

    Ok(reports)
}
