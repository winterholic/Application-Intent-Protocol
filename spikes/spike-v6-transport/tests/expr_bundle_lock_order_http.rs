//! Consumer-app finding over real HTTP: six callers race `[create Order, create OrderLine (FK to Product), take(Product)]` for stock 4 with
//! two units each. Before the row-lock fix every request ended `WIRE.DEADLINE_EXCEEDED` (the FK insert's KEY SHARE and `take`'s FOR UPDATE
//! deadlocked and each victim cost PostgreSQL's 1s `deadlock_timeout`). Now exactly two succeed in milliseconds, whatever the operation order.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when true
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
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

async fn post(address: SocketAddr, token: &str, fingerprint: &str, body: &Value) -> Value {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST /apply HTTP/1.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    let _ = timeout(Duration::from_secs(10), socket.read_to_string(&mut response)).await.unwrap();
    match response.split_once("\r\n\r\n") {
        Some((_, body)) if !body.is_empty() => serde_json::from_str(body).unwrap(),
        _ => json!({"dropped": true}),
    }
}

fn flow(key: &str, actor: i64, product: i64, line_first: bool) -> Value {
    let order = json!({"create": "Order", "values": {"owner": actor.to_string()}});
    let line = json!({"create": "OrderLine", "values": {"owner": actor.to_string(), "product": product.to_string(), "qty": 2}, "refs": {"order": {"created": if line_first { 0 } else { 1 }}}});
    let take = json!({"apply": "Product.take", "target": {"ids": [product.to_string()]}, "input": {"qty": 2}});
    json!({"key": key, "request": {"atomic": if line_first { vec![order, line, take] } else { vec![take, order, line] }}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn six_racing_orders_get_exactly_two_successes_in_either_operation_order() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_expr_bundle_lock_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = spike_v2_read::connect().await;
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    let members: Vec<String> = (1..=6).map(|id| format!("({id})")).collect();
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES {}", members.join(", "))).await.unwrap();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&facts, IdWire::DecimalString);
    let (port, keys) = spike_v6_transport::start_with_apply_contract(facts, IdWire::DecimalString).await;
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    for (name, line_first) in [("take-first", false), ("line-first", true)] {
        let product: i64 = if line_first { 2 } else { 1 };
        db.batch_execute(&format!("INSERT INTO {schema}.product(id,owner_id,stock) VALUES ({product},1,4)")).await.unwrap();
        let started = Instant::now();
        let calls: Vec<_> = (1..=6)
            .map(|actor| {
                let (token, fingerprint, body) =
                    (keys.issue(actor, 300), fingerprint.clone(), flow(&format!("{name}-{actor}"), actor, product, line_first));
                tokio::spawn(async move { post(address, &token, &fingerprint, &body).await })
            })
            .collect();
        let mut outcomes = vec![];
        for call in calls {
            let result = call.await.unwrap();
            outcomes.push(result["code"].as_str().unwrap_or(if result["ok"] == true { "OK" } else { "?" }).to_string());
        }
        outcomes.sort();
        assert_eq!(outcomes, ["OK", "OK", "WIRE.INVALID_STATE", "WIRE.INVALID_STATE", "WIRE.INVALID_STATE", "WIRE.INVALID_STATE"], "{name}");
        assert!(started.elapsed() < Duration::from_millis(1500), "{name} took {:?}", started.elapsed());
        let stock: i64 = db.query_one(&format!("SELECT stock FROM {schema}.product WHERE id = {product}"), &[]).await.unwrap().get(0);
        let lines: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.order_line WHERE product_id = {product}"), &[]).await.unwrap().get(0);
        assert_eq!((stock, lines), (0, 2), "{name}");
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
