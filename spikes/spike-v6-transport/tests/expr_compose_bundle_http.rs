//! P0-D over real HTTP: a bundle with a `{compose, targets, steps}` operation next to an input transition. The cache tags count the
//! compose and its steps (D-1), the serializable marker of the compose's contract decides the isolation (D-2), a compose plan error
//! is refused before anything runs (D-3), and the response says which `applied[]`/`created[]` entries belong to which operation
//! (D-4) without changing the response of a bundle that has no compose. The same key replays the whole result.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen};
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
resource Log {
  fields { id: Id; owner: Member; note: Text(1..20) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, note }
}
resource Order {
  fields { id: Id; owner: Member; status: Text(1..12) }
  rows read when owner = actor
  transition confirm { from status = "open"; allow owner = actor; to status = "confirmed" }
  expose apply confirm { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 5; transitions confirm; create Log from owner }
}
predicate free(m: Member) = not exists Slot where owner = m and taken = true
resource Slot {
  fields { id: Id; owner: Member; taken: Bool }
  rows read when owner = actor
  expose create { allow free(this.owner); fields owner, taken }
}
resource Room {
  fields { id: Id; owner: Member; name: Text(1..20) }
  rows read when owner = actor
  transition touch { from true; allow owner = actor; to name = "touched" }
  expose compose { bulk maxRows 3; transitions touch; create Slot from owner }
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

fn compose_order(ids: &[i64], steps: Vec<Value>) -> Value {
    json!({"compose": "Order", "targets": {"ids": ids.iter().map(i64::to_string).collect::<Vec<_>>()}, "steps": steps})
}

fn log_step() -> Value {
    json!({"create": "Log", "values": {"owner": {"item": "owner"}, "note": {"const": "x"}}})
}

fn take(id: i64, qty: i64) -> Value {
    json!({"apply": "Product.take", "target": {"ids": [id.to_string()]}, "input": {"qty": qty}})
}

async fn number(db: &tokio_postgres::Client, sql: String) -> i64 {
    db.query_one(&sql, &[]).await.unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bundle_with_a_compose_reports_its_boundaries_tags_and_replays_whole() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_expr_compose_bundle_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = connect_owned_with_url(DATABASE).await.expect("run-owned PostgreSQL");
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.product(id,owner_id,stock) VALUES (1,1,10),(2,1,10),(3,1,10),(4,1,10),(5,1,10),(7,1,10); \
         INSERT INTO {schema}.\"order\"(id,owner_id,status) VALUES (1,1,'open'),(2,1,'open'),(3,1,'open'),(4,1,'open'),(5,1,'open'),(6,1,'open'),(7,1,'open'),(8,1,'shipped'),(9,1,'open'),(10,1,'open'); \
         INSERT INTO {schema}.room(id,owner_id,name) VALUES (1,1,'r'); \
         CREATE TABLE {schema}.seen(tbl text NOT NULL, level text NOT NULL); \
         CREATE FUNCTION {schema}.capture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO {schema}.seen VALUES(TG_TABLE_NAME, current_setting('transaction_isolation')); RETURN NEW; END $$; \
         CREATE TRIGGER capture BEFORE INSERT ON {schema}.slot FOR EACH ROW EXECUTE FUNCTION {schema}.capture(); \
         CREATE TRIGGER capture BEFORE INSERT ON {schema}.log FOR EACH ROW EXECUTE FUNCTION {schema}.capture()"
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
    let confirmed = || async { number(&db, format!("SELECT count(*) FROM {schema}.\"order\" WHERE status = 'confirmed'")).await };
    let stored = || async { number(&db, format!("SELECT count(*) FROM {schema}.aip_idem")).await };
    let logs = || async { number(&db, format!("SELECT count(*) FROM {schema}.log")).await };

    // D-S1 + D-4 + D-1: input transition and compose in one bundle; the response names each operation's entries and the cache tags
    // include the compose's resources (an `Order` change that only the compose makes must still invalidate `Order`).
    let first = post(json!({"key": "c1", "request": {"atomic": [take(1, 3), compose_order(&[1, 2, 3], vec![json!({"transition": "confirm"}), log_step()])]}})).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["applied"].as_array().unwrap().len(), 2);
    assert_eq!(first["created"].as_array().unwrap().len(), 3);
    assert_eq!(first["ops"], json!([{"kind": "apply", "applied": [0, 1], "created": [0, 0]}, {"kind": "compose", "applied": [1, 2], "created": [0, 3]}]));
    assert_eq!(first["tags"], json!(["Log", "Order", "Product"]));
    assert_eq!((stock(1).await, confirmed().await, logs().await), (7, 3, 3));

    // D-1 alone: a bundle whose only writes are the compose's still lists the compose's resources.
    let only = post(json!({"key": "c2", "request": {"atomic": [compose_order(&[4], vec![json!({"transition": "confirm"}), log_step()])]}})).await;
    assert_eq!(only["tags"], json!(["Log", "Order"]), "{only}");

    // I-1: the same key replays the stored result (boundaries included) and runs nothing again.
    let before = (stock(1).await, confirmed().await, logs().await, stored().await);
    let resent = post(json!({"key": "c1", "request": {"atomic": [take(1, 3), compose_order(&[1, 2, 3], vec![json!({"transition": "confirm"}), log_step()])]}})).await;
    assert_eq!((resent["ok"].clone(), resent["replayed"].clone(), resent["ops"].clone(), resent["created"].clone()), (json!(true), json!(true), first["ops"].clone(), first["created"].clone()));
    assert_eq!((stock(1).await, confirmed().await, logs().await, stored().await), before);
    // I-2: the same key with a different compose is a different request.
    let other = post(json!({"key": "c1", "request": {"atomic": [take(1, 3), compose_order(&[1, 2], vec![json!({"transition": "confirm"}), log_step()])]}})).await;
    assert_eq!(other["code"], "WIRE.IDEMPOTENCY_MISMATCH");

    // A bundle without a compose answers exactly as before: no `ops` key (older clients read `applied[]` and `created[]` only).
    let plain = post(json!({"key": "p1", "request": {"atomic": [take(2, 1), {"create": "Log", "values": {"owner": "1", "note": "n"}}]}})).await;
    assert_eq!(plain["ok"], true, "{plain}");
    assert!(plain.get("ops").is_none(), "{plain}");
    assert_eq!(plain["tags"], json!(["Log", "Product"]));

    // R-1: a later operation fails (stock 10 < 100) or the compose's own step fails (a shipped order): nothing stays, no idempotency record.
    let (stock_before, confirmed_before, logs_before, stored_before) = (stock(3).await, confirmed().await, logs().await, stored().await);
    let late = post(json!({"key": "r1", "request": {"atomic": [compose_order(&[5, 6], vec![json!({"transition": "confirm"}), log_step()]), take(3, 100)]}})).await;
    assert_eq!(late["code"], "WIRE.INVALID_STATE", "{late}");
    let inner = post(json!({"key": "r2", "request": {"atomic": [take(3, 2), compose_order(&[7, 8], vec![json!({"transition": "confirm"}), log_step()])]}})).await;
    assert_eq!(inner["code"], "WIRE.INVALID_STATE", "{inner}");
    assert_eq!((stock(3).await, confirmed().await, logs().await, stored().await), (stock_before, confirmed_before, logs_before, stored_before));

    // D-3 over the wire: a compose plan error comes back as its code and stores nothing; nesting is refused as an unknown key.
    for (body, code) in [
        (json!({"key": "e1", "request": {"atomic": [take(3, 1), compose_order(&[9], vec![json!({"transition": "nothing"})])]}}), "WIRE.STEP_NOT_ALLOWED"),
        (json!({"key": "e2", "request": {"atomic": [take(3, 1), compose_order(&[9], vec![compose_order(&[9], vec![json!({"transition": "confirm"})])])]}}), "WIRE.UNKNOWN_KEY"),
        (json!({"key": "e3", "request": {"atomic": [take(3, 1), {"compose": "Product", "targets": {"ids": ["3"]}, "steps": [{"transition": "take"}]}]}}), "WIRE.NOT_EXPOSED"),
    ] {
        assert_eq!(post(body.clone()).await["code"], code, "{body}");
    }
    assert_eq!((stock(3).await, stored().await), (stock_before, stored_before));

    // D-2: the compose of Room needs SERIALIZABLE (its create allow reads for absence); Order's compose does not.
    let room = json!({"compose": "Room", "targets": {"ids": ["1"]}, "steps": [{"transition": "touch"}, {"create": "Slot", "values": {"owner": {"item": "owner"}, "taken": {"const": true}}}]});
    assert_eq!(post(json!({"key": "s1", "request": {"atomic": [room]}})).await["ok"], true);
    assert_eq!(post(json!({"key": "s2", "request": {"atomic": [compose_order(&[10], vec![log_step()])]}})).await["ok"], true);
    let seen: Vec<(String, String)> = db.query(&format!("SELECT tbl, level FROM {schema}.seen WHERE tbl = 'slot' OR tbl = 'log' ORDER BY tbl, level"), &[]).await.unwrap().iter().map(|r| (r.get(0), r.get(1))).collect();
    assert!(seen.contains(&("slot".to_string(), "serializable".to_string())), "{seen:?}");
    assert!(!seen.iter().any(|(tbl, level)| tbl == "slot" && level != "serializable"), "{seen:?}");

    // The generated TypeScript client builds the same kind of bundle with `b.compose` and reads the boundaries (typed `ops`).
    let client_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    std::fs::write(
        format!("{client_dir}/generated-expr-compose.ts"),
        spike_v5_sdk::contract_module_with_apply(&facts, "../../spike-v5-sdk/sdk/generic.ts", IdWire::DecimalString),
    )
    .unwrap();
    let node_token = keys.issue(1, 600);
    let node = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .current_dir(client_dir)
            .env("AIP_COMPOSE_URL", format!("http://127.0.0.1:{port}"))
            .env("AIP_COMPOSE_TOKEN", node_token)
            .args(["--test", "expr-compose-bundle-http.test.ts"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let output = format!("{}{}", String::from_utf8_lossy(&node.stdout), String::from_utf8_lossy(&node.stderr));
    assert!(node.status.success(), "{output}");
    assert_eq!(stock(4).await, 8, "the TypeScript bundle deducted once");
    assert_eq!(number(&db, format!("SELECT count(*) FROM {schema}.\"order\" WHERE id IN (5) AND status = 'confirmed'")).await, 1);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
