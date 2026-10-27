//! Verified adopt: every stored cell must open under the deployment KEK before the
//! prototype marker is removed. Tampered cells fail the adopt with the marker intact.
use aip_crypto::provider::LocalProvider;
use aip_execution::{
    encryption::{CellLocation, DekStore, Scope},
    read_opener::StoreOpener,
};
use aip_migrate::{adopt, creation, creation_for_verified_adopt, verify::encrypted_columns};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    panic::AssertUnwindSafe,
    time::{SystemTime, UNIX_EPOCH},
};

const DB: &str = aip_test_support::test_database_url!();

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Project { fields { id: Id; name: Text(1..50) } }
resource Secret {
  fields {
    id: Id
    project: Project
    value: Text(1..100) encrypted(scope: project)
    note: Text(1..50)? encrypted(scope: service)
  }
}
"#;

fn provider() -> LocalProvider {
    LocalProvider::from_spec("k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=", "k1").unwrap()
}

fn facts() -> Value {
    spike_v1_fixture::load_str(SOURCE, spike_v1_fixture::Form::A).unwrap().execution
}

fn schema(tag: &str) -> String {
    format!("aip_adopt_cells_{tag}_{}_{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

#[test]
fn fenced_creation_still_refuses_encrypted_facts_and_owner_columns_are_not_verifiable() {
    let facts = facts();
    assert_eq!(creation("aip_enc_fixture", &facts).unwrap_err()["code"], "BAD_FACTS");
    assert!(creation_for_verified_adopt("aip_enc_fixture", &facts).is_ok());
    let owner = spike_v1_fixture::load_str(
        "actor Member resource Member {fields {id: Id}} resource Profile {fields {id: Id; owner: Member; bio: Text encrypted(scope: owner)}}",
        spike_v1_fixture::Form::A,
    )
    .unwrap()
    .execution;
    assert_eq!(encrypted_columns("aip_enc_fixture", &owner).unwrap_err()["code"], "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE");
}

async fn proto_setup(schema: &str, facts: &Value) {
    let mut db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let tx = db.transaction().await.unwrap();
    let (mut statements, _) = creation_for_verified_adopt(schema, facts).unwrap();
    statements.push(format!("CREATE TABLE {schema}.aip_idem (principal text NOT NULL, key text, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal, key))"));
    for sql in &statements {
        tx.batch_execute(sql).await.unwrap();
    }
    tx.batch_execute(&format!("CREATE TABLE {schema}.aip_proto_meta (singleton boolean PRIMARY KEY CHECK(singleton),ddl_digest text NOT NULL,structure_digest text NOT NULL)"))
        .await
        .unwrap();
    tx.batch_execute("SET LOCAL search_path TO pg_catalog").await.unwrap();
    let catalog: String = tx.query_one(include_str!("../../../prototype/src/schema_catalog.sql"), &[&schema]).await.unwrap().get(0);
    let catalog: Value = serde_json::from_str(&catalog).unwrap();
    let ddl = spike_v1_fixture::digest(&json!(statements));
    let structure = spike_v1_fixture::digest(&json!({"format":"prototype-catalog-v1","catalog":catalog}));
    tx.execute(&format!("INSERT INTO {schema}.aip_proto_meta VALUES(true,$1,$2)"), &[&ddl, &structure]).await.unwrap();
    tx.commit().await.unwrap();
}

async fn seal_cells(schema: &str, store: &DekStore, tamper: bool) {
    let kek = provider();
    let mut db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let mut budget = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let service = Scope::service();
    let project = Scope::reference("Project", "1").unwrap();
    let provision = db.transaction().await.unwrap();
    store.ensure_active_in(&provision, &service, &kek).await.unwrap();
    store.ensure_active_in(&provision, &project, &kek).await.unwrap();
    provision.commit().await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.project(id, name) VALUES(1, 'p1')")).await.unwrap();
    let mut reservation = store.reserve_for_write(&mut budget, &project, 2).await.unwrap();
    let mut service_reservation = store.reserve_for_write(&mut budget, &service, 1).await.unwrap();
    let tx = db.transaction().await.unwrap();
    let value100 =
        store.seal_in(&tx, &mut reservation, &CellLocation::new("Secret", "value", "100").unwrap(), b"project-value-100", &kek).await.unwrap();
    let note100 =
        store.seal_in(&tx, &mut service_reservation, &CellLocation::new("Secret", "note", "100").unwrap(), b"service-note-100", &kek).await.unwrap();
    let value101 =
        store.seal_in(&tx, &mut reservation, &CellLocation::new("Secret", "value", "101").unwrap(), b"project-value-101", &kek).await.unwrap();
    let stored101 = if tamper { "not-a-frame".to_string() } else { value101 };
    tx.execute(
        &format!("INSERT INTO {schema}.secret(id, project_id, value, note) VALUES(100, 1, $1, $2), (101, 1, $3, NULL)"),
        &[&value100, &note100, &stored101],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_adopt_opens_every_cell_before_removing_the_marker() {
    let schema = schema("ok");
    let facts = facts();
    let store = DekStore::new(&schema).unwrap();
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    db.batch_execute(&store.ddl()).await.unwrap();
    proto_setup(&schema, &facts).await;
    seal_cells(&schema, &store, false).await;
    let outcome = AssertUnwindSafe(async {
        // The default adopt path stays fenced for encrypted facts.
        assert_eq!(adopt::plan(DB, &schema, &facts).await.unwrap_err()["code"], "BAD_FACTS");
        let plan = adopt::plan_verified(DB, &schema, &facts).await.unwrap();
        assert_eq!(plan["plan"]["verifiedCells"], json!(true));
        assert_eq!(
            adopt::apply_verified(DB, &schema, &facts, "wrong-plan", &StoreOpener::new(store.clone(), provider())).await.unwrap_err()["code"],
            "PLAN_CHANGED"
        );
        let done =
            adopt::apply_verified(DB, &schema, &facts, plan["digest"].as_str().unwrap(), &StoreOpener::new(store.clone(), provider())).await.unwrap();
        assert_eq!(done["ok"], json!(true));
        assert_eq!(done["verifiedColumns"], json!(2));
        assert_eq!(done["verifiedCells"], json!(3));
        assert!(!db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{schema}.aip_proto_meta")]).await.unwrap().get::<_, bool>(0));
        let repeated =
            adopt::apply_verified(DB, &schema, &facts, plan["digest"].as_str().unwrap(), &StoreOpener::new(store.clone(), provider())).await.unwrap();
        assert_eq!(repeated["alreadyApplied"], json!(true));
    })
    .catch_unwind()
    .await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE; DROP SCHEMA {} CASCADE", store.infrastructure_schema())).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_adopt_refuses_tampered_cells_with_the_marker_intact() {
    let schema = schema("bad");
    let facts = facts();
    let store = DekStore::new(&schema).unwrap();
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    db.batch_execute(&store.ddl()).await.unwrap();
    proto_setup(&schema, &facts).await;
    seal_cells(&schema, &store, true).await;
    let outcome = AssertUnwindSafe(async {
        let plan = adopt::plan_verified(DB, &schema, &facts).await.unwrap();
        let error = adopt::apply_verified(DB, &schema, &facts, plan["digest"].as_str().unwrap(), &StoreOpener::new(store.clone(), provider()))
            .await
            .unwrap_err();
        assert_eq!(error["code"], "WIRE.ENCRYPTION_CORRUPTED");
        assert!(db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{schema}.aip_proto_meta")]).await.unwrap().get::<_, bool>(0));
    })
    .catch_unwind()
    .await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE; DROP SCHEMA {} CASCADE", store.infrastructure_schema())).await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
