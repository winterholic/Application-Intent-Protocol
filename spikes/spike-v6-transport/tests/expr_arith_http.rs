//! P0-B over real HTTP: the caller sends only an amount (`input`), the server computes from the locked row, and the idempotency key
//! keeps a resend from deducting twice (test-plan I-1, I-2), also while the same key races itself and while many keys race one row.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000); cash: Decimal(12,2) }
  rows read when owner = actor
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow owner = actor
    to stock = stock - input.qty
  }
  transition pay {
    input { amount: Decimal(12,2) }
    from cash >= input.amount
    allow owner = actor
    to cash = cash - input.amount
  }
  expose apply take { target id; bulk maxRows 5 }
  expose apply pay { target id; bulk maxRows 5 }
}
"#;

async fn post(address: SocketAddr, token: &str, body: &Value) -> Value {
    let body = body.to_string();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&load_str(SPEC, Form::A).unwrap().execution, IdWire::DecimalString);
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

fn take(key: &str, id: &str, qty: i64) -> Value {
    json!({"key": key, "request": {"apply": "Product.take", "target": {"ids": [id]}, "input": {"qty": qty}}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_resent_key_never_deducts_twice_and_a_changed_input_under_the_same_key_is_refused() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_expr_arith_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = spike_v2_read::connect().await;
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.product(id,owner_id,stock,cash) VALUES (1,1,10,100.00),(2,1,10,100.00),(3,1,10,100.00),(4,1,10,100.00)"
    ))
    .await
    .unwrap();
    let (port, keys) = spike_v6_transport::start_with_apply_contract(facts, IdWire::DecimalString).await;
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let token = keys.issue(1, 300);
    let stock = |id: i64| {
        let (db, schema) = (&db, &schema);
        async move { db.query_one(&format!("SELECT stock::text FROM {schema}.product WHERE id = {id}"), &[]).await.unwrap().get::<_, String>(0) }
    };

    // I-1: the first call deducts, the resend replays the stored result and deducts nothing more.
    let first = post(address, &token, &take("t1", "1", 3)).await;
    assert_eq!((first["ok"].clone(), first["changed"].clone()), (json!(true), json!(["1"])), "{first}");
    assert_eq!(stock(1).await, "7");
    let resent = post(address, &token, &take("t1", "1", 3)).await;
    assert_eq!((resent["ok"].clone(), resent["replayed"].clone(), resent["changed"].clone()), (json!(true), json!(true), json!(["1"])), "{resent}");
    assert_eq!(stock(1).await, "7", "the replay must not deduct again");

    // I-2: the same key with only the input different is a different request.
    let mismatch = post(address, &token, &take("t1", "1", 4)).await;
    assert_eq!(mismatch["code"], "WIRE.IDEMPOTENCY_MISMATCH", "{mismatch}");
    assert_eq!(stock(1).await, "7");

    // Decimal input: exact, and replayed the same way.
    let pay = json!({"key": "p1", "request": {"apply": "Product.pay", "target": {"ids": ["1"]}, "input": {"amount": "12.50"}}});
    assert_eq!(post(address, &token, &pay).await["ok"], true);
    assert_eq!(post(address, &token, &pay).await["replayed"], true);
    let cash = db.query_one(&format!("SELECT cash::text FROM {schema}.product WHERE id = 1"), &[]).await.unwrap().get::<_, String>(0);
    assert_eq!(cash, "87.50");

    // input shape errors come back as wire codes and store no idempotency result.
    let stored = || async { db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get::<_, i64>(0) };
    let before = stored().await;
    for (body, code) in [
        (json!({"key": "e1", "request": {"apply": "Product.take", "target": {"ids": ["1"]}}}), "WIRE.BAD_VALUE"),
        (json!({"key": "e2", "request": {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 0}}}), "WIRE.BAD_VALUE"),
        (json!({"key": "e3", "request": {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1, "x": 1}}}), "WIRE.FIELD_NOT_EXPOSED"),
        (take("e4", "1", 999), "WIRE.BAD_VALUE"),
        (take("e5", "1", 8), "WIRE.INVALID_STATE"),
    ] {
        assert_eq!(post(address, &token, &body).await["code"], code, "{body}");
    }
    assert_eq!((stored().await, stock(1).await.as_str()), (before, "7"));

    // a bundle carries input per apply operation and replays whole.
    let bundle = json!({"key": "bn1", "request": {"atomic": [
        {"apply": "Product.take", "target": {"ids": ["2"]}, "input": {"qty": 2}},
        {"apply": "Product.take", "target": {"ids": ["2"]}, "input": {"qty": 3}}
    ]}});
    assert_eq!(post(address, &token, &bundle).await["ok"], true);
    assert_eq!(post(address, &token, &bundle).await["replayed"], true);
    assert_eq!(stock(2).await, "5");

    // The same key sent 12 times at once: one execution, eleven replays, one deduction.
    let same_key: Vec<_> = (0..12).map(|_| tokio::spawn({ let token = token.clone(); async move { post(address, &token, &take("race-same", "3", 4)).await } })).collect();
    let mut replayed = 0;
    for call in same_key {
        let result = call.await.unwrap();
        assert_eq!(result["ok"], true, "{result}");
        replayed += usize::from(result["replayed"] == true);
    }
    assert_eq!((replayed, stock(3).await.as_str()), (11, "6"));

    // 20 different keys, one unit each, stock 10: exactly 10 deduct and the rest are INVALID_STATE; stock ends at 0.
    let calls: Vec<_> = (0..20).map(|n| tokio::spawn({ let token = token.clone(); async move { post(address, &token, &take(&format!("race-{n}"), "4", 1)).await } })).collect();
    let (mut done, mut refused) = (0, 0);
    for call in calls {
        let result = call.await.unwrap();
        match result["code"].as_str() {
            None if result["ok"] == true => done += 1,
            Some("WIRE.INVALID_STATE") => refused += 1,
            other => panic!("unexpected {other:?}: {result}"),
        }
    }
    assert_eq!((done, refused, stock(4).await.as_str()), (10, 10, "0"));
}
