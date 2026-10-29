use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();
const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource SalesOrder {
  fields { id: Id; owner: Member; refreshed: Bool; created: Time? }
  transition refresh {
    allow owner = actor
    from refreshed = false
    to refreshed = true
    update Summary where order = this.id {
      label = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.label
      created = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.created otherwise this.created
    }
  }
  expose apply refresh { target id; bulk maxRows 1 }
}
resource Summary { fields { id: Id; order: SalesOrder; label: Text?; created: Time? } }
resource Item { fields { id: Id; order: SalesOrder; active: Bool; label: Text; created: Time? } }
"#;

fn schema() -> String {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_latest_effect_{}_{}", std::process::id(), nonce)
}

#[tokio::test]
async fn latest_effect_reads_latest_row_and_uses_fallback_in_postgres() {
    let schema = schema();
    sqlgen::set_schema(&schema);
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1); \
         INSERT INTO {schema}.sales_order(id,owner_id,refreshed,created) VALUES \
         (100,1,false,'2025-03-01T00:00:00Z'),(101,1,false,'2025-03-02T00:00:00Z'); \
         INSERT INTO {schema}.summary(id,order_id) VALUES (300,100),(301,101); \
         INSERT INTO {schema}.item(id,order_id,active,label,created) VALUES \
         (200,100,true,'older','2025-01-01T00:00:00Z'), \
         (201,100,true,'same-time-lower-id','2025-02-01T00:00:00Z'), \
         (202,100,true,'same-time-higher-id','2025-02-01T00:00:00Z'), \
         (203,100,true,'null-date',NULL)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-30T00:00:00Z".into() };
    for id in [100, 101] {
        let request = json!({"apply":"SalesOrder.refresh","target":{"ids":[id.to_string()]}});
        let result = apply(&mut db, &facts, &request, &caller, &Knobs::default()).await.unwrap();
        assert_eq!(result.changed, [id]);
    }

    let selected = db.query_one(&format!("SELECT label, created::text FROM {schema}.summary WHERE id=300"), &[]).await.unwrap();
    assert_eq!(selected.get::<_, String>(0), "same-time-higher-id");
    assert_eq!(selected.get::<_, String>(1), "2025-02-01 00:00:00+00");
    let fallback = db.query_one(&format!("SELECT label, created::text FROM {schema}.summary WHERE id=301"), &[]).await.unwrap();
    assert!(fallback.get::<_, Option<String>>(0).is_none());
    assert_eq!(fallback.get::<_, String>(1), "2025-03-02 00:00:00+00");

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
