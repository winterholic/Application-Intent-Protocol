//! LIMITS E9, the part `bundle_lock_order_compose_where` does not cover: bundles that name the same rows through `target.ids` in
//! different orders. Operations run in array order and each locks its target when it runs, so two bundles taking products 1,2 and 2,1
//! deadlocked; every victim cost `deadlock_timeout` (1s) and the 2s write deadline ended most races as `WIRE.DEADLINE_EXCEEDED`.
//! `lock_bundle_targets` now locks the named rows in `(resource, id)` order when the bundle starts. These tests pin both halves:
//! the deadlock is gone, and nothing a caller can observe about operation order, error order, visibility or rollback changed.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{bundle, Knobs};
use std::{sync::Arc, time::Instant};

const DB: &str = aip_test_support::test_database_url!();

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when true
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
  expose apply take { target id; bulk maxRows 5 }
}
resource Box {
  fields { id: Id; owner: Member; n: Int(0..1000) }
  rows read when owner = actor
  transition bump { from true; allow actor != null; to n = n + 1 }
  expose apply bump { target id; bulk maxRows 5 }
}
"#;

fn take(id: i64, qty: i64) -> Value {
    json!({"apply": "Product.take", "target": {"ids": [id.to_string()]}, "input": {"qty": qty}})
}

fn bump(id: i64) -> Value {
    json!({"apply": "Box.bump", "target": {"ids": [id.to_string()]}})
}

/// `sqlgen::set_schema` is process-wide, so the tests share one schema and run one at a time.
static SETUP: tokio::sync::OnceCell<(Arc<Value>, String)> = tokio::sync::OnceCell::const_new();
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn setup() -> (Arc<Value>, tokio_postgres::Client, String) {
    let (facts, s) = SETUP
        .get_or_init(|| async {
            let facts = Arc::new(load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution);
            sqlgen::set_schema(&format!("aip_v3_e9_opposite_{}", std::process::id()));
            let db = connect().await;
            for ddl in sqlgen::ddl(&facts).unwrap() {
                db.batch_execute(&ddl).await.unwrap();
            }
            let s = sqlgen::schema().to_string();
            db.batch_execute(&format!(
                "INSERT INTO {s}.member (id) VALUES (1),(2),(3),(4),(5),(6),(7),(8); \
                 INSERT INTO {s}.product (id, owner_id, stock) VALUES (1,1,1000),(2,1,1000),(3,1,1000),(4,1,1000),(5,1,1000),(6,1,0); \
                 INSERT INTO {s}.box (id, owner_id, n) VALUES (1, 1, 0), (2, 2, 0)"
            ))
            .await
            .unwrap();
            (facts, s)
        })
        .await
        .clone();
    (facts, connect().await, s)
}

async fn race(facts: Arc<Value>, requests: Vec<(i64, Value)>) -> Vec<(String, u128)> {
    let gate = Arc::new(tokio::sync::Barrier::new(requests.len()));
    let mut tasks = vec![];
    for (actor, req) in requests {
        let (facts, gate) = (facts.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = connect_with_url(DB).await.unwrap();
            let caller = Caller { actor_id: Some(actor), now: "2026-11-22T00:00:00Z".into() };
            gate.wait().await;
            let started = Instant::now();
            let result = bundle(&mut conn, &facts, &req, &caller, &Knobs::default()).await;
            (result.map(|_| "OK".to_string()).unwrap_or_else(|e| format!("{} {}", e.code, e.msg)), started.elapsed().as_millis())
        }));
    }
    let mut out = vec![];
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

async fn one(facts: &Value, actor: i64, ops: Vec<Value>) -> Result<(), (String, String)> {
    let mut conn = connect_with_url(DB).await.unwrap();
    let caller = Caller { actor_id: Some(actor), now: "2026-11-22T00:00:00Z".into() };
    bundle(&mut conn, facts, &json!({"atomic": ops}), &caller, &Knobs::default()).await.map(|_| ()).map_err(|e| (e.code.to_string(), e.msg))
}

async fn stock(db: &tokio_postgres::Client, s: &str, id: i64) -> i64 {
    db.query_one(&format!("SELECT stock::bigint FROM {s}.product WHERE id = {id}"), &[]).await.unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn bundles_taking_the_same_products_in_opposite_orders_do_not_deadlock() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let (facts, db, s) = setup().await;
    for round in 0..3 {
        // Callers alternate between 1,2,3,4 and 4,3,2,1; one caller takes only a middle pair.
        let requests: Vec<(i64, Value)> = (1..=8)
            .map(|actor| {
                let ids: Vec<i64> = match actor {
                    8 => vec![3, 2],
                    a if a % 2 == 0 => vec![1, 2, 3, 4],
                    _ => vec![4, 3, 2, 1],
                };
                (actor, json!({"atomic": ids.into_iter().map(|id| take(id, 1)).collect::<Vec<_>>()}))
            })
            .collect();
        let outcomes = race(facts.clone(), requests).await;
        let summary: Vec<String> = outcomes.iter().map(|(o, ms)| format!("{o}@{ms}ms")).collect();
        // One deadlock costs deadlock_timeout (1s), so a sub-second bound separates "ordered" from "deadlocked and retried".
        assert!(outcomes.iter().all(|(o, ms)| o == "OK" && *ms < 1000), "round {round}: {summary:?}");
        let expected = |id: i64| 1000 - (round + 1) * if id == 2 || id == 3 { 8 } else { 7 };
        for id in 1..=4 {
            assert_eq!(stock(&db, &s, id).await, expected(id), "round {round}: product {id} lost one unit per bundle that named it");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_failing_operation_still_decides_the_error_and_nothing_is_kept() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let (facts, db, s) = setup().await;
    // Product 6 has no stock and product 999 does not exist. Locking all named rows first must not change which one is reported.
    let invalid_first = one(&facts, 1, vec![take(5, 1), take(6, 1), take(999, 1)]).await.unwrap_err();
    assert_eq!(invalid_first.0, "WIRE.INVALID_STATE", "{invalid_first:?}");
    let missing_first = one(&facts, 1, vec![take(5, 1), take(999, 1), take(6, 1)]).await.unwrap_err();
    assert_eq!(missing_first.0, "WIRE.MISSING_TARGET", "{missing_first:?}");
    assert_eq!(stock(&db, &s, 5).await, 1000, "a failed bundle rolls back the operations that ran before the failure");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rows_the_caller_cannot_read_are_neither_locked_nor_waited_for() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let (facts, db, s) = setup().await;
    // Another transaction holds box 2, which actor 1 cannot read. A pre-lock without the row policy would wait here until the 2s write
    // deadline and then report DEADLINE_EXCEEDED; the policy-aware lock skips the row and the operation reports MISSING_TARGET at its turn.
    let holder = connect().await;
    holder.batch_execute(&format!("BEGIN; SELECT 1 FROM {s}.box WHERE id = 2 FOR UPDATE")).await.unwrap();
    let started = Instant::now();
    let refused = one(&facts, 1, vec![bump(1), bump(2)]).await.unwrap_err();
    let waited = started.elapsed().as_millis();
    holder.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(refused.0, "WIRE.MISSING_TARGET", "{refused:?}");
    assert!(waited < 700, "the bundle waited {waited}ms for a row it cannot see");
    let n: i64 = db.query_one(&format!("SELECT n::bigint FROM {s}.box WHERE id = 1"), &[]).await.unwrap().get(0);
    assert_eq!(n, 0, "the visible row's bump rolled back with the refused bundle");
}
