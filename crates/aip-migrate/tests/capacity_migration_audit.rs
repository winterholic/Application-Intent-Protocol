use aip_migrate::{init, plan};
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);

impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_capacity_audit_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}

impl Drop for OwnedSchema {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let _ = std::process::Command::new("psql")
            .args(["-X", "-q", "-d", "postgres", "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")])
            .output();
    }
}

fn facts(source: &str) -> Value {
    load_str(source, Form::A).unwrap().execution
}

#[tokio::test]
async fn new_resource_with_capacity_has_no_existing_groups_to_check() {
    let schema = OwnedSchema::new();
    let before = facts("resource Member { fields { id: Id } } actor Member");
    let after = facts(
        r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item { fields { id: Id; owner: Member; active: Bool } invariant maxItems per owner }
"#,
    );
    init(DB, &schema.0, &before).await.unwrap();
    let report = plan(DB, &schema.0, &after, &None).await;
    assert!(report.is_ok(), "new resource should plan: {report:?}");
}

#[tokio::test]
async fn new_nullable_group_column_has_no_existing_groups_to_check() {
    let schema = OwnedSchema::new();
    let before = facts("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; active: Bool } }");
    let after = facts(
        r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item { fields { id: Id; owner: Member?; active: Bool } invariant maxItems per owner }
"#,
    );
    init(DB, &schema.0, &before).await.unwrap();
    let report = plan(DB, &schema.0, &after, &None).await;
    assert!(report.is_ok(), "new nullable group column should plan: {report:?}");
}

#[tokio::test]
async fn new_nullable_condition_column_has_no_existing_qualifying_rows() {
    let schema = OwnedSchema::new();
    let before = facts("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; owner: Member } }");
    let after = facts(
        r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item { fields { id: Id; owner: Member; active: Bool? } invariant maxItems per owner }
"#,
    );
    init(DB, &schema.0, &before).await.unwrap();
    let report = plan(DB, &schema.0, &after, &None).await;
    assert!(report.is_ok(), "new nullable condition column should plan: {report:?}");
}

#[tokio::test]
async fn backfilled_new_group_column_is_checked_against_existing_rows() {
    let schema = OwnedSchema::new();
    let before = facts("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; active: Bool } }");
    let after = facts(
        r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item { fields { id: Id; owner: Member?; active: Bool } invariant maxItems per owner }
"#,
    );
    init(DB, &schema.0, &before).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.batch_execute(&format!(
        "INSERT INTO {}.member(id) VALUES(1); INSERT INTO {}.item(id,active) VALUES(1,true),(2,true),(3,true)",
        schema.0, schema.0
    ))
    .await
    .unwrap();

    let migration = Some(json!({"backfill":[{"resource":"Item","field":"owner","value":1}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(report["blocked"], true, "{report}");
    assert!(
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "Blocked" && change["reason"].as_str().unwrap_or("").contains("maxItems") }),
        "{report}"
    );
}

#[tokio::test]
async fn backfilled_new_condition_column_is_checked_against_existing_rows() {
    let schema = OwnedSchema::new();
    let before = facts("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; owner: Member } }");
    let after = facts(
        r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item { fields { id: Id; owner: Member; active: Bool? } invariant maxItems per owner }
"#,
    );
    init(DB, &schema.0, &before).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.batch_execute(&format!("INSERT INTO {}.member(id) VALUES(1); INSERT INTO {}.item(id,owner_id) VALUES(1,1),(2,1),(3,1)", schema.0, schema.0))
        .await
        .unwrap();

    let migration = Some(json!({"backfill":[{"resource":"Item","field":"active","value":true}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(report["blocked"], true, "{report}");
    assert!(
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "Blocked" && change["reason"].as_str().unwrap_or("").contains("maxItems") }),
        "{report}"
    );
}
