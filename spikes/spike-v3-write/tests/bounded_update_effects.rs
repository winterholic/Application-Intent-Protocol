use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = "host=localhost dbname=postgres";
const SOURCE: &str = r#"
enum OrderStatus { OPEN, CANCELLED }
enum AllocationStatus { ACTIVE, RELEASED }
resource Member { fields { id: Id } }
actor Member
resource SalesOrder {
  fields { id: Id; owner: Member; status: OrderStatus }
  rows read when owner = actor
  transition cancel {
    from status = OPEN
    to status = CANCELLED
    allow owner = actor
    notify owner "order.cancelled"
    update many Allocation maxRows 3 where order = this.id and status = ACTIVE { status = RELEASED }
  }
  expose apply cancel { target id; bulk maxRows 10 }
  expose compose { bulk maxRows 10; transitions cancel }
}
limit maxReleased on Allocation = atMost 3 where status = RELEASED
resource Allocation {
  fields { id: Id; order: SalesOrder; status: AllocationStatus; note: Text? }
  rows read when false
  invariant maxReleased per order
  check releasedNeedsNote when status != RELEASED or note != null
}
"#;

fn facts() -> Value {
    load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

fn schema() -> String {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_bounded_effect_{}_{}", std::process::id(), nonce)
}

async fn setup(facts: &Value) -> (tokio_postgres::Client, String) {
    let schema = schema();
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.sales_order(id,owner_id,status) VALUES \
         (100,1,'OPEN'),(101,1,'OPEN'),(102,1,'OPEN'),(103,2,'OPEN'),(104,1,'OPEN'),(105,1,'OPEN'),(106,1,'OPEN'),(109,1,'OPEN'),(110,1,'OPEN'), \
         (111,1,'OPEN'),(112,1,'OPEN'),(113,1,'OPEN'),(120,1,'OPEN'); \
         INSERT INTO {schema}.allocation(id,order_id,status,note) VALUES \
         (200,100,'ACTIVE','ok'),(201,100,'ACTIVE','ok'),(202,101,'ACTIVE','ok'),(203,101,'ACTIVE','ok'), \
         (204,103,'ACTIVE','ok'),(205,104,'ACTIVE',NULL), \
         (210,105,'ACTIVE','ok'),(211,105,'ACTIVE','ok'),(212,105,'ACTIVE','ok'), \
         (220,106,'ACTIVE','ok'),(221,106,'ACTIVE','ok'),(222,106,'ACTIVE','ok'),(223,106,'ACTIVE','ok'), \
         (230,109,'ACTIVE','ok'),(231,109,'ACTIVE','ok'),(232,109,'RELEASED','ok'),(233,109,'RELEASED','ok'), \
         (240,111,'ACTIVE','ok'),(241,113,'ACTIVE','ok'),(250,120,'ACTIVE','ok'),(251,120,'ACTIVE','ok')"
    ))
    .await
    .unwrap();
    (db, schema)
}

fn caller(actor_id: i64) -> Caller {
    Caller { actor_id: Some(actor_id), now: "2026-10-07T00:00:00Z".into() }
}

async fn order_state(db: &tokio_postgres::Client, schema: &str, id: i64) -> (String, i64) {
    let row = db.query_one(
        &format!("SELECT o.status, count(*) FILTER (WHERE a.status='ACTIVE') FROM {schema}.sales_order o LEFT JOIN {schema}.allocation a ON a.order_id=o.id WHERE o.id=$1 GROUP BY o.id"),
        &[&id],
    ).await.unwrap();
    (row.get(0), row.get(1))
}

async fn cancel(
    db: &mut tokio_postgres::Client,
    facts: &Value,
    ids: &[&str],
    actor: i64,
) -> Result<spike_v3_write::Applied, spike_v2_read::plan::Reject> {
    apply(db, facts, &json!({"apply":"SalesOrder.cancel","target":{"ids":ids}}), &caller(actor), &Knobs::default()).await
}

