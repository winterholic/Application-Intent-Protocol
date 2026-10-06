//! Shared helpers for the end-to-end tests: compile an example, migrate a fresh
//! database, and call intents in-process.
#![allow(dead_code)]

use aip_runtime::engine::{Call, Engine, Reply};
use aip_runtime::error::AipError;
use aip_runtime::objects::ObjectStore;
use serde_json::{Value, json};
use std::sync::Arc;

/// The server secret of the engines the tests build: signs nothing real, derives webhook secrets.
pub const TEST_SECRET: &[u8] = b"e2e-server-secret-not-for-production";

/// Keys for the engines that need them, made here from random bytes: no key is written down anywhere. The second one
/// is only for tests that rotate.
pub fn random_key() -> String {
    use base64::Engine as _;
    let bytes: Vec<u8> = (0..2).flat_map(|_| uuid::Uuid::new_v4().into_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn test_keys() -> Arc<aip_runtime::crypto::Keys> {
    static KEYS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let spec = KEYS.get_or_init(|| format!("t1:{}", random_key()));
    Arc::new(aip_runtime::crypto::Keys::parse(spec).expect("test keys"))
}

/// Compiles source through the whole pipeline. `None` when the frontend reports
/// errors; otherwise the plan and the backend diagnostics as JSON (golden form).
pub fn compile_source(src: &str) -> Option<Value> {
    let (core, map) = aip_sema::pipeline::check_source(src).into_core()?;
    let compiled = aip_pg::compile(&core, &map);
    let diags: Vec<Value> = compiled
        .diagnostics
        .iter()
        .map(|d| json!({"severity": format!("{:?}", d.severity), "code": d.code, "message": d.message, "line": d.line, "col": d.col}))
        .collect();
    Some(json!({"diagnostics": diags, "program": compiled.program}))
}

pub async fn setup_app(app: &str, db: &str) -> Arc<Engine> {
    let path = format!("{}/../../examples/{app}/app.aip", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).expect("example source");
    let checked = aip_sema::pipeline::check_source(&src);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("no errors");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    setup_program(db, compiled.program).await
}

/// A fresh database for an already compiled program (tests may alter the plan first).
pub async fn setup_program(db: &str, program: aip_plan::Program) -> Arc<Engine> {
    setup_program_with(db, program, Default::default()).await
}

/// Like [`setup_program`], with the outbound webhook settings the test needs (the default refuses private addresses).
pub async fn setup_program_with(db: &str, program: aip_plan::Program, outbound: aip_runtime::outbound::Settings) -> Arc<Engine> {
    let admin_url = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());
    let admin = aip_runtime::pool(&admin_url, 1).expect("admin pool");
    let c = admin.get().await.expect("admin connection");
    let exists = c.query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&db]).await.expect("query");
    if exists.is_none() {
        c.execute(format!("CREATE DATABASE {db}").as_str(), &[]).await.expect("create db");
    }
    let url = admin_url.rsplit_once('/').map(|(base, _)| format!("{base}/{db}")).expect("url");
    let pool = aip_runtime::pool(&url, 16).expect("pool");
    aip_runtime::migrate(&pool, &program, true).await.expect("migrate");
    let dir = std::env::temp_dir().join("aip-e2e-objects");
    let keys = aip_runtime::crypto::uses_encryption(&program).then(test_keys);
    Arc::new(Engine { program: Arc::new(program), pool, objects: ObjectStore::new(dir), secret: TEST_SECRET.to_vec(), outbound, keys })
}

pub async fn call(e: &Engine, intent: &str, actor: Option<&str>, input: Value, key: Option<&str>) -> Result<Reply, AipError> {
    e.call(Call {
        intent: intent.into(),
        input,
        actor: actor.map(String::from),
        client: Some("test-client".into()),
        idempotency_key: key.map(String::from),
        ..Default::default()
    })
    .await
}

pub async fn ok(e: &Engine, intent: &str, actor: Option<&str>, input: Value, key: Option<&str>) -> Value {
    match call(e, intent, actor, input, key).await {
        Ok(r) => r.data,
        Err(err) => panic!("{intent} failed: {err}"),
    }
}

pub async fn fails(e: &Engine, intent: &str, actor: Option<&str>, input: Value, key: Option<&str>) -> AipError {
    match call(e, intent, actor, input, key).await {
        Ok(r) => panic!("{intent} should fail but returned {}", r.data),
        Err(err) => err,
    }
}

pub async fn sql_one(e: &Engine, q: &str) -> Value {
    let c = e.pool.get().await.expect("conn");
    let row = c.query_one(&format!("WITH x AS ({q}) SELECT to_jsonb(x) FROM x"), &[]).await.expect("sql");
    let v: Value = row.get(0);
    v.as_object().and_then(|o| o.values().next().cloned()).unwrap_or(Value::Null)
}

pub async fn drain(e: &Engine) {
    while aip_runtime::dispatch::tick(e).await.expect("dispatch") {}
}
