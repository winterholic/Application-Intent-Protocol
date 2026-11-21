use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_launch, ReadExtensions, ServerOptions, WorkerIsolation, WorkerLang, WorkerLaunchConfig},
};
use std::{fs, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const SPEC: &str = include_str!("fixtures/write-extension.aip");
const DATABASE: &str = aip_test_support::test_database_url!();

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn post(address: SocketAddr, token: Option<&str>, fingerprint: Option<&str>, body: &Value) -> Value {
    let mut socket = TcpStream::connect(address).await.expect("connect HTTP request");
    let body = body.to_string();
    let auth = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    let contract = fingerprint.map(|fingerprint| format!("x-aip-contract: {fingerprint}\r\n")).unwrap_or_default();
    socket
        .write_all(format!("POST /apply HTTP/1.1\r\nhost: {address}\r\n{auth}{contract}content-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .expect("write HTTP request");
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(5), socket.read_to_end(&mut bytes)).await.expect("HTTP response deadline").expect("read HTTP response");
    let response = String::from_utf8(bytes).expect("HTTP response UTF-8");
    let (_, body) = response.split_once("\r\n\r\n").expect("HTTP response headers");
    serde_json::from_str(body).expect("HTTP JSON response")
}

fn request(id: &str) -> Value {
    json!({"extension":"Event.run","input":{"id":id}})
}

fn body(key: &str, id: &str) -> Value {
    json!({"key":key,"request":request(id)})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_pg_write_extension_replays_before_full_worker_slots() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-write-replay-pressure-{unique}"));
    fs::create_dir(&path).unwrap();
    let owned = OwnedDir(path);
    let root = owned.0.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'");
    fs::write(
        owned.0.join("write.mjs"),
        format!(
            "import fs from 'node:fs';\nconst root='{root}';\nexport async function run(input,ctx){{fs.appendFileSync(root+'/started',String(input.id)+'\\n');if(fs.existsSync(root+'/slow'))await new Promise(resolve=>setTimeout(resolve,900));const r=await ctx.data.apply('Event','mark',[input.id]);return {{id:r.changed[0]??r.unchanged[0],count:1}};}}\n"
        ),
    )
    .unwrap();

    let facts: Value = load_str(SPEC, Form::A).unwrap().execution;
    let schema = format!("aip_write_replay_pressure_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DATABASE).await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("new isolated schema");
    for statement in ddl.iter().skip(1) {
        db.batch_execute(statement).await.unwrap();
    }
    db.batch_execute(&idempotency_ddl()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2);INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(12,1,false),(13,1,false),(14,1,false),(15,1,false),(99,2,false)"
    ))
    .await
    .unwrap();

    let wire = IdWire::DecimalString;
    let fingerprint = contract_fingerprint_with_all_extensions(&facts, wire);
    let server = listen_with_prototype_launch(
        facts,
        wire,
        "127.0.0.1:0".parse().unwrap(),
        DATABASE.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: owned.0.clone() }),
        ServerOptions::default(),
        true,
        WorkerLaunchConfig { isolation: WorkerIsolation::None, ..Default::default() },
    )
    .await
    .unwrap();
    let token = server.keys.issue(1, 60);
    let first = post(server.address, Some(&token), Some(&fingerprint), &body("done-key", "11")).await;
    assert_eq!(first["ok"], true, "initial extension must commit: {first}");
    assert_eq!(first["replayed"], Value::Null);
    fs::write(owned.0.join("slow"), "hold workers").unwrap();

    let mut workers = Vec::new();
    for index in 0..4 {
        let address = server.address;
        let token = token.clone();
        let fingerprint = fingerprint.clone();
        workers.push(tokio::spawn(async move {
            post(address, Some(&token), Some(&fingerprint), &body(&format!("pending-{index}"), &(12 + index).to_string())).await
        }));
    }
    timeout(Duration::from_secs(2), async {
        loop {
            if fs::read_to_string(owned.0.join("started")).is_ok_and(|started| started.lines().count() >= 5) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("four workers started while one earlier completed request exists");

    let mut failures = Vec::new();
    let replay = post(server.address, Some(&token), Some(&fingerprint), &body("done-key", "11")).await;
    if replay["ok"] != true || replay["replayed"] != true || replay["output"] != first["output"] {
        failures.push(format!("completed matching request should replay while all four slots are occupied: {replay}"));
    }
    let mismatch = post(server.address, Some(&token), Some(&fingerprint), &body("done-key", "99")).await;
    if mismatch["code"] != "WIRE.IDEMPOTENCY_MISMATCH" {
        failures.push(format!("completed key with a different request must retain mismatch rejection: {mismatch}"));
    }
    let new_request = post(server.address, Some(&token), Some(&fingerprint), &body("new-key", "11")).await;
    if new_request["code"] != "WIRE.WORKER_BUSY" {
        failures.push(format!("new key must retain worker backpressure: {new_request}"));
    }
    let incomplete = post(server.address, Some(&token), Some(&fingerprint), &body("pending-0", "12")).await;
    if incomplete["code"] != "WIRE.WORKER_BUSY" {
        failures.push(format!("incomplete key must retain worker backpressure: {incomplete}"));
    }
    let no_auth = post(server.address, None, Some(&fingerprint), &body("done-key", "11")).await;
    if no_auth["code"] != "WIRE.UNAUTHENTICATED" {
        failures.push(format!("authentication must still precede replay: {no_auth}"));
    }
    let wrong_fingerprint = format!("{}{}", if fingerprint.starts_with('0') { "1" } else { "0" }, &fingerprint[1..]);
    let wrong_contract = post(server.address, Some(&token), Some(&wrong_fingerprint), &body("done-key", "11")).await;
    if wrong_contract["code"] != "WIRE.CONTRACT_MISMATCH" {
        failures.push(format!("contract fingerprint must still precede replay: {wrong_contract}"));
    }
    let started = fs::read_to_string(owned.0.join("started")).unwrap();
    if started.lines().count() != 5 {
        failures.push(format!("replay and rejected requests must not start another worker: {} starts", started.lines().count()));
    }

    fs::remove_file(owned.0.join("slow")).unwrap();
    for worker in workers {
        let result = worker.await.unwrap();
        if result["ok"] != true {
            failures.push(format!("pending worker did not settle: {result}"));
        }
    }
    let other_caller = post(server.address, Some(&server.keys.issue(2, 60)), Some(&fingerprint), &body("done-key", "11")).await;
    if other_caller["code"] != "WIRE.EXTENSION_ERROR" {
        failures.push(format!("same key must not replay across caller scope: {other_caller}"));
    }
    let denied = post(server.address, Some(&token), Some(&fingerprint), &body("denied-key", "99")).await;
    if denied["code"] != "WIRE.EXTENSION_ERROR" {
        failures.push(format!("scope denial must remain a failed write: {denied}"));
    }
    let rows: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get(0);
    if rows != 5 {
        failures.push(format!("only the completed request and four pending requests should be journaled; got {rows}"));
    }
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
