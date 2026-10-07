//! PostgreSQL repro of the specific InvenTree stale cancellation rule:
//! one order status changes, its single active allocation is released, and a stale
//! repeated cancel is rejected without repeating the notification effect.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = "host=localhost dbname=postgres";
const SOURCE: &str = r#"
enum OrderStatus { PENDING, CANCELLED }
enum AllocationStatus { ACTIVE, RELEASED }
resource Member { fields { id: Id; email: Email } }
actor Member
resource SalesOrder {
  fields { id: Id; owner: Member; status: OrderStatus }
  rows read when owner = actor
  transition cancel {
    allow owner = actor
    from status = PENDING
    to status = CANCELLED
    update Allocation where order = this.id and status = ACTIVE { status = RELEASED }
    notify owner "order.cancelled"
  }
  expose apply cancel { target id; bulk maxRows 1 }
}
resource Allocation {
  fields { id: Id; order: SalesOrder; status: AllocationStatus }
  unique order, id
}
"#;

fn facts() -> serde_json::Value {
    load_str(SOURCE, Form::A).unwrap_or_else(|diagnostics| panic!("InvenTree pattern rejected: {diagnostics:?}")).execution
}

fn schema_name() -> String {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_oss_cancel_{}_{}", std::process::id(), nonce)
}

#[tokio::test]
async fn cancel_releases_one_allocation_and_stale_retry_does_not_repeat_effect() {
    let schema = schema_name();
    sqlgen::set_schema(&schema);
    let facts = facts();
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,email) VALUES (1,'manager@example.test'); \
         INSERT INTO {schema}.sales_order(id,owner_id,status) VALUES (100,1,'PENDING'),(101,1,'PENDING'); \
         INSERT INTO {schema}.allocation(id,order_id,status) VALUES (200,100,'ACTIVE'),(201,101,'ACTIVE'),(202,101,'ACTIVE')"
    ))
    .await
    .expect("seed one order and its allocation");

    let caller = Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() };
    let request = json!({"apply":"SalesOrder.cancel","target":{"ids":["100"]}});
    let first = apply(&mut db, &facts, &request, &caller, &Knobs::default()).await.unwrap();
    assert_eq!(first.changed, [100]);

    let stale_retry = apply(&mut db, &facts, &request, &caller, &Knobs::default()).await;
    assert_eq!(stale_retry.unwrap_err().code, "INVALID_STATE");

    let state = db
        .query_one(
            &format!(
                "SELECT o.status, a.status, (SELECT count(*) FROM {schema}.aip_outbox WHERE topic='order.cancelled') \
             FROM {schema}.sales_order o JOIN {schema}.allocation a ON a.order_id=o.id WHERE o.id=100"
            ),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(state.get::<_, String>(0), "CANCELLED");
    assert_eq!(state.get::<_, String>(1), "RELEASED");
    assert_eq!(state.get::<_, i64>(2), 1, "stale retry must not repeat its outbox effect");

    // InvenTree supports releasing a set of allocations. This AIP effect currently
    // requires exactly one matching allocation per target order, so a multi-line order
    // is rejected atomically instead of being counted as supported behavior.
    let multi_line =
        apply(&mut db, &facts, &json!({"apply":"SalesOrder.cancel","target":{"ids":["101"]}}), &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(multi_line.code, "EFFECT_TARGET_MISSING");
    let untouched = db
        .query_one(
            &format!(
                "SELECT o.status, count(*) FILTER (WHERE a.status='ACTIVE'), \
                 (SELECT count(*) FROM {schema}.aip_outbox WHERE topic='order.cancelled') \
                 FROM {schema}.sales_order o JOIN {schema}.allocation a ON a.order_id=o.id \
                 WHERE o.id=101 GROUP BY o.status"
            ),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(untouched.get::<_, String>(0), "PENDING");
    assert_eq!(untouched.get::<_, i64>(1), 2);
    assert_eq!(untouched.get::<_, i64>(2), 1);

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
