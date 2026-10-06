//! Data migrations (`migration X { ... }`): each runs once per database, in declaration order, after the schema is in place.
//!
//! What ran is recorded in `_aip_migration` (name, digest of the body, time). The record is written in the same
//! transaction as the migration's changes, so a migration is either applied and recorded or neither. A name that is
//! recorded with a different digest stops the start: an applied migration cannot be edited, because the data already went
//! through the body that ran. A session advisory lock held by the starting process makes several processes starting
//! together run each migration once; the ones that wait find it recorded.

use crate::exec::{self, Env, ExecCtx};
use crate::objects::ObjectStore;
use aip_ir::codes;
use aip_plan::{Migration, Program};
use std::collections::HashMap;

/// Key of the advisory lock that serialises migrations of one database (the bytes of "aipmigra").
pub(crate) const LOCK_KEY: i64 = 0x6169_706d_6967_7261;

/// Runs the migrations of `program` that have not run yet and returns their names. The caller holds the migration lock.
pub(crate) async fn run_held(client: &mut deadpool_postgres::Object, program: &Program) -> anyhow::Result<Vec<String>> {
    if program.migrations.is_empty() {
        return Ok(Vec::new());
    }
    run_locked(client, program).await
}

async fn run_locked(client: &mut deadpool_postgres::Object, program: &Program) -> anyhow::Result<Vec<String>> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS \"_aip_migration\" (\"name\" text PRIMARY KEY, \"digest\" text NOT NULL, \"applied_at\" timestamptz NOT NULL DEFAULT now())",
        )
        .await?;
    let applied: HashMap<String, String> =
        client.query("SELECT \"name\", \"digest\" FROM \"_aip_migration\"", &[]).await?.iter().map(|r| (r.get(0), r.get(1))).collect();
    // all of them are compared before any new one runs, so an edited migration never leaves a half-applied start behind
    for m in &program.migrations {
        if let Some(was) = applied.get(&m.name)
            && *was != m.digest
        {
            anyhow::bail!(
                "{}: migration '{}' already ran with a different body ({was}, now {}); an applied migration cannot be changed, add a new migration instead",
                codes::MIGRATION_CHANGED,
                m.name,
                m.digest
            );
        }
    }
    let objects = ObjectStore::new(std::env::temp_dir().join("aip-migration-objects"));
    let mut ran = Vec::new();
    for m in program.migrations.iter().filter(|m| !applied.contains_key(&m.name)) {
        apply(client, program, &objects, m)
            .await
            .map_err(|e| anyhow::anyhow!("{}: migration '{}' failed and was rolled back: {}", codes::MIGRATION_FAILED, m.name, why(&e)))?;
        tracing::info!(migration = %m.name, "applied");
        ran.push(m.name.clone());
    }
    Ok(ran)
}

async fn apply(client: &mut deadpool_postgres::Object, program: &Program, objects: &ObjectStore, m: &Migration) -> anyhow::Result<()> {
    let tx = client.transaction().await?;
    let uploads = HashMap::new();
    let mut ctx = ExecCtx { program, intent: &m.name, uploads: &uploads, objects, staged: Vec::new(), partial: Vec::new(), has_actor: false, keys: None };
    let mut env = Env::default();
    exec::run_steps(&mut ctx, &*tx, &m.steps, &mut env).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    exec::settle_rules(&mut ctx, &*tx, &env).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    tx.execute("INSERT INTO \"_aip_migration\" (\"name\", \"digest\") VALUES ($1, $2)", &[&m.name, &m.digest]).await?;
    tx.commit().await?;
    Ok(())
}

/// The database's own words for a failure; `tokio_postgres::Error` alone says only "db error".
fn why(e: &anyhow::Error) -> String {
    match e.downcast_ref::<tokio_postgres::Error>().and_then(|x| x.as_db_error()) {
        Some(d) => d.message().to_string(),
        None => e.to_string(),
    }
}
