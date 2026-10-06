//! AIP runtime: executes a compiled `aip_plan::Program` against PostgreSQL and
//! serves it over HTTP. It builds no SQL of its own.

pub mod auth;
pub mod crypto;
pub mod dispatch;
pub mod dynamic;
pub mod engine;
pub mod error;
pub mod exec;
pub mod http;
pub mod jobs;
pub mod migration;
pub mod objects;
pub mod outbound;
pub mod rekey;
pub mod schedule;
pub mod schema;
pub mod subscribe;
pub mod validate;
pub mod webhook;

use aip_plan::Program;
pub use deadpool_postgres::Pool;
use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod};
pub use schema::{CheckResult, Deployment, PlanKind, PlanReport, deployed_core, plan};
use std::sync::Arc;

pub fn pool(database_url: &str, size: usize) -> anyhow::Result<Pool> {
    let cfg: tokio_postgres::Config = database_url.parse()?;
    let mgr = Manager::from_config(cfg, tokio_postgres::NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast });
    Ok(Pool::builder(mgr).max_size(size).build()?)
}

/// Applies the program's DDL, then the data migrations that have not run yet. `reset` drops everything first (development only).
/// Without a `Deployment` there is no recorded program to compare with, so the schema must be fresh or already match.
pub async fn migrate(pool: &Pool, program: &Program, reset: bool) -> anyhow::Result<MigrateReport> {
    migrate_with(pool, program, None, reset).await
}

/// Like [`migrate`], planning the change from the program recorded in `_aip_deployment` and recording this one.
/// Both `aip migrate` and `aip run` come through here, so a server never serves data a migration has not reached.
pub async fn migrate_deployed(pool: &Pool, program: &Program, deploy: &Deployment<'_>, reset: bool) -> anyhow::Result<MigrateReport> {
    migrate_with(pool, program, Some(deploy), reset).await
}

async fn migrate_with(pool: &Pool, program: &Program, deploy: Option<&Deployment<'_>>, reset: bool) -> anyhow::Result<MigrateReport> {
    let mut client = pool.get().await?;
    // one lock for the schema change and the data migrations: a second process starting together waits, then finds both done
    client.execute("SELECT pg_advisory_lock($1)", &[&migration::LOCK_KEY]).await?;
    let result = migrate_locked(&mut client, program, deploy, reset).await;
    // a pooled connection must not carry the lock to its next user
    let unlock = client.execute("SELECT pg_advisory_unlock($1)", &[&migration::LOCK_KEY]).await;
    let report = result?;
    unlock?;
    Ok(report)
}

async fn migrate_locked(
    client: &mut deadpool_postgres::Object,
    program: &Program,
    deploy: Option<&Deployment<'_>>,
    reset: bool,
) -> anyhow::Result<MigrateReport> {
    let (applied, drift) = schema::apply(client, program, deploy, reset).await?;
    let migrations = if drift.is_empty() { migration::run_held(client, program).await? } else { Vec::new() };
    Ok(MigrateReport { applied: applied.steps.len(), kind: applied.kind, steps: applied.steps, notes: applied.notes, drift, migrations })
}

pub struct MigrateReport {
    /// Statements the schema change ran (0 when the database already matched).
    pub applied: usize,
    pub kind: PlanKind,
    pub steps: Vec<aip_plan::EvolveStep>,
    pub notes: Vec<String>,
    /// Differences between the schema and a database that has no deployment record (only without a `Deployment`).
    pub drift: Vec<String>,
    /// Data migrations this call ran, in order.
    pub migrations: Vec<String>,
}

pub struct Options {
    pub database_url: String,
    pub port: u16,
    pub secret: Vec<u8>,
    pub dev_auth: bool,
    pub trusted_proxies: bool,
    pub object_dir: std::path::PathBuf,
    pub workers: bool,
    /// Deliver `outbound webhooks` to private, loopback and link-local addresses too. For development and tests only:
    /// by default such a URL is refused, which is what keeps a tenant from making the server call its own network.
    pub allow_private_webhook_targets: bool,
    /// Client contract served at `/aip/describe`, computed by the caller from Core IR.
    pub contract: serde_json::Value,
    /// From `AIP_ENCRYPTION_KEYS`, checked against the program by [`crypto::load`] before anything else starts.
    pub keys: Option<Arc<crypto::Keys>>,
}

pub async fn run(program: Program, opts: Options) -> anyhow::Result<()> {
    // refuse to serve encrypted fields without the keys, whoever built the options
    if crypto::uses_encryption(&program) && opts.keys.is_none() {
        anyhow::bail!("{}: the program has encrypted fields and no keys were given ({})", aip_ir::codes::ENCRYPTION_KEYS_MISSING, crypto::ENV_KEYS);
    }
    let pool = pool(&opts.database_url, 16)?;
    let engine = Arc::new(engine::Engine {
        program: Arc::new(program),
        pool,
        objects: objects::ObjectStore::new(opts.object_dir),
        secret: opts.secret.clone(),
        outbound: outbound::Settings { allow_private: opts.allow_private_webhook_targets },
        keys: opts.keys.clone(),
    });
    if opts.allow_private_webhook_targets {
        tracing::warn!("outbound webhooks may call private addresses: development only");
    }
    if opts.workers {
        if !engine.program.outbound.is_empty() {
            tokio::spawn(outbound::run(engine.clone()));
        }
        tokio::spawn(dispatch::run(engine.clone()));
        tokio::spawn(schedule::run(engine.clone()));
        tokio::spawn(jobs::run(engine.clone()));
    }
    let state =
        http::AppState::new(engine, Arc::new(opts.secret), opts.dev_auth, opts.trusted_proxies, Arc::new(opts.contract), &opts.database_url).await;
    http::serve(state, std::net::SocketAddr::from(([127, 0, 0, 1], opts.port))).await
}
