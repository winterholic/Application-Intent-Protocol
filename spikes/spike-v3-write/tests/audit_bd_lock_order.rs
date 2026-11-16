//! Audit B (2026-11-16): `docs/design/05-execution-model.md` 3.1 promises that every row lock of one command is taken
//! in one global order, so two AIP commands cannot wait on each other in a cycle. A single transition honours it (the judge
//! locks `ORDER BY t.id`), but an `atomic` bundle runs its operations in request order and each operation locks only its own
//! targets. Two bundles that touch the same two rows in opposite order therefore deadlock; PostgreSQL aborts one after
//! `deadlock_timeout` and the caller gets `WIRE.CONFLICT` for what is a plain business race.
//!
//! Expected (design): no cycle. One bundle wins both rows, the other is refused by the business rule (`INVALID_STATE`,
//! the row is already CLAIMED). Real PostgreSQL, one connection per bundle.
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
async fn atomic_bundles_touching_the_same_rows_in_opposite_order_do_not_deadlock() {
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
            // Hold each operation's locks for a moment so both bundles hold their first row before asking for the second.
            let knobs = Knobs { pause_after_lock_ms: 100, ..Knobs::default() };
            let outcome = tokio::time::timeout(Duration::from_secs(10), bundle(&mut db, &facts, &request, &caller, &knobs))
                .await
                .expect("a bundle must finish, not hang");
            (actor, outcome.map(|_| ()).map_err(|reject| reject.code))
        }));
    }
    let mut outcomes = vec![];
    for task in tasks {
        outcomes.push(task.await.unwrap());
    }
    let owners: Vec<Option<i64>> =
        db.query(&format!("SELECT owner_id FROM {schema}.task ORDER BY id"), &[]).await.unwrap().iter().map(|row| row.get(0)).collect();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();

    assert!(
        outcomes.iter().all(|(_, outcome)| !matches!(outcome, Err("WIRE.CONFLICT"))),
        "opposite-order bundles formed a lock cycle and PostgreSQL broke it: {outcomes:?}"
    );
    assert_eq!(outcomes.iter().filter(|(_, outcome)| outcome.is_ok()).count(), 1, "exactly one bundle must win: {outcomes:?}");
    assert!(owners[0].is_some() && owners[0] == owners[1], "the winner owns both rows and the loser changed nothing: {owners:?}");
}
