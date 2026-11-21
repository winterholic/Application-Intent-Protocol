//! P0-A over real HTTP: a bundle whose later operations name the id an earlier create made (`refs`, `target.created`).
//! The idempotency key covers the whole bundle and stores the request with the symbols as sent, so a resend replays the stored
//! result including the ids it made (test-plan A-S1, I-1, I-2, I-3, R-1, V-3 request side), also when the response was lost
//! after COMMIT and while the same key races itself.
use aip_test_fault_proxy::Fault;
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen};
use spike_v6_transport::server::listen_with_apply_contract;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const DATABASE: &str = aip_test_support::test_database_url!();

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when owner = actor
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow owner = actor
    to stock = stock - input.qty
  }
  expose apply take { target id; bulk maxRows 5 }
}
resource Order {
  fields { id: Id; owner: Member; status: Text(1..12) }
  rows read when owner = actor
  transition confirm { from status = "open"; allow owner = actor; to status = "confirmed" }
  expose create { allow owner = actor; fields owner, status }
  expose apply confirm { target id; bulk maxRows 5 }
}
resource OrderLine {
  fields { id: Id; owner: Member; order: Order; product: Product; qty: Int(1..100) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, order, product, qty }
}
"#;

async fn post_with(address: SocketAddr, token: &str, fingerprint: &str, body: &Value) -> Value {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!("POST /apply HTTP/1.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
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

/// 주문 생성 → 생성한 주문 id로 줄 생성 → input 수량으로 재고 차감 → 주문 확정.
fn flow(key: &str, product: i64, qty: i64) -> Value {
    json!({"key": key, "request": {"atomic": [
        {"create": "Order", "values": {"owner": "1", "status": "open"}},
        {"create": "OrderLine", "values": {"owner": "1", "product": product.to_string(), "qty": qty}, "refs": {"order": {"created": 0}}},
        {"apply": "Product.take", "target": {"ids": [product.to_string()]}, "input": {"qty": qty}},
        {"apply": "Order.confirm", "target": {"created": 0}}
    ]}})
}

async fn number(db: &tokio_postgres::Client, sql: String) -> i64 {
    db.query_one(&sql, &[]).await.unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bundle_with_refs_replays_its_created_ids_and_never_runs_twice() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_expr_refs_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = connect_owned_with_url(DATABASE).await.expect("run-owned PostgreSQL");
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.product(id,owner_id,stock) VALUES (1,1,10),(2,1,10),(3,1,10),(4,1,10),(5,1,10),(6,1,10)"
    ))
    .await
    .unwrap();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&facts, IdWire::DecimalString);
    let (port, keys) = spike_v6_transport::start_with_apply_contract(facts.clone(), IdWire::DecimalString).await;
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let token = keys.issue(1, 300);
    let post = |body: Value| {
        let (token, fingerprint) = (token.clone(), fingerprint.clone());
        async move { post_with(address, &token, &fingerprint, &body).await }
    };
    let stock = |id: i64| {
        let (db, schema) = (&db, &schema);
        async move { number(db, format!("SELECT stock FROM {schema}.product WHERE id = {id}")).await }
    };
    let rows = || async {
        (
            number(&db, format!("SELECT count(*) FROM {schema}.\"order\"")).await,
            number(&db, format!("SELECT count(*) FROM {schema}.order_line")).await,
            number(&db, format!("SELECT count(*) FROM {schema}.aip_idem")).await,
        )
    };

    // A-S1 over the wire: the order, its line and the confirmation commit together; `created[]` lists the two creates.
    let first = post(flow("f1", 1, 3)).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["created"].as_array().unwrap().iter().map(|c| c["resource"].as_str().unwrap()).collect::<Vec<_>>(), ["Order", "OrderLine"]);
    let order_id = first["created"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(stock(1).await, 7);
    assert_eq!(number(&db, format!("SELECT count(*) FROM {schema}.order_line WHERE order_id = {order_id}")).await, 1);
    assert_eq!(number(&db, format!("SELECT count(*) FROM {schema}.\"order\" WHERE id = {order_id} AND status = 'confirmed'")).await, 1);

    // I-1: the same key replays the stored result, the same created ids, and deducts nothing more.
    let before = rows().await;
    let resent = post(flow("f1", 1, 3)).await;
    assert_eq!((resent["ok"].clone(), resent["replayed"].clone()), (json!(true), json!(true)), "{resent}");
    assert_eq!(resent["created"], first["created"], "the replay carries the ids the first run made");
    assert_eq!((stock(1).await, rows().await), (7, before));

    // I-2: the same key with only the `refs` different (or the resolved id written as a value) is a different request.
    let mut other_refs = flow("f1", 1, 3);
    other_refs["request"]["atomic"][1]["refs"] = json!({"order": {"created": 1}});
    assert_eq!(post(other_refs).await["code"], "WIRE.IDEMPOTENCY_MISMATCH");
    let mut as_value = flow("f1", 1, 3);
    as_value["request"]["atomic"][1].as_object_mut().unwrap().remove("refs");
    as_value["request"]["atomic"][1]["values"]["order"] = json!(order_id);
    assert_eq!(post(as_value).await["code"], "WIRE.IDEMPOTENCY_MISMATCH");
    // ... and so is a changed `input` under the same key (P0-B inside the bundle).
    assert_eq!(post(flow("f1", 1, 4)).await["code"], "WIRE.IDEMPOTENCY_MISMATCH");
    assert_eq!((stock(1).await, rows().await), (7, before));

    // R-1: the third operation fails (stock 10 < 100): no order, no line, no idempotency record; the same key runs again afterwards.
    let before = rows().await;
    let refused = post(flow("r1", 5, 100)).await;
    assert_eq!(refused["code"], "WIRE.INVALID_STATE", "{refused}");
    assert_eq!((stock(5).await, rows().await), (10, before));
    let again = post(flow("r1", 5, 4)).await;
    assert_eq!(again["ok"], true, "{again}");
    assert!(again.get("replayed").is_none(), "the refused run stored nothing, so this is a fresh execution: {again}");

    // Refusals decided before the transaction come back as wire codes and store nothing.
    let before = rows().await;
    for (body, code) in [
        (json!({"key": "e1", "request": {"atomic": [
            {"create": "Order", "values": {"owner": "1", "status": "open"}},
            {"create": "OrderLine", "values": {"owner": "1", "product": "3", "qty": 1}, "refs": {"order": {"created": 7}}}]}}), "WIRE.BAD_VALUE"),
        (json!({"key": "e2", "request": {"atomic": [
            {"create": "Order", "values": {"owner": "1", "status": "open"}},
            {"create": "OrderLine", "values": {"owner": "1", "qty": 1}, "refs": {"product": {"created": 0}, "order": {"created": 0}}}]}}), "WIRE.TYPE_MISMATCH"),
        (json!({"key": "e3", "request": {"atomic": [
            {"apply": "Product.take", "target": {"ids": ["3"]}, "input": {"qty": 1}},
            {"apply": "Order.confirm", "target": {"created": 0}}]}}), "WIRE.VALUE_SOURCE_NOT_ALLOWED"),
    ] {
        assert_eq!(post(body.clone()).await["code"], code, "{body}");
    }
    assert_eq!((stock(3).await, rows().await), (10, before));

    // A request that is not about this contract is refused before it runs (the fingerprint is part of every write).
    let stale = post_with(address, &token, &"0".repeat(64), &flow("c1", 6, 1)).await;
    assert_eq!(stale["code"], "WIRE.CONTRACT_MISMATCH", "{stale}");
    assert_eq!((stock(6).await, rows().await), (10, before));

    // The same key sent 12 times at once: one execution (one order, one line, one deduction), eleven replays of the same ids.
    let before = rows().await;
    let calls: Vec<_> = (0..12).map(|_| tokio::spawn(post(flow("race-same", 4, 2)))).collect();
    let (mut replayed, mut created) = (0, std::collections::BTreeSet::new());
    for call in calls {
        let result = call.await.unwrap();
        assert_eq!(result["ok"], true, "{result}");
        replayed += usize::from(result["replayed"] == true);
        created.insert(result["created"].to_string());
    }
    assert_eq!((replayed, created.len(), stock(4).await), (11, 1, 8));
    assert_eq!(rows().await, (before.0 + 1, before.1 + 1, before.2 + 1));

    // I-3: the response is lost after COMMIT. The server says COMMIT_UNKNOWN; the same key replays the committed ids.
    let (proxy_address, fired) = aip_test_fault_proxy::start(Fault::AckDropFin).await;
    let proxied = listen_with_apply_contract(facts.clone(), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), aip_test_support::database_url_via_proxy(proxy_address.port()))
        .await
        .unwrap();
    let lossy_token = proxied.keys.issue(1, 600);
    let before = rows().await;
    let lost = post_with(proxied.address, &lossy_token, &fingerprint, &flow("lost-1", 6, 5)).await;
    let phase = timeout(Duration::from_secs(2), fired).await.expect("the proxy cut the COMMIT acknowledgement").expect("fault signal");
    assert_eq!(lost["code"], "WIRE.COMMIT_UNKNOWN", "phase={phase}: {lost}");
    // The commit did happen: order, line, deduction and idempotency record are all there.
    assert_eq!((stock(6).await, rows().await), (5, (before.0 + 1, before.1 + 1, before.2 + 1)));
    let recovered = post_with(proxied.address, &lossy_token, &fingerprint, &flow("lost-1", 6, 5)).await;
    assert_eq!((recovered["ok"].clone(), recovered["replayed"].clone()), (json!(true), json!(true)), "{recovered}");
    let made = recovered["created"].as_array().unwrap();
    assert_eq!(made.iter().map(|c| c["resource"].as_str().unwrap()).collect::<Vec<_>>(), ["Order", "OrderLine"]);
    let order_id = made[0]["id"].as_str().unwrap();
    assert_eq!(
        number(&db, format!("SELECT count(*) FROM {schema}.\"order\" o JOIN {schema}.order_line l ON l.order_id = o.id WHERE o.id = {order_id} AND l.id = {}", made[1]["id"].as_str().unwrap())).await,
        1,
        "the replayed ids are the committed rows"
    );
    assert_eq!((stock(6).await, rows().await), (5, (before.0 + 1, before.1 + 1, before.2 + 1)), "the recovery did not run it again");
    proxied.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
