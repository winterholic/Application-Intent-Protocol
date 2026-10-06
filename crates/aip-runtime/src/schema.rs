//! Applying a program's schema to a database: the first deployment creates everything, later ones are planned from the
//! program recorded in `_aip_deployment` (see `aip_pg::evolve`) and applied in one transaction.
//!
//! A deployment is recorded in the same transaction as the change it describes, so the record always says what the
//! tables are. The change is planned by the caller (`Deployment::evolve`): this module only runs what the plan says,
//! after running the checks it attaches, and refuses a plan that carries rejections.

use aip_ir::codes;
use aip_plan::{DataCheck, EvolvePlan, EvolveStep, Program, StepClass};
use deadpool_postgres::{Object, Pool, Transaction};
use serde::Serialize;

/// What the caller knows about the program being deployed that the compiled plan does not carry.
pub struct Deployment<'a> {
    pub core: &'a aip_ir::Program,
    pub ddl_version: u32,
    /// Plans the change from a deployed program to this one.
    pub evolve: &'a (dyn Fn(&aip_ir::Program) -> EvolvePlan + Send + Sync),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    /// No application tables yet: everything is created.
    Fresh,
    /// The deployed program is this program, by the same generator.
    Unchanged,
    /// Tables without a deployment record that match the program: the record is written and the generated triggers refreshed.
    Adopt,
    /// Changes from the recorded program to this one.
    Evolve,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub step: usize,
    pub what: String,
    /// Rows that break the rule; `None` when the check cannot run before the change (it names something the change creates).
    pub violations: Option<i64>,
    pub sample: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanReport {
    pub kind: PlanKind,
    pub plan: EvolvePlan,
    pub checks: Vec<CheckResult>,
    /// Data migrations that would run after the schema change.
    pub pending_migrations: Vec<String>,
}

impl PlanReport {
    /// True when applying would be refused: a rejection, or rows that break a new rule.
    pub fn blocked(&self) -> bool {
        !self.plan.rejections.is_empty() || self.checks.iter().any(|c| c.violations.unwrap_or(0) > 0)
    }
}

pub(crate) fn ddl_step(sql: &str) -> EvolveStep {
    EvolveStep { sql: sql.to_string(), class: StepClass::Safe, why: "fresh database".into(), checks: Vec::new() }
}

struct Recorded {
    digest: String,
    ddl_version: u32,
    core: aip_ir::Program,
}

const CREATE_RECORD: &str = "CREATE TABLE IF NOT EXISTS \"_aip_deployment\" (\"id\" bigserial PRIMARY KEY, \"applied_at\" timestamptz NOT NULL DEFAULT now(), \"digest\" text NOT NULL, \"ddl_version\" int NOT NULL, \"ir\" text NOT NULL, \"steps\" jsonb NOT NULL)";

async fn recorded(client: &Object) -> anyhow::Result<Option<Recorded>> {
    let exists: bool = client.query_one("SELECT to_regclass('_aip_deployment') IS NOT NULL", &[]).await?.get(0);
    if !exists {
        return Ok(None);
    }
    let Some(row) = client.query_opt("SELECT \"digest\", \"ddl_version\", \"ir\" FROM \"_aip_deployment\" ORDER BY \"id\" DESC LIMIT 1", &[]).await?
    else {
        return Ok(None);
    };
    let (digest, ddl_version, ir): (String, i32, String) = (row.get(0), row.get(1), row.get(2));
    let core = serde_json::from_str(&ir)
        .map_err(|e| anyhow::anyhow!("the recorded program in _aip_deployment cannot be read ({e}); it was written by another version of aip"))?;
    Ok(Some(Recorded { digest, ddl_version: ddl_version as u32, core }))
}

/// The Core IR of the program the database was last deployed with.
pub async fn deployed_core(pool: &Pool) -> anyhow::Result<Option<aip_ir::Program>> {
    let client = pool.get().await?;
    Ok(recorded(&client).await?.map(|r| r.core))
}

async fn application_tables(client: &Object) -> anyhow::Result<i64> {
    Ok(client
        .query_one("SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name NOT LIKE '\\_aip\\_%'", &[])
        .await?
        .get(0))
}

