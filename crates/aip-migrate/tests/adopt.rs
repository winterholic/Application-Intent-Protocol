use aip_migrate::{adopt, creation, preflight};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    panic::AssertUnwindSafe,
    time::{SystemTime, UNIX_EPOCH},
};

const DB: &str = "host=localhost dbname=postgres";
fn facts() -> Value {
    spike_v1_fixture::load_str(include_str!("../../../prototype/example/app.aip"), spike_v1_fixture::Form::A).unwrap().execution
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adoption_preserves_data_and_committed_idempotency() {
    let schema = format!("aip_adopt_{}_{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
    let facts = facts();
    let mut db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let tx = db.transaction().await.unwrap();
    let (mut statements, _) = creation(&schema, &facts).unwrap();
    statements.push(format!("CREATE TABLE {schema}.aip_idem (principal text NOT NULL, key text, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal, key))"));
    for sql in &statements {
        tx.batch_execute(sql).await.unwrap();
    }
    tx.batch_execute(&format!("CREATE TABLE {schema}.aip_proto_meta (singleton boolean PRIMARY KEY CHECK(singleton),ddl_digest text NOT NULL,structure_digest text NOT NULL)")).await.unwrap();
    tx.batch_execute("SET LOCAL search_path TO pg_catalog").await.unwrap();
    let catalog: String = tx.query_one(include_str!("../../../prototype/src/schema_catalog.sql"), &[&schema]).await.unwrap().get(0);
    let catalog: Value = serde_json::from_str(&catalog).unwrap();
    let ddl = spike_v1_fixture::digest(&json!(statements));
    let structure = spike_v1_fixture::digest(&json!({"format":"prototype-catalog-v1","catalog":catalog}));
    tx.execute(&format!("INSERT INTO {schema}.aip_proto_meta VALUES(true,$1,$2)"), &[&ddl, &structure]).await.unwrap();
    tx.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(42); INSERT INTO {schema}.aip_idem VALUES('actor:42:ids:decimal-string-v13','saved','request','{{\"saved\":true}}')"
    ))
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let outcome = AssertUnwindSafe(async {
        db.batch_execute(&format!("ALTER TABLE {schema}.member ADD COLUMN unexpected text")).await.unwrap();
        assert_eq!(adopt::plan(DB, &schema, &facts).await.unwrap_err()["code"], "SCHEMA_MISMATCH");
        db.batch_execute(&format!("ALTER TABLE {schema}.member DROP COLUMN unexpected")).await.unwrap();
        assert_eq!(adopt::plan_with_wire(DB, &schema, &facts, "safe").await.unwrap_err()["code"], "WIRE_MODE_MISMATCH");
        assert!(db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{schema}.aip_proto_meta")]).await.unwrap().get::<_, bool>(0));
        let plan = adopt::plan_with_wire(DB, &schema, &facts, "decimal").await.unwrap();
        assert!(plan["digest"].as_str().is_some());
        assert_eq!(adopt::apply_with_wire(DB, &schema, &facts, "wrong-plan", "decimal").await.unwrap_err()["code"], "PLAN_CHANGED");
        assert_eq!(adopt::apply_with_wire(DB, &schema, &facts, plan["digest"].as_str().unwrap(), "decimal").await.unwrap()["ok"], true);
        let repeated = adopt::apply_with_wire(DB, &schema, &facts, plan["digest"].as_str().unwrap(), "decimal").await.unwrap();
        assert_eq!(repeated["alreadyApplied"], true);
        let wire: String = db.query_one(&format!("SELECT runtime_wire FROM {schema}.aip_migrate_meta"), &[]).await.unwrap().get(0);
        assert_eq!(wire, "decimal");
        assert_eq!(
            adopt::apply_with_wire(DB, &schema, &facts, plan["digest"].as_str().unwrap(), "safe").await.unwrap_err()["code"],
            "WIRE_MODE_MISMATCH"
        );
        assert_eq!(db.query_one(&format!("SELECT count(*) FROM {schema}.aip_migrate_journal"), &[]).await.unwrap().get::<_, i64>(0), 1);
        assert!(adopt::apply(DB, &schema, &facts, "different-plan").await.is_err());
        preflight(DB, &schema, &facts).await.unwrap();
        assert_eq!(db.query_one(&format!("SELECT id FROM {schema}.member"), &[]).await.unwrap().get::<_, i64>(0), 42);
        assert_eq!(
            db.query_one(&format!("SELECT result FROM {schema}.aip_idem WHERE key='saved'"), &[]).await.unwrap().get::<_, Value>(0),
            json!({"saved":true})
        );
        assert!(adopt::plan(DB, &schema, &facts).await.is_err(), "adoption must not overwrite a product deployment");
    })
    .catch_unwind()
    .await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
