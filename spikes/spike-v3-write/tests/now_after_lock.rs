//! O-28: a write is judged at the time it is allowed to run, not at the time it was received. A request that
//! waited for a row lock and so passed its deadline must not be admitted because `now` was read before the wait.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::time::Duration;

const DEFINITION: &str = "resource Member { fields { id: Id } }
actor Member
resource Task {
  fields { id: Id; due: Time; done: Bool }
  rows read when true
  transition close { allow actor != null and due >= now; from done = false; to done = true }
  expose apply close { target id; bulk maxRows 1 }
}";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_that_passed_while_waiting_for_the_row_lock_is_enforced() {
    let facts: Value = load_str(DEFINITION, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_now_after_lock_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let mut db = connect().await;
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.task(id,due,done) VALUES(1, clock_timestamp() + interval '700 milliseconds', false)"
    ))
    .await
    .unwrap();

    // Another session holds the row. The request reads its clock first, while the deadline is still ahead.
    let holder = connect().await;
    holder.batch_execute(&format!("BEGIN; SELECT id FROM {schema}.task WHERE id=1 FOR UPDATE")).await.unwrap();
    let now: String = db.query_one("SELECT clock_timestamp()::text", &[]).await.unwrap().get(0);
    let caller = Caller { actor_id: Some(1), now };
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1300)).await;
        holder.batch_execute("COMMIT").await.unwrap();
    });
    let knobs = Knobs { fresh_now_after_lock: true, ..Knobs::default() };
    let outcome = apply(&mut db, &facts, &json!({"apply":"Task.close","target":{"ids":["1"]}}), &caller, &knobs).await;
    release.await.unwrap();
    let done: bool = db.query_one(&format!("SELECT done FROM {schema}.task WHERE id=1"), &[]).await.unwrap().get(0);

    // A fixed clock far in the past keeps deciding when the knob is off, so fixed-time callers stay deterministic.
    db.batch_execute(&format!("UPDATE {schema}.task SET due='2026-10-14T00:00:00Z', done=false")).await.unwrap();
    let fixed = Caller { actor_id: Some(1), now: "2026-10-13T00:00:00Z".into() };
    let deterministic = apply(&mut db, &facts, &json!({"apply":"Task.close","target":{"ids":["1"]}}), &fixed, &Knobs::default()).await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();

    let rejected = outcome.expect_err("the deadline passed during the lock wait, so the write must be refused");
    assert_eq!(rejected.code, "WIRE.FORBIDDEN", "{rejected:?}");
    assert!(!done, "nothing was written");
    assert_eq!(deterministic.expect("a fixed caller clock decides").changed, vec![1]);
}
