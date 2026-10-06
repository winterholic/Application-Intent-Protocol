//! End-to-end for `migration`: data migrations run once per database, in declaration order, each in one transaction,
//! recorded with the digest of the body that ran; an edited applied migration stops the start; two processes starting
//! together run each migration once.

use aip_runtime::{Pool, migrate};
use std::sync::Arc;

fn plan(src: &str) -> aip_plan::Program {
    let checked = aip_sema::pipeline::check_source(src);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("no errors");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    compiled.program
}

/// An empty database of this name, and a pool on it.
async fn fresh(db: &str) -> Pool {
    let admin_url = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());
    let admin = aip_runtime::pool(&admin_url, 1).expect("admin pool");
    let c = admin.get().await.expect("admin connection");
    c.execute(format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)").as_str(), &[]).await.expect("drop");
    c.execute(format!("CREATE DATABASE {db}").as_str(), &[]).await.expect("create");
    let url = admin_url.rsplit_once('/').map(|(base, _)| format!("{base}/{db}")).expect("url");
    aip_runtime::pool(&url, 16).expect("pool")
}

async fn sql(pool: &Pool, q: &str) {
    pool.get().await.expect("conn").batch_execute(q).await.expect("sql");
}

async fn num(pool: &Pool, q: &str) -> i64 {
    pool.get().await.expect("conn").query_one(q, &[]).await.expect("query").get(0)
}

async fn text(pool: &Pool, q: &str) -> String {
    pool.get().await.expect("conn").query_one(q, &[]).await.expect("query").get(0)
}

const BASE: &str = r#"
use postgres

entity Item {
  name: Text(1..50)
  label: Text(1..50)?
  n: Int = 0
  unique (name) else NAME_TAKEN
}

command AddItem(name: Text(1..50)) {
  allow public
  do { insert Item { name } }
}
"#;

fn with(migrations: &str) -> aip_plan::Program {
    plan(&format!("{BASE}\n{migrations}"))
}

const LABEL: &str = "migration first_label {\n  update Item i where i.label = null set label = \"x\"\n}\n";
const COUNT: &str = "migration then_count {\n  update Item i set n = 1\n}\n";

#[tokio::test]
async fn migrations_run_once_in_order_and_cannot_be_edited() {
    let pool = fresh("aip_e2e_migration_main").await;
    // the schema first, as an earlier deployment left it, with data the migration will meet
    let p0 = with("");
    assert!(migrate(&pool, &p0, true).await.expect("schema").migrations.is_empty());
    sql(&pool, "INSERT INTO item (name) VALUES ('a'), ('b')").await;

    // first start: the migration runs and is recorded with its digest
    let p1 = with(LABEL);
    let r = migrate(&pool, &p1, false).await.expect("first start");
    assert_eq!(r.migrations, ["first_label"]);
    assert_eq!(num(&pool, "SELECT count(*) FROM item WHERE label = 'x'").await, 2, "the migration reached every row");
    assert_eq!(text(&pool, "SELECT digest FROM _aip_migration WHERE name = 'first_label'").await, p1.migrations[0].digest);

    // restart: nothing runs again, even for a row that would match
    sql(&pool, "INSERT INTO item (name) VALUES ('c')").await;
    assert!(migrate(&pool, &p1, false).await.expect("restart").migrations.is_empty());
    assert_eq!(num(&pool, "SELECT count(*) FROM item WHERE label IS NULL").await, 1, "an applied migration is not run twice");

    // a comment and spacing are not part of the body: the digest is of the meaning
    let reformatted = with("// backfills the label\nmigration first_label   {\n\n  update Item i   where i.label = null   set label = \"x\"\n}\n");
    assert_eq!(reformatted.migrations[0].digest, p1.migrations[0].digest);
    assert!(migrate(&pool, &reformatted, false).await.expect("reformatted").migrations.is_empty());

    // the body of an applied migration is edited: the start fails, and a migration declared after it does not run
    let edited = with(
        "migration first_label {\n  update Item i where i.label = null set label = \"y\"\n}\nmigration then_count {\n  update Item i set n = 1\n}\n",
    );
    let err = migrate(&pool, &edited, false).await.err().expect("an edited applied migration stops the start").to_string();
    assert!(err.contains("AIP.MIGRATION.CHANGED") && err.contains("first_label"), "{err}");
    assert_eq!(num(&pool, "SELECT count(*) FROM item WHERE n = 1").await, 0, "nothing ran on a start that was refused");
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_migration").await, 1);

    // new migrations run in declaration order, each recorded
    let p2 = with(&format!("{LABEL}{COUNT}migration add_z {{\n  insert Item {{ name: \"z\" }}\n}}\n"));
    let r = migrate(&pool, &p2, false).await.expect("new migrations");
    assert_eq!(r.migrations, ["then_count", "add_z"]);
    assert_eq!(text(&pool, "SELECT string_agg(name, ',' ORDER BY applied_at) FROM _aip_migration").await, "first_label,then_count,add_z");
    assert_eq!(num(&pool, "SELECT count(*) FROM item WHERE n = 1").await, 3, "then_count touched the rows that existed");
    assert_eq!(num(&pool, "SELECT n FROM item WHERE name = 'z'").await, 0, "add_z ran after then_count, so its row was not touched");
}

