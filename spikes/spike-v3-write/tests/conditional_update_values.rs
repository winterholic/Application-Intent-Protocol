use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();
fn schema() -> String {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_latest_effect_{}_{}", std::process::id(), nonce)
}

#[tokio::test]
async fn update_effect_copies_parent_values_from_transition_prestate() {
    let schema = schema();
    sqlgen::set_schema(&schema);
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource SalesOrder {
  fields { id: Id; owner: Member; enabled: Bool; refreshed: Bool }
  rows read when owner = actor
  transition refresh {
    allow owner = actor
    from refreshed = false
    to refreshed = true, enabled = false
    update Summary when this.enabled = true where order = this.id { refreshed = this.enabled }
  }
  expose apply refresh { target id; bulk maxRows 1 }
  expose compose { bulk maxRows 1; transitions refresh }
}
resource Summary {
  fields { id: Id; order: SalesOrder; refreshed: Bool }
  rows read when order.owner = actor
}
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1); \
         INSERT INTO {schema}.sales_order(id,owner_id,enabled,refreshed) VALUES (100,1,true,false),(101,1,true,false),(102,1,true,false); \
         INSERT INTO {schema}.summary(id,order_id,refreshed) VALUES (300,100,false),(301,101,false),(302,102,false)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-11-02T00:00:00Z".into() };

    apply(&mut db, &facts, &json!({"apply":"SalesOrder.refresh","target":{"ids":["100"]}}), &caller, &Knobs::default()).await.unwrap();

    assert!(db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id=300"), &[]).await.unwrap().get::<_, bool>(0));
    assert!(!db.query_one(&format!("SELECT enabled FROM {schema}.sales_order WHERE id=100"), &[]).await.unwrap().get::<_, bool>(0));

    bundle(&mut db, &facts, &json!({"atomic":[{"apply":"SalesOrder.refresh","target":{"ids":["101"]}}]}), &caller, &Knobs::default()).await.unwrap();
    assert!(db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id=301"), &[]).await.unwrap().get::<_, bool>(0));
    assert!(!db.query_one(&format!("SELECT enabled FROM {schema}.sales_order WHERE id=101"), &[]).await.unwrap().get::<_, bool>(0));

    compose(
        &mut db,
        &facts,
        &json!({"compose":"SalesOrder","targets":{"ids":["102"]},"steps":[{"transition":"refresh"}]}),
        &caller,
        &Knobs::default(),
    )
    .await
    .unwrap();
    assert!(db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id=302"), &[]).await.unwrap().get::<_, bool>(0));
    assert!(!db.query_one(&format!("SELECT enabled FROM {schema}.sales_order WHERE id=102"), &[]).await.unwrap().get::<_, bool>(0));

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
