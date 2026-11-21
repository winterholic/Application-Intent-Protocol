//! Same deadlock family as `expr_bundle_lock_order`: a create policy that reads the referenced product (`unitPrice = product.price`)
//! takes `FOR SHARE` on that row, and a later operation that updates the same row asks for a conflicting lock. `bundle_reference_locks`
//! reserved the write lock only for a later `target.ids` apply. A later `compose` (ids known up front) and a later `target.where` apply
//! (ids found by the query) were missed, so N racing callers each held a shared lock while waiting for the update lock; every deadlock
//! victim costs `deadlock_timeout` (1s) and the 2s write deadline ended most of them as `WIRE.DEADLINE_EXCEEDED`.
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
  fields { id: Id; owner: Member; stock: Int(0..1000); price: Int(0..1000); seen: Bool }
  rows read when true
  transition mark { from true; allow actor != null; to seen = true }
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
  expose apply mark { target where; bulk maxRows 5 }
  expose apply take { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 5; transitions mark }
  expose read { select id, stock; filter id.eq; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource Order {
  fields { id: Id; owner: Member }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner }
}
resource OrderLine {
  fields { id: Id; owner: Member; order: Order; product: Product; qty: Int(1..100); unitPrice: Int(0..1000) }
  rows read when owner = actor
  expose create { allow owner = actor and unitPrice = product.price; fields owner, order, product, qty, unitPrice }
}
"#;

fn flow(actor: i64, product: i64, last: Value) -> Value {
    let order = json!({"create": "Order", "values": {"owner": actor.to_string()}});
    let line = json!({"create": "OrderLine", "values": {"owner": actor.to_string(), "product": product.to_string(), "qty": 1, "unitPrice": 10}, "refs": {"order": {"created": 0}}});
    json!({"atomic": [order, line, last]})
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
async fn a_line_before_a_compose_or_where_update_of_the_same_product_does_not_deadlock() {
    let facts = Arc::new(load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution);
    sqlgen::set_schema(&format!("aip_v3_bundle_lock_cw_{}", std::process::id()));
    let db = connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    let s = sqlgen::schema();
    let members: Vec<String> = (1..=20).map(|id| format!("({id})")).collect();
    db.batch_execute(&format!("INSERT INTO {s}.member (id) VALUES {}", members.join(", "))).await.unwrap();
    let compose = |product: i64| json!({"compose": "Product", "targets": {"ids": [product.to_string()]}, "steps": [{"transition": "mark"}]});
    let by_where = |product: i64| json!({"apply": "Product.mark", "target": {"where": [{"field": "id", "op": "eq", "value": product.to_string()}]}});
    for (name, last) in [("compose", &compose as &dyn Fn(i64) -> Value), ("where", &by_where)] {
        for round in 0..3 {
            let product = 100 + round + if name == "where" { 10 } else { 0 };
            db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock, price, seen) VALUES ({product}, 1, 4, 10, false)"))
                .await
                .unwrap();
            let outcomes = race(facts.clone(), (1..=6).map(|actor| (actor, flow(actor, product, last(product)))).collect()).await;
            let summary: Vec<String> = outcomes.iter().map(|(o, ms)| format!("{o}@{ms}ms")).collect();
            assert!(outcomes.iter().all(|(o, ms)| o == "OK" && *ms < 1000), "{name} round {round}: {summary:?}");
            let lines: i64 = db.query_one(&format!("SELECT count(*) FROM {s}.order_line WHERE product_id = {product}"), &[]).await.unwrap().get(0);
            assert_eq!(lines, 6, "{name} round {round}: every bundle commits its line");
        }
    }
}
