//! `aip rekey`: re-encrypts every stored value that is not under the current key (the first of `AIP_ENCRYPTION_KEYS`).
//!
//! It walks each encrypted column in keyset batches, one transaction per batch, and a value that is already current is
//! left alone, so it can be stopped and run again. The write is compare-and-set (`WHERE id = $1 AND col = <what was
//! read>`): a value a running server changed in the meantime is not overwritten, and it was written with the current
//! key of that server anyway. The copies of an encrypted column (the published snapshot, the versions of a `history`
//! entity) are re-encrypted too, or a key could never be retired.
//!
//! The row's user triggers are off inside the batch's transaction: re-encrypting is not an edit, so it must not bump
//! `version`, touch `updated_at`, write another history version or wake subscriptions. `DISABLE TRIGGER` takes a lock
//! that holds writers back for the length of one batch.

use crate::crypto::Keys;
use aip_plan::Program;
use deadpool_postgres::Pool;

const BATCH: i64 = 500;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RekeyReport {
    /// `Entity.field` and the table the values are in.
    pub field: String,
    pub table: String,
    pub rekeyed: u64,
    pub already_current: u64,
    /// Values that could not be decrypted (unknown key, changed, moved), left as they are. The ids are for the operator.
    pub failed: Vec<String>,
    /// Values changed by someone else between the read and the write; run again to be sure.
    pub raced: u64,
}

fn q(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// One place an encrypted value is kept.
struct Target {
    field: String,
    table: String,
    column: String,
    /// The column of the table, or a key of the `data` document of a history table.
    history: bool,
}

fn targets(program: &Program) -> Vec<Target> {
    let mut out = Vec::new();
    for (entity, spec) in &program.entities {
        for col in spec.columns.iter().filter(|c| c.encrypted) {
            let field = format!("{entity}.{}", col.field);
            let mut add = |table: &str, history: bool| {
                out.push(Target { field: field.clone(), table: table.to_string(), column: col.column.clone(), history });
            };
            add(&spec.table, false);
            if let Some(t) = &spec.published_table {
                add(t, false);
            }
            if let Some(t) = &spec.history_table {
                add(t, true);
            }
        }
    }
    out
}

pub async fn rekey(pool: &Pool, program: &Program, keys: &Keys) -> anyhow::Result<Vec<RekeyReport>> {
    let mut reports = Vec::new();
    let prefix = format!("v1:{}:", keys.current());
    for t in targets(program) {
        reports.push(rekey_target(pool, &t, keys, &prefix).await?);
    }
    Ok(reports)
}

async fn rekey_target(pool: &Pool, t: &Target, keys: &Keys, prefix: &str) -> anyhow::Result<RekeyReport> {
    let mut report = RekeyReport { field: t.field.clone(), table: t.table.clone(), ..Default::default() };
    let (table, col) = (q(&t.table), q(&t.column));
    // a history table keeps one row per (id, version) with the row as a JSON document; the others one per id
    let value = if t.history { format!("\"data\" ->> '{}'", t.column.replace('\'', "''")) } else { col.clone() };
    let select = format!(
        "SELECT \"id\"::text, {} AS ver, {value} AS v FROM {table} WHERE ({value}) IS NOT NULL AND (\"id\", {}) > ($1::text::uuid, $2::bigint) ORDER BY \"id\"{} LIMIT {BATCH}",
        if t.history { "\"version\"" } else { "0::bigint" },
        if t.history { "\"version\"" } else { "0::bigint" },
        if t.history { ", \"version\"" } else { "" }
    );
    // the nil uuid sorts before every row id
    let mut after: (String, i64) = ("00000000-0000-0000-0000-000000000000".into(), 0);
    loop {
        let mut client = pool.get().await?;
        let tx = client.transaction().await?;
        tx.execute(format!("ALTER TABLE {table} DISABLE TRIGGER USER").as_str(), &[]).await?;
        let rows = tx.query(select.as_str(), &[&after.0, &after.1]).await?;
        if rows.is_empty() {
            tx.rollback().await?;
            break;
        }
        for r in &rows {
            let (id, version, stored): (String, i64, String) = (r.get(0), r.get(1), r.get(2));
            after = (id.clone(), version);
            if stored.starts_with(prefix) {
                report.already_current += 1;
                continue;
            }
            let Ok(plain) = keys.decrypt(&t.field, &id, &stored) else {
                report.failed.push(if t.history { format!("{id}@{version}") } else { id });
                continue;
            };
            let fresh = keys.encrypt(&t.field, &id, &plain);
            let n = if t.history {
                tx.execute(
                    format!(
                        "UPDATE {table} SET \"data\" = jsonb_set(\"data\", ARRAY[$4::text], to_jsonb($5::text)) \
                         WHERE \"id\" = $1::text::uuid AND \"version\" = $2 AND \"data\" ->> $4 = $3"
                    )
                    .as_str(),
                    &[&id, &version, &stored, &t.column, &fresh],
                )
                .await?
            } else {
                tx.execute(format!("UPDATE {table} SET {col} = $3 WHERE \"id\" = $1::text::uuid AND {col} = $2").as_str(), &[&id, &stored, &fresh])
                    .await?
            };
            if n == 1 {
                report.rekeyed += 1;
            } else {
                report.raced += 1;
            }
        }
        tx.commit().await?;
    }
    Ok(report)
}