#[tokio::test]
async fn a_failing_migration_rolls_back_and_stops_the_start() {
    let pool = fresh("aip_e2e_migration_fail").await;
    migrate(&pool, &with(""), true).await.expect("schema");
    // the second insert breaks `unique (name)`: the first one must not survive
    let bad = with(&format!("{LABEL}migration bad {{\n  insert Item {{ name: \"dup\" }}\n  insert Item {{ name: \"dup\" }}\n}}\n{COUNT}"));
    let err = migrate(&pool, &bad, false).await.err().expect("the start fails").to_string();
    assert!(err.contains("AIP.MIGRATION.FAILED") && err.contains("bad"), "{err}");
    assert_eq!(num(&pool, "SELECT count(*) FROM item WHERE name = 'dup'").await, 0, "the failed migration was rolled back");
    assert_eq!(
        text(&pool, "SELECT string_agg(name, ',') FROM _aip_migration").await,
        "first_label",
        "only what finished is recorded; later ones did not run"
    );

    // fixed, the same start goes on from where it stopped
    let fixed = with(&format!("{LABEL}migration bad {{\n  insert Item {{ name: \"dup\" }}\n}}\n{COUNT}"));
    let r = migrate(&pool, &fixed, false).await.expect("start after the fix");
    assert_eq!(r.migrations, ["bad", "then_count"]);
}

#[tokio::test]
async fn two_processes_starting_together_run_a_migration_once() {
    let pool = Arc::new(fresh("aip_e2e_migration_race").await);
    migrate(&pool, &with(""), true).await.expect("schema");
    // enough rows that the migration takes a while: both starters are inside it at the same moment unless something serialises them
    sql(&pool, "INSERT INTO item (name) SELECT 'row' || g FROM generate_series(1, 150000) g").await;
    let p = Arc::new(with("migration bump {\n  update Item i set n = i.n + 1\n}\n"));
    let start = |pool: Arc<Pool>, p: Arc<aip_plan::Program>| tokio::spawn(async move { migrate(&pool, &p, false).await.map(|r| r.migrations) });
    let (a, b) = (start(pool.clone(), p.clone()), start(pool.clone(), p.clone()));
    let (a, b) = (a.await.expect("join").expect("first starter"), b.await.expect("join").expect("second starter"));
    let mut ran: Vec<String> = a.into_iter().chain(b).collect();
    ran.sort();
    assert_eq!(ran, ["bump"], "exactly one of the two ran it");
    assert_eq!(num(&pool, "SELECT max(n) FROM item").await, 1, "no row went through the migration twice");
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_migration").await, 1);
}

#[tokio::test]
async fn a_migration_reaches_every_tenant_in_one_transaction() {
    // the saas example with its migration taken out: the schema as an earlier deployment had it; seeded by a cross tenant transaction
    let src = std::fs::read_to_string(format!("{}/../../examples/saas/app.aip", env!("CARGO_MANIFEST_DIR"))).expect("example");
    let at = src.find("\nmigration backfill_done_notes").expect("the example's migration");
    let (before, full) = (plan(&src[..at]), plan(&src));
    let pool = fresh("aip_e2e_migration_tenants").await;
    migrate(&pool, &before, true).await.expect("schema");
    sql(
        &pool,
        "BEGIN; SELECT set_config('aip.tenant_cross', 'on', true);
         INSERT INTO member (email) VALUES ('a@x.com');
         INSERT INTO workspace (name) VALUES ('A'), ('B');
         INSERT INTO project (workspace_id, title) SELECT id, 'P' || name FROM workspace;
         INSERT INTO task (project_id, title, status) SELECT id, 't', 'DONE' FROM project;
         COMMIT;",
    )
    .await;
    assert_eq!(num(&pool, "SELECT count(DISTINCT p.workspace_id) FROM task t JOIN project p ON p.id = t.project_id WHERE t.notes IS NULL").await, 2);
    // one statement rewrites the rows of both workspaces; the tenant pin does not stop a migration
    assert_eq!(migrate(&pool, &full, false).await.expect("migrates").migrations, ["backfill_done_notes"]);
    assert_eq!(num(&pool, "SELECT count(*) FROM task WHERE notes = 'completed'").await, 2);
}
