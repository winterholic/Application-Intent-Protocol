//! A definite PostgreSQL FK refusal must not be reported as an unsettled write.
//! Simulates deployment metadata such as aip_principals, outside the resource definition.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, compose, is_retryable, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();

#[tokio::test]
async fn immediate_and_deferred_foreign_keys_are_definite_refusals_on_all_write_paths() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let schema = format!("aip_v3_fk_refusal_{}_{}", std::process::id(), nonce);
    sqlgen::set_schema(&schema);
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource Parent {
  fields { id: Id; owner: Member }
  rows read when owner = actor
  transition purge {
    allow owner = actor
    from true
    delete many Child maxRows 10 where parent = this.id
    delete
  }
  expose apply purge { target id; bulk maxRows 1 }
  expose compose { bulk maxRows 1; transitions purge }
}
resource Child {
  fields { id: Id; parent: Parent }
  rows read when parent.owner = actor
}
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1); \
         INSERT INTO {schema}.parent(id, owner_id) VALUES (100,1); \
         INSERT INTO {schema}.child(id, parent_id) VALUES (300,100); \
         CREATE TABLE {schema}.binding(id bigint PRIMARY KEY, parent_id bigint NOT NULL, \
           CONSTRAINT retained_binding FOREIGN KEY(parent_id) REFERENCES {schema}.parent(id) DEFERRABLE INITIALLY IMMEDIATE); \
         INSERT INTO {schema}.binding(id,parent_id) VALUES (200,100)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-11-06T00:00:00Z".into() };
    let request = json!({"apply":"Parent.purge","target":{"ids":["100"]}});
    let mut failures = Vec::new();
    for deferred in [false, true] {
        let mode = if deferred { "DEFERRED" } else { "IMMEDIATE" };
        db.batch_execute(&format!("ALTER TABLE {schema}.binding ALTER CONSTRAINT retained_binding DEFERRABLE INITIALLY {mode}")).await.unwrap();
        for path in ["apply", "bundle", "compose"] {
            let result = match path {
                "apply" => apply(&mut db, &facts, &request, &caller, &Knobs::default()).await.map(|_| ()),
                "bundle" => bundle(&mut db, &facts, &json!({"atomic":[request.clone()]}), &caller, &Knobs::default()).await.map(|_| ()),
                _ => compose(
                    &mut db,
                    &facts,
                    &json!({"compose":"Parent","targets":{"ids":["100"]},"steps":[{"transition":"purge"}]}),
                    &caller,
                    &Knobs::default(),
                )
                .await
                .map(|_| ()),
            };
            match result {
                Err(ref refusal) if refusal.code == "WIRE.REFERENCE_CONFLICT" && !is_retryable(refusal) => {
                    assert!(!refusal.msg.contains("retained_binding"), "constraint names are private metadata");
                    assert!(!refusal.msg.contains(&schema), "deployment schema is private metadata");
                }
                other => failures.push(format!("{path}/{mode}: expected definite WIRE.REFERENCE_CONFLICT, got {other:?}")),
            }
            let parent_count: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.parent WHERE id=100"), &[]).await.unwrap().get(0);
            let binding_count: i64 =
                db.query_one(&format!("SELECT count(*) FROM {schema}.binding WHERE id=200 AND parent_id=100"), &[]).await.unwrap().get(0);
            assert_eq!((parent_count, binding_count), (1, 1), "{path}/{mode}: both rows survive rollback");
            let child_count: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.child WHERE id=300"), &[]).await.unwrap().get(0);
            assert_eq!(child_count, 1, "{path}/{mode}: the delete effect must roll back too");
        }
    }
    // Pure-delete compose must run declared effects before deleting its parent;
    // it must not render an empty UPDATE SET statement.
    db.batch_execute(&format!("DELETE FROM {schema}.binding WHERE id=200")).await.unwrap();
    let result =
        compose(&mut db, &facts, &json!({"compose":"Parent","targets":{"ids":["100"]},"steps":[{"transition":"purge"}]}), &caller, &Knobs::default())
            .await;
    assert!(result.is_ok(), "unreferenced compose deletion must commit: {result:?}");
    for table in ["parent", "child"] {
        let count: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.{table}"), &[]).await.unwrap().get(0);
        assert_eq!(count, 0, "compose deletion must remove both the parent and its declared effect rows");
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("; "));
}
