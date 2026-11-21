//! Consumer-app finding (consumer-apps 8.2): a bundle `[create OrderLine (FK to Product), take(Product)]` raced by several callers
//! deadlocks on the Product row (the FK insert takes FOR KEY SHARE first, then `take` wants FOR UPDATE), while `[take, create OrderLine]`
//! serializes cleanly. The server already retried 40P01 (up to 3 times), but every deadlock victim costs PostgreSQL's `deadlock_timeout`
//! (1s) and N racing callers need N-1 victims, so nothing converged inside the 2s write deadline (`WIRE.DEADLINE_EXCEEDED`).
//! The transition's row lock is now `FOR NO KEY UPDATE` unless it changes a key column or deletes, which does not conflict with the
//! `FOR KEY SHARE` an FK insert takes, so both orders converge: exactly the callers the stock can cover succeed, in milliseconds.
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
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow actor != null
    to stock = stock - input.qty
  }
  expose apply take { target id; bulk maxRows 5 }
}
resource Order {
  fields { id: Id; owner: Member }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner }
}
resource OrderLine {
  fields { id: Id; owner: Member; order: Order; product: Product; qty: Int(1..100) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, order, product, qty }
}
"#;

fn flow(actor: i64, product: i64, line_first: bool) -> Value {
    let order = json!({"create": "Order", "values": {"owner": actor.to_string()}});
    let line = json!({"create": "OrderLine", "values": {"owner": actor.to_string(), "product": product.to_string(), "qty": 2}, "refs": {"order": {"created": if line_first { 0 } else { 1 }}}});
    let take = json!({"apply": "Product.take", "target": {"ids": [product.to_string()]}, "input": {"qty": 2}});
    json!({"atomic": if line_first { vec![order, line, take] } else { vec![take, order, line] }})
}

async fn race(facts: Arc<Value>, requests: Vec<(i64, Value)>) -> Vec<(String, u128)> {
    let gate = Arc::new(tokio::sync::Barrier::new(requests.len()));
    let mut tasks = vec![];
    for (actor, req) in requests {
        let (facts, gate) = (facts.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = connect_with_url(DB).await.unwrap();
            let caller = Caller { actor_id: Some(actor), now: "2026-11-21T00:00:00Z".into() };
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn racing_bundles_converge_whether_the_line_or_the_deduction_comes_first() {
    let facts = Arc::new(load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution);
    sqlgen::set_schema(&format!("aip_v3_bundle_lock_order_{}", std::process::id()));
    let db = connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    let s = sqlgen::schema();
    let members: Vec<String> = (1..=20).map(|id| format!("({id})")).collect();
    db.batch_execute(&format!("INSERT INTO {s}.member (id) VALUES {}", members.join(", "))).await.unwrap();
    for (name, line_first) in [("take-first", false), ("line-first", true)] {
        for round in 0..3 {
            let product = 100 + round + if line_first { 10 } else { 0 };
            db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock) VALUES ({product}, 1, 4)")).await.unwrap();
            let outcomes = race(facts.clone(), (1..=6).map(|actor| (actor, flow(actor, product, line_first))).collect()).await;
            let ok = outcomes.iter().filter(|(o, _)| o == "OK").count();
            let stock: i64 = db.query_one(&format!("SELECT stock FROM {s}.product WHERE id = {product}"), &[]).await.unwrap().get(0);
            let summary: Vec<String> = outcomes.iter().map(|(o, ms)| format!("{o}@{ms}ms")).collect();
            // 재고 4, 2개짜리 6건: 정확히 2건이 성공하고 나머지는 업무 거절이다. 교착·기한 초과·충돌이 새지 않고 기한(2초)보다 훨씬 빨리 끝난다.
            assert_eq!((ok, stock), (2, 0), "{name} round {round}: {summary:?}");
            assert!(
                outcomes.iter().all(|(o, ms)| (o == "OK" || o.starts_with("WIRE.INVALID_STATE")) && *ms < 1000),
                "{name} round {round}: {summary:?}"
            );
            let lines: i64 = db.query_one(&format!("SELECT count(*) FROM {s}.order_line WHERE product_id = {product}"), &[]).await.unwrap().get(0);
            assert_eq!(lines, 2, "실패한 묶음의 줄은 남지 않는다");
        }
    }
}
