//! Audit B (2026-11-16): opposite-order atomic bundles may deadlock; retry the fully rolled-back SQL transaction within
//! the existing bound, then require one winner and one business refusal. The recorded CI reproduction observed 40P01.
//! The sleep after each transition's row lock is only a scheduling hint; it does not prove the sessions overlap at that
//! point. `WIRE.CONFLICT` alone is not a deadlock signal: only V3's retryable SQL-conflict marker is counted below.
//! The public result does not retain the raw SQLSTATE, so PostgreSQL log evidence remains necessary for diagnosis.
//! Expected business result: one bundle wins both rows and the other receives `WIRE.INVALID_STATE` on the claimed row.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{bundle, Knobs};
use std::{sync::Arc, time::Duration};

const DB: &str = aip_test_support::test_database_url!();
const SPEC: &str = r#"
enum Status { OPEN, CLAIMED }
resource Member { fields { id: Id } }
actor Member
resource Task {
  fields { id: Id; owner: Member?; status: Status }
  rows read when true
  transition claim { from status = OPEN; to status = CLAIMED, owner = actor; allow true }
  expose apply claim { target id; bulk maxRows 5 }
}
"#;

fn claim(id: i64) -> Value {
    json!({"apply":"Task.claim","target":{"ids":[id.to_string()]}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opposite_order_bundles_retry_deadlocks_then_return_one_business_refusal() {
    let schema = format!("aip_bd_lock_order_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let facts = Arc::new(load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution);
    let db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); INSERT INTO {schema}.task(id,owner_id,status) VALUES (30,NULL,'OPEN'),(31,NULL,'OPEN')"
    ))
    .await
    .unwrap();

    let gate = Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = vec![];
    for (actor, order) in [(1i64, [30, 31]), (2, [31, 30])] {
        let (facts, gate) = (facts.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let mut db = connect_with_url(DB).await.unwrap();
            gate.wait().await;
            let request = json!({"atomic": order.iter().map(|id| claim(*id)).collect::<Vec<_>>()});
            let caller = Caller { actor_id: Some(actor), now: "2026-11-16T00:00:00Z".into() };
            // Best-effort schedule hint only; exact overlap must be confirmed from PostgreSQL lock/error evidence.
            let knobs = Knobs { pause_after_lock_ms: 100, ..Knobs::default() };
            let outcome = tokio::time::timeout(Duration::from_secs(10), bundle(&mut db, &facts, &request, &caller, &knobs))
                .await
                .expect("a bundle must finish, not hang");
            (
                actor,
                outcome.map(|_| ()).map_err(|reject| {
                    let retryable_sql_conflict = spike_v3_write::is_retryable(&reject);
                    (reject.code, retryable_sql_conflict)
                }),
            )
        }));
    }
    let mut outcomes = vec![];
    for task in tasks {
        outcomes.push(task.await.unwrap());
    }
    let owners: Vec<Option<i64>> =
        db.query(&format!("SELECT owner_id FROM {schema}.task ORDER BY id"), &[]).await.unwrap().iter().map(|row| row.get(0)).collect();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();

    assert!(outcomes.iter().all(|(_, outcome)| !matches!(outcome, Err((_, true)))), "a retryable SQL concurrency outcome was returned: {outcomes:?}");
    assert_eq!(outcomes.iter().filter(|(_, outcome)| outcome.is_ok()).count(), 1, "exactly one bundle must win: {outcomes:?}");
    assert_eq!(
        outcomes.iter().filter(|(_, outcome)| matches!(outcome, Err((code, false)) if *code == "WIRE.INVALID_STATE")).count(),
        1,
        "the losing bundle must be refused by the business state, not another error: {outcomes:?}"
    );
    assert!(owners[0].is_some() && owners[0] == owners[1], "the winner owns both rows and the loser changed nothing: {owners:?}");
}
