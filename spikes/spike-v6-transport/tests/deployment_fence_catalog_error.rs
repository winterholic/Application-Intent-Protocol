//! The deployment fence compares the external schema's catalog digest before each request (`check_deployment_fence`). A catalog read that
//! fails (the connection drops, the query times out) says nothing about the schema, so it must be reported as the retryable
//! `WIRE.DB_UNAVAILABLE`. It used to be reported as `WIRE.DEPLOYMENT_CHANGED`, which tells the operator to restart the service
//! and tells a saturated server's callers something other than `WORKER_BUSY`. Real drift must keep `DEPLOYMENT_CHANGED`.
use aip_test_fault_proxy::{start, Fault};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_deployment_launch, DeploymentFence, ServerOptions, WorkerIsolation, WorkerLaunchConfig},
};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const DATABASE: &str = aip_test_support::test_database_url!();
/// Only the catalog snapshot reads function definitions.
const CATALOG_SNAPSHOT: &str = "pg_get_functiondef";

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when true
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
  expose apply take { target id; bulk maxRows 5 }
}
"#;

async fn post(address: SocketAddr, token: &str, fingerprint: &str, body: &Value) -> Value {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.expect("connect");
    socket
        .write_all(
            format!(
                "POST /apply HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("write");
    let mut response = String::new();
    timeout(Duration::from_secs(10), socket.read_to_string(&mut response)).await.expect("HTTP deadline").expect("read");
    let (_, body) = response.split_once("\r\n\r\n").expect("HTTP headers");
    serde_json::from_str(body).expect("JSON body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_catalog_read_is_unavailable_not_a_changed_deployment() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let mut facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_fence_catalog_{unique}");
    let digest = "d".repeat(64);
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DATABASE).await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("new isolated schema");
    for statement in ddl.iter().skip(1) {
        db.batch_execute(statement).await.unwrap();
    }
    db.batch_execute(&idempotency_ddl()).await.unwrap();
    db.batch_execute(&format!(
        "CREATE TABLE {schema}.aip_migrate_meta(singleton boolean PRIMARY KEY, facts_digest text NOT NULL); INSERT INTO {schema}.aip_migrate_meta VALUES(true,'{digest}');\
         INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.product(id,owner_id,stock) VALUES (1,1,1000)"
    ))
    .await
    .unwrap();
    let catalog = spike_v2_read::external::snapshot(&db, &schema).await.unwrap();
    facts["externalStorage"] = json!({"mapping":{"schema":schema},"structureDigest":spike_v1_fixture::digest(&catalog)});
    let wire = IdWire::DecimalString;
    let fingerprint = contract_fingerprint_with_all_extensions(&facts, wire);
    let (proxy, fired) = start(Fault::CutAfterStatement(CATALOG_SNAPSHOT)).await;
    let server = listen_with_prototype_deployment_launch(
        facts,
        wire,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::database_url_via_proxy(proxy.port()),
        None,
        ServerOptions::default(),
        false,
        WorkerLaunchConfig { isolation: WorkerIsolation::None, ..Default::default() },
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        false,
    )
    .await
    .unwrap();
    let take = |key: &str| json!({"key": key, "request": {"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 1}}});
    let token = server.keys.issue(1, 60);

    // The first catalog read of the first request loses its connection.
    let first = post(server.address, &token, &fingerprint, &take("fence-1")).await;
    timeout(Duration::from_secs(1), fired).await.expect("the proxy must have cut the catalog read").unwrap();
    assert_eq!(first["code"], "WIRE.DB_UNAVAILABLE", "a failed catalog read is not drift: {first}");
    let stock = || async { db.query_one(&format!("SELECT stock::bigint FROM {schema}.product WHERE id=1"), &[]).await.unwrap().get::<_, i64>(0) };
    assert_eq!(stock().await, 1000, "the refused request wrote nothing");

    // The same key goes through once the database answers.
    let second = post(server.address, &token, &fingerprint, &take("fence-1")).await;
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(stock().await, 999);

    // Real drift keeps its code.
    db.batch_execute(&format!("ALTER TABLE {schema}.product ADD COLUMN drift int")).await.unwrap();
    let drifted = post(server.address, &token, &fingerprint, &take("fence-2")).await;
    assert_eq!(drifted["code"], "WIRE.DEPLOYMENT_CHANGED", "{drifted}");
    assert_eq!(stock().await, 999);

    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
