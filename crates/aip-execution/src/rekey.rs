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

impl RekeyLease {
    /// Attempts to acquire or renew a lease for `target_key`.
    /// Returns `true` if the lease was granted, `false` if held by another active worker.
    pub async fn acquire(
        client: &impl GenericClient,
        infra_schema: &str,
        target_key: &str,
        holder: &str,
        epoch: u64,
        ttl_secs: u64,
    ) -> Result<bool, StoreError> {
        let ttl_i64 = ttl_secs.min(3600) as i64;
        let epoch_i64 = epoch as i64;
        let statement = format!(
            "INSERT INTO {infra_schema}.aip_rekey_leases (target_key, holder, epoch, leased_until)
             VALUES ($1, $2, $3, clock_timestamp() + ($4 || ' seconds')::interval)
             ON CONFLICT (target_key) DO UPDATE
             SET holder = EXCLUDED.holder, epoch = EXCLUDED.epoch, leased_until = EXCLUDED.leased_until
             WHERE aip_rekey_leases.leased_until < clock_timestamp()
                OR (aip_rekey_leases.holder = EXCLUDED.holder AND aip_rekey_leases.epoch = EXCLUDED.epoch)"
        );
        let rows = client.execute(&statement, &[&target_key, &holder, &epoch_i64, &ttl_i64.to_string()]).await?;
        Ok(rows > 0)
    }

    /// Releases a held lease.
    pub async fn release(client: &impl GenericClient, infra_schema: &str, target_key: &str, holder: &str) -> Result<(), StoreError> {
        let statement = format!("DELETE FROM {infra_schema}.aip_rekey_leases WHERE target_key = $1 AND holder = $2");
        client.execute(&statement, &[&target_key, &holder]).await?;
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
    let acquired =
        RekeyLease::acquire(db, ctx.store.infrastructure_schema(), &target_key, ctx.holder_id, ctx.active_dek.epoch(), DEFAULT_LEASE_TTL_SECS)
            .await?;

    if !acquired {
        return Err(StoreError::StaleWriter);
    }

    let mut pass = 0;
    while pass < MAX_REKEY_PASSES {
        pass += 1;
        let mut cursor = String::new();
        let mut pass_raced = 0;

        loop {
            // Heartbeat lease
            let renewed = RekeyLease::acquire(
                db,
                ctx.store.infrastructure_schema(),
                &target_key,
                ctx.holder_id,
                ctx.active_dek.epoch(),
                DEFAULT_LEASE_TTL_SECS,
            )
            .await?;
            if !renewed {
                let _ = RekeyLease::release(db, ctx.store.infrastructure_schema(), &target_key, ctx.holder_id).await;
                return Err(StoreError::StaleWriter);
            }

            let select_sql = format!(
                "SELECT \"{}\"::text, \"{}\" FROM \"{}\" WHERE \"{}\" IS NOT NULL AND \"{}\"::text > $1 ORDER BY \"{}\"::text ASC LIMIT $2",
                target.id_column, target.column, target.table, target.column, target.id_column, target.id_column
            );

            let limit = REKEY_BATCH_SIZE as i64;
            let rows = db.query(&select_sql, &[&cursor, &limit]).await?;
            if rows.is_empty() {
                break;
            }

            for row in &rows {
                report.scanned += 1;
                let id: String = row.get(0);
                let stored_frame: String = row.get(1);
                cursor = id.clone();

                let header = match cell::parse_header(&stored_frame) {
                    Ok(h) => h,
                    Err(_) => {
                        report.failed_ids.push(id);
                        continue;
                    }
                };

                if header.dek_id == ctx.active_dek.dek().id() && header.version == ctx.active_dek.dek().version() {
                    report.already_current += 1;
                    continue;
                }

                if header.dek_id != ctx.old_dek.id() || header.version != ctx.old_dek.version() {
                    // Belongs to another generation; record and do not mutate
                    report.failed_ids.push(id);
                    continue;
                }

                // Decrypt old cell
                let location = match CellLocation::new(&target.resource, &target.field, &id) {
                    Ok(loc) => loc,
                    Err(_) => {
                        report.failed_ids.push(id);
                        continue;
                    }
                };

                let plaintext = match ctx.store.open_cell(db, &target.scope, &location, &stored_frame, ctx.provider).await {
                    Ok(pt) => pt,
                    Err(_) => {
                        report.failed_ids.push(id);
                        continue;
                    }
                };

                // Reserve budget on independent budget connection
                let mut reservation = match ctx.store.reserve_for_write(budget, &target.scope, 1).await {
                    Ok(res) => res,
                    Err(e) => {
                        let _ = RekeyLease::release(db, ctx.store.infrastructure_schema(), &target_key, ctx.holder_id).await;
                        return Err(e);
                    }
                };

                // Seal new cell and CAS update in the same resource transaction
                let tx = db.transaction().await?;
                let sealed_frame = match ctx.store.seal_in(&tx, &mut reservation, &location, &plaintext, ctx.provider).await {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = tx.rollback().await;
                        let _ = RekeyLease::release(db, ctx.store.infrastructure_schema(), &target_key, ctx.holder_id).await;
                        return Err(e);
                    }
                };

                let update_sql = format!(
                    "UPDATE \"{}\" SET \"{}\" = $1 WHERE \"{}\"::text = $2 AND \"{}\" = $3",
                    target.table, target.column, target.id_column, target.column
                );

                let updated = tx.execute(&update_sql, &[&sealed_frame, &id, &stored_frame]).await?;
                if updated == 1 {
                    tx.commit().await?;
                    report.rekeyed += 1;
                } else {
                    tx.rollback().await?;
                    report.raced += 1;
                    pass_raced += 1;
                }
            }
        }

        if pass_raced == 0 {
            break;
        }
    }

    // Final 0 verification scan: ensure zero cells remain with old_dek.id
    let mut remaining = 0;
    let mut check_cursor = String::new();
    loop {
        let verify_sql = format!(
            "SELECT \"{}\"::text, \"{}\" FROM \"{}\" WHERE \"{}\" IS NOT NULL AND \"{}\"::text > $1 ORDER BY \"{}\"::text ASC LIMIT $2",
            target.id_column, target.column, target.table, target.column, target.id_column, target.id_column
        );
        let limit = REKEY_BATCH_SIZE as i64;
        let rows = db.query(&verify_sql, &[&check_cursor, &limit]).await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let id: String = row.get(0);
            let frame: String = row.get(1);
            check_cursor = id;

            if cell::parse_header(&frame).is_ok_and(|h| h.dek_id == ctx.old_dek.id() && h.version == ctx.old_dek.version()) {
                remaining += 1;
            }
        }
    }
    report.remaining_old = remaining;

    let _ = RekeyLease::release(db, ctx.store.infrastructure_schema(), &target_key, ctx.holder_id).await;
    Ok(report)
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

            if target_rep.remaining_old > 0 || !target_rep.failed_ids.is_empty() {
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