async fn outbox_count(db: &tokio_postgres::Client, schema: &str) -> i64 {
    db.query_one(&format!("SELECT count(*) FROM {schema}.aip_outbox"), &[]).await.unwrap().get(0)
}

async fn wait_for_lock(db: &tokio_postgres::Client, pid: i32, statement: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let blocked: bool = db
                .query_one(
                    "SELECT coalesce(wait_event_type='Lock',false) AND strpos(query,$2)>0 FROM pg_stat_activity WHERE pid=$1",
                    &[&pid, &statement],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected DB statement waits for a conflicting lock");
}

#[tokio::test]
async fn bounded_effect_scenarios_are_atomic_scoped_and_bounded() {
    let facts = facts();
    let (mut db, schema) = setup(&facts).await;
    assert_eq!(cancel(&mut db, &facts, &["102"], 1).await.unwrap().changed, [102]);
    assert_eq!(order_state(&db, &schema, 102).await, ("CANCELLED".into(), 0));
    assert_eq!(outbox_count(&db, &schema).await, 1);

    let bulk = cancel(&mut db, &facts, &["101", "100"], 1).await.unwrap_err();
    assert_eq!(bulk.code, "EFFECT_TARGET_MISSING", "four children exceed global maxRows 3");
    for id in [100, 101] {
        assert_eq!(order_state(&db, &schema, id).await, ("OPEN".into(), 2));
    }
    assert_eq!(outbox_count(&db, &schema).await, 1, "prior notify effect rolls back too");

    assert_eq!(cancel(&mut db, &facts, &["100"], 1).await.unwrap().changed, [100]);
    assert_eq!(order_state(&db, &schema, 100).await, ("CANCELLED".into(), 0));
    assert_eq!(order_state(&db, &schema, 101).await, ("OPEN".into(), 2), "another parent is untouched");
    assert_eq!(cancel(&mut db, &facts, &["100"], 1).await.unwrap_err().code, "INVALID_STATE");
    assert_eq!(outbox_count(&db, &schema).await, 2, "stale retry produces no duplicate effect");

    assert_eq!(cancel(&mut db, &facts, &["103"], 1).await.unwrap_err().code, "MISSING_TARGET");
    assert_eq!(order_state(&db, &schema, 103).await, ("OPEN".into(), 1));
    assert_eq!(cancel(&mut db, &facts, &["104"], 1).await.unwrap_err().code, "CHECK_FAILED");
    assert_eq!(order_state(&db, &schema, 104).await, ("OPEN".into(), 1));
    assert_eq!(outbox_count(&db, &schema).await, 2);

    assert_eq!(cancel(&mut db, &facts, &["105"], 1).await.unwrap().changed, [105], "exact maxRows boundary succeeds");
    assert_eq!(order_state(&db, &schema, 105).await, ("CANCELLED".into(), 0));
    assert_eq!(cancel(&mut db, &facts, &["106"], 1).await.unwrap_err().code, "EFFECT_TARGET_MISSING");
    assert_eq!(order_state(&db, &schema, 106).await, ("OPEN".into(), 4));
    assert_eq!(cancel(&mut db, &facts, &["109"], 1).await.unwrap_err().code, "INVARIANT_VIOLATED");
    assert_eq!(order_state(&db, &schema, 109).await, ("OPEN".into(), 2));
    assert_eq!(outbox_count(&db, &schema).await, 3);

    // An insertion already holding the FK lock must commit before the parent is locked.
    // The effect's subsequent RC statement must include that newly committed child.
    let mut inserting = connect_with_url(DB).await.unwrap();
    let insert_tx = inserting.transaction().await.unwrap();
    insert_tx.execute(&format!("INSERT INTO {schema}.allocation(order_id,status,note) VALUES(110,'ACTIVE','ok')"), &[]).await.unwrap();
    let mut writer = connect_with_url(DB).await.unwrap();
    let pid: i32 = writer.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let concurrent_facts = facts.clone();
    let pending = tokio::spawn(async move { cancel(&mut writer, &concurrent_facts, &["110"], 1).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let blocked: bool =
                db.query_one("SELECT coalesce(wait_event_type='Lock',false) FROM pg_stat_activity WHERE pid=$1", &[&pid]).await.unwrap().get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("parent writer waits for the child insertion's FK lock");
    insert_tx.commit().await.unwrap();
    assert_eq!(pending.await.unwrap().unwrap().changed, [110]);
    assert_eq!(order_state(&db, &schema, 110).await, ("CANCELLED".into(), 0));
    assert_eq!(outbox_count(&db, &schema).await, 4);

    let mut moving = connect_with_url(DB).await.unwrap();
    let move_tx = moving.transaction().await.unwrap();
    move_tx.execute(&format!("UPDATE {schema}.allocation SET order_id=113 WHERE id=240"), &[]).await.unwrap();
    let mut writer = connect_with_url(DB).await.unwrap();
    let pid: i32 = writer.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let concurrent_facts = facts.clone();
    let pending = tokio::spawn(async move { cancel(&mut writer, &concurrent_facts, &["111"], 1).await });
    wait_for_lock(&db, pid, "SELECT u.id").await;
    move_tx.commit().await.unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(order_state(&db, &schema, 111).await, ("CANCELLED".into(), 0));
    let moved_status: String = db.query_one(&format!("SELECT status FROM {schema}.allocation WHERE id=240"), &[]).await.unwrap().get(0);
    assert_eq!(moved_status, "ACTIVE", "a child moved outside the scope before lock acquisition is rechecked");

    let mut guarding = connect_with_url(DB).await.unwrap();
    let parent_tx = guarding.transaction().await.unwrap();
    parent_tx.query_one(&format!("SELECT id FROM {schema}.sales_order WHERE id=112 FOR UPDATE"), &[]).await.unwrap();
    let mover = connect_with_url(DB).await.unwrap();
    let pid: i32 = mover.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let move_sql = format!("UPDATE {schema}.allocation SET order_id=112 WHERE id=241");
    let pending = tokio::spawn(async move { mover.execute(&move_sql, &[]).await });
    wait_for_lock(&db, pid, "UPDATE").await;
    parent_tx.rollback().await.unwrap();
    assert_eq!(pending.await.unwrap().unwrap(), 1, "moving into a locked parent waits for its FK lock");
    cancel(&mut db, &facts, &["112"], 1).await.unwrap();
    assert_eq!(order_state(&db, &schema, 112).await, ("CANCELLED".into(), 0));
    assert_eq!(outbox_count(&db, &schema).await, 6);
    let atomic = json!({"atomic":[
        {"apply":"SalesOrder.cancel","target":{"ids":["120"]}},
        {"apply":"SalesOrder.cancel","target":{"ids":["106"]}}
    ]});
    assert_eq!(bundle(&mut db, &facts, &atomic, &caller(1), &Knobs::default()).await.unwrap_err().code, "EFFECT_TARGET_MISSING");
    assert_eq!(order_state(&db, &schema, 120).await, ("OPEN".into(), 2), "a later bundle failure rolls back earlier parent and child updates");
    assert_eq!(outbox_count(&db, &schema).await, 6);
    let combined = json!({"compose":"SalesOrder","targets":{"ids":["120","101"]},"steps":[{"transition":"cancel"}]});
    assert_eq!(compose(&mut db, &facts, &combined, &caller(1), &Knobs::default()).await.unwrap_err().code, "EFFECT_TARGET_MISSING");
    for id in [120, 101] {
        assert_eq!(order_state(&db, &schema, id).await, ("OPEN".into(), 2));
    }
    assert_eq!(outbox_count(&db, &schema).await, 6);
    let single = json!({"compose":"SalesOrder","targets":{"ids":["120"]},"steps":[{"transition":"cancel"}]});
    let composed = compose(&mut db, &facts, &single, &caller(1), &Knobs::default()).await.unwrap();
    assert_eq!(composed.applied[0].changed, [120]);
    assert_eq!(order_state(&db, &schema, 120).await, ("CANCELLED".into(), 0));
    assert_eq!(outbox_count(&db, &schema).await, 7);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
