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
async fn conditional_update_effect_uses_prestate_and_skips_false_rows_in_bulk() {
    let schema = schema();
    sqlgen::set_schema(&schema);
    let source = r#"
resource Member { fields { id: Id; level: Int } }
actor Member
resource SalesOrder {
  fields { id: Id; owner: Member; refreshed: Bool; enabled: Bool }
  transition refresh {
    allow true
    from refreshed = false
    to refreshed = true, enabled = false
    update Summary when this.enabled = true and this.owner.level > 3 where order = this.id { refreshed = true }
  }
  expose apply refresh { target id; bulk maxRows 3 }
  expose compose { bulk maxRows 3; transitions refresh }
}
resource Summary { fields { id: Id; order: SalesOrder; refreshed: Bool } }
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,level) VALUES (1,4); \
         INSERT INTO {schema}.sales_order(id,owner_id,refreshed,enabled) VALUES \
         (100,1,false,true),(101,1,false,false),(102,1,false,true),(103,1,false,true),(104,1,false,false),(105,1,false,true),(106,1,false,false); \
         INSERT INTO {schema}.summary(id,order_id,refreshed) VALUES (300,100,false),(303,103,false),(305,105,false)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-30T00:00:00Z".into() };

    let request = json!({"apply":"SalesOrder.refresh","target":{"ids":["100","101"]}});
    let result = apply(&mut db, &facts, &request, &caller, &Knobs::default()).await.unwrap();
    assert_eq!(result.changed, [100, 101]);
    let parents = db.query(&format!("SELECT id, refreshed, enabled FROM {schema}.sales_order ORDER BY id"), &[]).await.unwrap();
    assert_eq!(
        parents.iter().map(|row| (row.get::<_, i64>(0), row.get::<_, bool>(1), row.get::<_, bool>(2))).collect::<Vec<_>>(),
        [
            (100, true, false),
            (101, true, false),
            (102, false, true),
            (103, false, true),
            (104, false, false),
            (105, false, true),
            (106, false, false)
        ]
    );
    let child = db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id=300"), &[]).await.unwrap();
    assert!(child.get::<_, bool>(0));

    let atomic = json!({"atomic":[{"apply":"SalesOrder.refresh","target":{"ids":["103","104"]}}]});
    let bundled = bundle(&mut db, &facts, &atomic, &caller, &Knobs::default()).await.unwrap();
    assert_eq!(bundled.applied[0].changed, [103, 104]);
    assert!(db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id=303"), &[]).await.unwrap().get::<_, bool>(0));

    let combined = json!({"compose":"SalesOrder","targets":{"ids":["105","106"]},"steps":[{"transition":"refresh"}]});
    let composed = compose(&mut db, &facts, &combined, &caller, &Knobs::default()).await.unwrap();
    assert_eq!(composed.applied[0].changed, [105, 106]);
    assert!(db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id=305"), &[]).await.unwrap().get::<_, bool>(0));

    let missing = json!({"apply":"SalesOrder.refresh","target":{"ids":["102"]}});
    let error = apply(&mut db, &facts, &missing, &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(error.code, "WIRE.EFFECT_TARGET_MISSING");
    let rolled_back = db.query_one(&format!("SELECT refreshed, enabled FROM {schema}.sales_order WHERE id=102"), &[]).await.unwrap();
    assert!(!rolled_back.get::<_, bool>(0));
    assert!(rolled_back.get::<_, bool>(1));

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