/// Differences between the compiled schema and the live database, for a database that has no deployment record.
async fn drift(client: &Object, program: &Program) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for (name, e) in &program.entities {
        let cols: Vec<String> = client
            .query("SELECT column_name FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1", &[&e.table])
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        if cols.is_empty() {
            out.push(format!("table {} ({name}) is missing", e.table));
            continue;
        }
        for c in &e.columns {
            if !cols.contains(&c.column) {
                out.push(format!("column {}.{} ({name}.{}) is missing", e.table, c.column, c.field));
            }
        }
    }
    Ok(out)
}

fn db_message(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(d) => d.message().to_string(),
        None => e.to_string(),
    }
}

/// What the planner refused, as one error.
fn refusal(plan: &EvolvePlan) -> anyhow::Error {
    let mut text =
        format!("the program cannot be deployed to this database: {} change(s) need a decision, nothing was changed", plan.rejections.len());
    for r in &plan.rejections {
        text.push_str(&format!("\n  {}: {}\n    fix: {}", r.code, r.message, r.fix));
    }
    anyhow::anyhow!(text)
}

async fn run_check(tx: &Transaction<'_>, c: &DataCheck) -> anyhow::Result<()> {
    let n: i64 = tx
        .query_one(c.count_sql.as_str(), &[])
        .await
        .map_err(|e| anyhow::anyhow!("{}: {}: {}", codes::SCHEMA_FAILED, c.what, db_message(&e)))?
        .get(0);
    if n > 0 {
        let ids: Vec<String> = tx.query(c.sample_sql.as_str(), &[]).await?.iter().map(|r| r.get(0)).collect();
        anyhow::bail!(
            "{}: {} ({n} row(s), for example {}); nothing was changed. Fix those rows first, then deploy again",
            codes::SCHEMA_DATA_CONFLICT,
            c.what,
            ids.join(", ")
        );
    }
    Ok(())
}

async fn run_steps(tx: &Transaction<'_>, steps: &[EvolveStep]) -> anyhow::Result<()> {
    for s in steps {
        for c in &s.checks {
            run_check(tx, c).await?;
        }
        if s.sql.is_empty() {
            continue;
        }
        tx.batch_execute(&s.sql)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {}: {}\n  in: {}", codes::SCHEMA_FAILED, s.why, db_message(&e), s.sql.lines().next().unwrap_or("")))?;
    }
    Ok(())
}

async fn record(tx: &Transaction<'_>, d: &Deployment<'_>, steps: &[EvolveStep]) -> anyhow::Result<()> {
    let summary: Vec<serde_json::Value> = steps.iter().map(|s| serde_json::json!({"class": s.class, "why": s.why})).collect();
    tx.execute(
        "INSERT INTO \"_aip_deployment\" (\"digest\", \"ddl_version\", \"ir\", \"steps\") VALUES ($1, $2, $3, $4)",
        &[&aip_ir::digest(d.core), &(d.ddl_version as i32), &aip_ir::canonical_json(d.core), &serde_json::Value::Array(summary)],
    )
    .await?;
    Ok(())
}

pub struct Applied {
    pub kind: PlanKind,
    pub steps: Vec<EvolveStep>,
    pub notes: Vec<String>,
}

