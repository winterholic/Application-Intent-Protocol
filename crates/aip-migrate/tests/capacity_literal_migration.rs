use aip_migrate::{init, plan};
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_capacity_literal_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}
impl Drop for OwnedSchema {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let _ = std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
    }
}

fn facts(source: &str) -> Value {
    load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

#[tokio::test]
async fn predicate_literal_and_new_field_backfill_use_distinct_query_parameters() {
    let schema = OwnedSchema::new();
    let before = facts("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; owner: Member; label: Text } }");
    let after = facts(
        r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where label = "warm"
resource Item { fields { id: Id; owner: Member; label: Text; note: Text? } invariant maxItems per owner }
"#,
    );
    init(DB, &schema.0, &before).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.batch_execute(&format!(
        "INSERT INTO {}.member(id) VALUES(1); INSERT INTO {}.item(id,owner_id,label) VALUES(1,1,'warm'),(2,1,'warm'),(3,1,'warm')",
        schema.0, schema.0
    ))
    .await
    .unwrap();

    let migration = Some(json!({"backfill":[{"resource":"Item","field":"note","value":"cold"}]}));
    let report = plan(DB, &schema.0, &after, &migration).await.unwrap();
    assert_eq!(report["blocked"], true, "predicate and backfill literals collided: {report}");
    assert!(
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "Blocked" && change["reason"].as_str().unwrap_or("").contains("maxItems") }),
        "{report}"
    );
}
