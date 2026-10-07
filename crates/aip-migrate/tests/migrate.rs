use aip_migrate::{apply, bind_wire, init, init_with_wire, plan, preflight};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_migrate_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}
impl Drop for OwnedSchema {
    fn drop(&mut self) {
        let name = self.0.clone();
        std::thread::spawn(move || {
            let mut command = std::process::Command::new("psql");
            let _ = command.args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {name} CASCADE")]).output();
        })
        .join()
        .ok();
    }
}

fn facts() -> serde_json::Value {
    load_str(include_str!("../../../prototype/example/app.aip"), Form::A).unwrap().execution
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_with_wire_persists_mode_in_creation_transaction() {
    let schema = OwnedSchema::new();
    init_with_wire(DB, &schema.0, &facts(), "safe").await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let stored: Option<String> = db.query_one(&format!("SELECT runtime_wire FROM {}.aip_migrate_meta", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(stored.as_deref(), Some("safe"));
    assert_eq!(bind_wire(DB, &schema.0, "safe").await.unwrap()["alreadyBound"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_init_wire_does_not_create_schema() {
    let schema = OwnedSchema::new();
    assert_eq!(init_with_wire(DB, &schema.0, &facts(), "legacy").await.unwrap_err()["code"], "BAD_WIRE_MODE");
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let missing: bool = db.query_one("SELECT to_regnamespace($1) IS NULL", &[&schema.0]).await.unwrap().get(0);
    assert!(missing);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_binding_is_stable_and_idempotent() {
    let schema = OwnedSchema::new();
    init(DB, &schema.0, &facts()).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.execute(
        &format!("INSERT INTO {}.aip_idem (principal,key,request,result) VALUES($1,'prior','request','{{}}'::jsonb)", schema.0),
        &[&"actor:42:ids:safe-number-v13"],
    )
    .await
    .unwrap();
    db.execute(
        &format!("INSERT INTO {}.aip_idem (principal,key,request,result) VALUES($1,'anonymous','request','{{}}'::jsonb)", schema.0),
        &[&"anonymous:ids:safe-number-v13"],
    )
    .await
    .unwrap();
    assert_eq!(bind_wire(DB, &schema.0, "safe").await.unwrap()["wire"], "safe");
    assert_eq!(bind_wire(DB, &schema.0, "safe").await.unwrap()["alreadyBound"], true);
    assert_eq!(bind_wire(DB, &schema.0, "decimal").await.unwrap_err()["code"], "WIRE_MODE_MISMATCH");
    assert_eq!(bind_wire(DB, &schema.0, "other").await.unwrap_err()["code"], "BAD_WIRE_MODE");
    preflight(DB, &schema.0, &facts()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_wire_binding_accepts_only_one_mode() {
    let schema = OwnedSchema::new();
    init(DB, &schema.0, &facts()).await.unwrap();
    let (safe, decimal) = tokio::join!(bind_wire(DB, &schema.0, "safe"), bind_wire(DB, &schema.0, "decimal"));
    assert_ne!(safe.is_ok(), decimal.is_ok());
    assert_eq!(safe.err().or(decimal.err()).unwrap()["code"], "WIRE_MODE_MISMATCH");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_committed_idempotency_rejects_other_wire_or_unknown_principal() {
    for principal in ["actor:42:ids:decimal-string-v13", "actor:42:unknown"] {
        let schema = OwnedSchema::new();
        init(DB, &schema.0, &facts()).await.unwrap();
        let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        db.execute(
            &format!("INSERT INTO {}.aip_idem (principal,key,request,result) VALUES($1,'prior','request','{{}}'::jsonb)", schema.0),
            &[&principal],
        )
        .await
        .unwrap();
        assert_eq!(bind_wire(DB, &schema.0, "safe").await.unwrap_err()["code"], "WIRE_MODE_MISMATCH");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_schema_preserves_rows_across_reviewed_additive_change() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    preflight(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id) VALUES(42)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["nickname"] = json!({"ty":"Text?"});
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report:?}");
    assert!(report["steps"].as_array().is_some_and(|s| !s.is_empty()));
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
    let count: i64 = client.query_one(&format!("SELECT count(*) FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrecognized_migration_directive_is_rejected() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let invalid = Some(json!({"sql":"DROP TABLE member"}));
    assert_eq!(plan(DB, &schema.0, &before, &invalid).await.unwrap_err()["code"], "BAD_MIGRATION");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn required_field_uses_explicit_typed_backfill_and_plan_acknowledgement() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id) VALUES(42)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["label"] = json!({"ty":"Text"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"label","value":"kept"}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(report["requiresReview"], true);
    assert_eq!(apply(DB, &schema.0, &after, &migration, "wrong").await.unwrap_err()["code"], "PLAN_CHANGED");
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();
    let value: String = client.query_one(&format!("SELECT label FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, "kept");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integer_backfill_uses_typed_parameter() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id) VALUES(42)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"score","value":7}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();
    let value: i64 = client.query_one(&format!("SELECT score FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_type_change_preserves_convertible_rows() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["score"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id,score) VALUES(42,'7')", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int?"});
    let conversion = Some(json!({"convert":[{"resource":"Member","field":"score","to":"Int?"}]}));
    let report = plan(DB, &schema.0, &after, &conversion).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert_eq!(report["requiresReview"], true);
    apply(DB, &schema.0, &after, &conversion, report["digest"].as_str().unwrap()).await.unwrap();
    let value: i64 = client.query_one(&format!("SELECT score FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decimal_conversion_is_lossless_and_rejects_values_postgres_would_round() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["score"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id,score) VALUES(42,'12.30')", schema.0), &[]).await.unwrap();

    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Decimal<5,2>?"});
    let migration = Some(json!({"convert":[{"resource":"Member","field":"score","to":"Decimal<5,2>?"}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert!(report["steps"].as_array().unwrap().iter().any(|s| s["sql"].as_str().unwrap().contains("TYPE numeric(5,2)")));
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();
    let value: String = client.query_one(&format!("SELECT score::text FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, "12.30");

    let invalid_schema = OwnedSchema::new();
    init(DB, &invalid_schema.0, &before).await.unwrap();
    let (invalid_client, invalid_connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = invalid_connection.await;
    });
    invalid_client.execute(&format!("INSERT INTO {}.member(id,score) VALUES(43,'1.234')", invalid_schema.0), &[]).await.unwrap();
    let invalid_report = plan(DB, &invalid_schema.0, &after, &migration).await.unwrap();
    assert_eq!(invalid_report["blocked"], true, "{invalid_report}");
    assert!(invalid_report["steps"].as_array().unwrap().iter().all(|s| !s["sql"].as_str().unwrap().contains("TYPE numeric(5,2)")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decimal_backfill_uses_numeric_parameter_and_fixed_scale() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id) VALUES(42)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["ledger"] = json!({"ty":"Decimal<5,2>"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"ledger","value":"0.99"}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();
    let value: String = client.query_one(&format!("SELECT ledger::text FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, "0.99");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_text_range_change_enforces_check() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["name"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["name"] = json!({"ty":"Text?","range":[1,5]});
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_unique_index_change_enforces_new_constraint() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["code"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["unique"] = json!([["code"]]);
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reference_type_renames_physical_column_and_preserves_value() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["buddy"] = json!({"ty":"Int?"});
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id,buddy) VALUES(42,42)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["buddy"] = json!({"ty":"Ref<Member>?"});
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
    let value: i64 = client.query_one(&format!("SELECT buddy_id FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_references_follow_generator_creation_order() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Aardvark"] = json!({"fields":{"id":{"ty":"Id<Aardvark>"},"zed":{"ty":"Ref<Zed>?"}},"unique":[],"invariants":{}});
    after["resources"]["Zed"] = json!({"fields":{"id":{"ty":"Id<Zed>"}},"unique":[],"invariants":{}});
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drift_and_journal_tamper_stop_planning() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.batch_execute(&format!("ALTER TABLE {}.member ADD COLUMN rogue text", schema.0)).await.unwrap();
    assert_eq!(plan(DB, &schema.0, &before, &None).await.unwrap_err()["code"], "SCHEMA_MISMATCH");
    client.batch_execute(&format!("ALTER TABLE {}.member DROP COLUMN rogue", schema.0)).await.unwrap();
    client.batch_execute(&format!("UPDATE {}.aip_migrate_journal SET new_digest='bad'", schema.0)).await.unwrap();
    assert_eq!(plan(DB, &schema.0, &before, &None).await.unwrap_err()["code"], "SCHEMA_MISMATCH");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_backfill_and_noop_apply_are_effective() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id) VALUES(42)", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["nickname"] = json!({"ty":"Text?"});
    let migration = Some(json!({"backfill":[{"resource":"Member","field":"nickname","value":"hello"}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap();
    let value: String = client.query_one(&format!("SELECT nickname FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, "hello");
    let same = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(apply(DB, &schema.0, &after, &None, same["digest"].as_str().unwrap()).await.unwrap()["unchanged"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn principal_actor_binding_cannot_be_reassigned() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(&format!(
            "INSERT INTO {}.member(id) VALUES(42),(43); INSERT INTO {}.aip_principals(issuer,subject,actor_id) VALUES('issuer','subject',42)",
            schema.0, schema.0
        ))
        .await
        .unwrap();
    assert!(
        client
            .batch_execute(&format!("UPDATE {}.aip_principals SET actor_id=43 WHERE issuer='issuer' AND subject='subject'", schema.0))
            .await
            .is_err()
    );
    client
        .batch_execute(&format!("UPDATE {}.aip_principals SET enabled=false,min_iat=100 WHERE issuer='issuer' AND subject='subject'", schema.0))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_read_exposure_is_reported_as_breaking() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Event"]["exposeRead"]["select"].as_object_mut().unwrap().remove("title");
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert!(report["changes"].as_array().unwrap().iter().any(|change| change["class"] == "Breaking"), "{report}");
    assert_eq!(report["requiresReview"], true);
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_change_and_function_body_drift_are_rejected() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut changed_actor = before.clone();
    changed_actor["actor"] = json!("Event");
    let report = plan(DB, &schema.0, &changed_actor, &None).await.unwrap();
    assert_eq!(report["blocked"], true);
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(&format!(
            "CREATE OR REPLACE FUNCTION {}.aip_reject_principal_actor_change() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$",
            schema.0
        ))
        .await
        .unwrap();
    assert_eq!(preflight(DB, &schema.0, &before).await.unwrap_err()["code"], "SCHEMA_MISMATCH");
}

#[test]
fn generated_ddl_rejects_unvalidated_identifiers_and_enum_literals() {
    let mut invalid = facts();
    let member = invalid["resources"].as_object_mut().unwrap().remove("Member").unwrap();
    invalid["resources"]["Member;DROP"] = member;
    assert_eq!(aip_migrate::creation("aip_ownschema", &invalid).unwrap_err()["code"], "BAD_FACTS");
    let mut invalid = facts();
    invalid["enums"]["Phase"] = json!(["READY' ); DROP TABLE member; --"]);
    assert_eq!(aip_migrate::creation("aip_ownschema", &invalid).unwrap_err()["code"], "BAD_FACTS");
    let mut invalid = facts();
    invalid["predicates"]["evil"] = json!({"body":{"cmp":"=); DROP TABLE member; --"}});
    assert_eq!(aip_migrate::creation("aip_ownschema", &invalid).unwrap_err()["code"], "BAD_FACTS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enum_change_rebuilds_check_for_new_values() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["enums"]["Phase"] = json!(["DONE"]);
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_resource_drop_removes_only_owned_table() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"].as_object_mut().unwrap().remove("Event");
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert!(report["changes"].as_array().unwrap().iter().any(|c| c["class"] == "Destructive"));
    apply(DB, &schema.0, &after, &None, report["digest"].as_str().unwrap()).await.unwrap();
    preflight(DB, &schema.0, &after).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_conversion_rolls_back_ddl_and_journal() {
    let schema = OwnedSchema::new();
    let mut before = facts();
    before["resources"]["Member"]["fields"]["score"] = json!({"ty":"Text?"});
    init(DB, &schema.0, &before).await.unwrap();
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.execute(&format!("INSERT INTO {}.member(id,score) VALUES(42,'bad')", schema.0), &[]).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["score"] = json!({"ty":"Int?"});
    let migration = Some(json!({"convert":[{"resource":"Member","field":"score","to":"Int?"}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(apply(DB, &schema.0, &after, &migration, report["digest"].as_str().unwrap()).await.unwrap_err()["code"], "MIGRATION_FAILED");
    preflight(DB, &schema.0, &before).await.unwrap();
    let value: String = client.query_one(&format!("SELECT score FROM {}.member WHERE id=42", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(value, "bad");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applied_plan_retry_reads_journal_without_running_again() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["nickname"] = json!({"ty":"Text?"});
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    let digest = report["digest"].as_str().unwrap();
    apply(DB, &schema.0, &after, &None, digest).await.unwrap();
    assert_eq!(apply(DB, &schema.0, &after, &None, digest).await.unwrap()["alreadyApplied"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_apply_calls_write_one_deployment() {
    let schema = OwnedSchema::new();
    let before = facts();
    init(DB, &schema.0, &before).await.unwrap();
    let mut after = before.clone();
    after["resources"]["Member"]["fields"]["nickname"] = json!({"ty":"Text?"});
    let report = plan(DB, &schema.0, &after, &None).await.unwrap();
    let digest = report["digest"].as_str().unwrap();
    let (a, b) = tokio::join!(apply(DB, &schema.0, &after, &None, digest), apply(DB, &schema.0, &after, &None, digest));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a["alreadyApplied"], b["alreadyApplied"]);
    let (client, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let count: i64 = client.query_one(&format!("SELECT count(*) FROM {}.aip_migrate_journal", schema.0), &[]).await.unwrap().get(0);
    assert_eq!(count, 2);
}