/// Brings the database to `program` under the migration lock the caller holds. Without a `Deployment` (no Core IR at hand)
/// it can only create a fresh database or confirm that an existing one matches.
pub(crate) async fn apply(
    client: &mut Object,
    program: &Program,
    deploy: Option<&Deployment<'_>>,
    reset: bool,
) -> anyhow::Result<(Applied, Vec<String>)> {
    if reset {
        client.batch_execute("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;").await?;
    }
    if deploy.is_some() {
        client.batch_execute(CREATE_RECORD).await?;
    }
    let existing = application_tables(client).await?;
    let last = if deploy.is_some() { recorded(client).await? } else { None };
    let mut drifted = Vec::new();
    let applied = match (existing, deploy) {
        (0, _) => {
            let steps: Vec<EvolveStep> = program.ddl.iter().map(|s| ddl_step(s)).collect();
            let tx = client.transaction().await?;
            run_steps(&tx, &steps).await?;
            if let Some(d) = deploy {
                record(&tx, d, &steps).await?;
            }
            tx.commit().await?;
            Applied { kind: PlanKind::Fresh, steps, notes: Vec::new() }
        }
        (_, None) => {
            drifted = drift(client, program).await?;
            Applied { kind: PlanKind::Unchanged, steps: Vec::new(), notes: Vec::new() }
        }
        (_, Some(d)) => match last {
            Some(l) if l.digest == aip_ir::digest(d.core) && l.ddl_version == d.ddl_version => {
                Applied { kind: PlanKind::Unchanged, steps: Vec::new(), notes: Vec::new() }
            }
            Some(l) => {
                let plan = (d.evolve)(&l.core);
                if !plan.rejections.is_empty() {
                    return Err(refusal(&plan));
                }
                let tx = client.transaction().await?;
                run_steps(&tx, &plan.steps).await?;
                record(&tx, d, &plan.steps).await?;
                tx.commit().await?;
                Applied { kind: PlanKind::Evolve, steps: plan.steps, notes: plan.notes }
            }
            None => {
                drifted = drift(client, program).await?;
                if !drifted.is_empty() {
                    return Err(anyhow::anyhow!(
                        "{}: the database has tables but no deployment record, and it does not match the program:\n  {}",
                        codes::SCHEMA_UNRECORDED,
                        drifted.join("\n  ")
                    ));
                }
                // the schema is what the program says: record it, and bring the generated functions and triggers up to date
                let plan = (d.evolve)(d.core);
                let tx = client.transaction().await?;
                run_steps(&tx, &plan.steps).await?;
                record(&tx, d, &plan.steps).await?;
                tx.commit().await?;
                Applied { kind: PlanKind::Adopt, steps: plan.steps, notes: plan.notes }
            }
        },
    };
    Ok((applied, drifted))
}

/// What `apply` would do, without changing anything: the checks are read-only queries, the rest is the plan itself.
pub async fn plan(pool: &Pool, program: &Program, d: &Deployment<'_>) -> anyhow::Result<PlanReport> {
    let client = pool.get().await?;
    let existing = application_tables(&client).await?;
    let last = recorded(&client).await?;
    let (kind, plan) = match (existing, last) {
        (0, _) => (PlanKind::Fresh, EvolvePlan { steps: program.ddl.iter().map(|s| ddl_step(s)).collect(), ..Default::default() }),
        (_, Some(l)) if l.digest == aip_ir::digest(d.core) && l.ddl_version == d.ddl_version => (PlanKind::Unchanged, EvolvePlan::default()),
        (_, Some(l)) => (PlanKind::Evolve, (d.evolve)(&l.core)),
        (_, None) => {
            let drifted = drift(&client, program).await?;
            if drifted.is_empty() {
                (PlanKind::Adopt, (d.evolve)(d.core))
            } else {
                let rejection = aip_plan::Rejection {
                    code: codes::SCHEMA_UNRECORDED.into(),
                    message: format!("the database has tables but no deployment record, and it does not match the program: {}", drifted.join("; ")),
                    fix: "use `aip migrate --reset` on a development database, or bring the schema in line by hand once".into(),
                };
                (PlanKind::Adopt, EvolvePlan { rejections: vec![rejection], ..Default::default() })
            }
        }
    };
    let mut checks = Vec::new();
    for (i, s) in plan.steps.iter().enumerate() {
        for c in &s.checks {
            // each check on its own: one that names something the change creates fails, and must not stop the others
            match client.query_one(c.count_sql.as_str(), &[]).await {
                Ok(row) => {
                    let n: i64 = row.get(0);
                    let sample = if n > 0 { client.query(c.sample_sql.as_str(), &[]).await?.iter().map(|r| r.get(0)).collect() } else { Vec::new() };
                    checks.push(CheckResult { step: i, what: c.what.clone(), violations: Some(n), sample });
                }
                Err(_) => checks.push(CheckResult { step: i, what: c.what.clone(), violations: None, sample: Vec::new() }),
            }
        }
    }
    let mut pending = Vec::new();
    let has_table: bool = client.query_one("SELECT to_regclass('_aip_migration') IS NOT NULL", &[]).await?.get(0);
    let done: Vec<String> =
        if has_table { client.query("SELECT \"name\" FROM \"_aip_migration\"", &[]).await?.iter().map(|r| r.get(0)).collect() } else { Vec::new() };
    for m in &program.migrations {
        if !done.contains(&m.name) {
            pending.push(m.name.clone());
        }
    }
    Ok(PlanReport { kind, plan, checks, pending_migrations: pending })
}
