//! A SERIALIZABLE write extension that loses a race must say `WIRE.CONFLICT` (retryable), never `WIRE.INTERNAL`.
//! PostgreSQL reports a serialization failure on whichever statement notices the read/write dependency first, and one of
//! them is the `UPDATE aip_idem SET result` that records the extension's result before COMMIT.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{
        listen_with_prototype_deployment_launch, DeploymentFence, ReadExtensions, ServerOptions, WorkerIsolation, WorkerLang, WorkerLaunchConfig,
    },
};
use std::{fs, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const DATABASE: &str = aip_test_support::test_database_url!();

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = "CONFIRMED"
resource Booking {
  fields { id: Id; room: Room; status: Text }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = "REQUESTED"; to status = "CONFIRMED" }
  expose apply confirm { target id; bulk maxRows 1 }
  extension write confirmRoom {
    input { id: Booking.Id }
    output { id: Booking.Id; count: Int }
    access Booking.confirm
    effect db
    deadline 2s
    implementation "write.confirmRoom"
  }
}
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn losers_of_a_serializable_extension_race_report_a_retryable_conflict() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-ser-ext-conflict-{unique}"));
    fs::create_dir(&path).unwrap();
    let owned = OwnedDir(path);
    // The pause keeps every caller inside its transaction until all of them have read the room's bookings.
    fs::write(
        owned.0.join("write.mjs"),
        "export async function confirmRoom(input,ctx){await new Promise(r=>setTimeout(r,80));const r=await ctx.data.apply('Booking','confirm',[input.id]);return {id:input.id,count:r.changed.length};}\n",
    )
    .unwrap();

    let mut facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_ser_ext_conflict_{unique}");
    let digest = "c".repeat(64);
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
         INSERT INTO {schema}.member(id) VALUES (1),(2),(3),(4); INSERT INTO {schema}.room(id,name) VALUES (1,'r');\
         INSERT INTO {schema}.booking(id,room_id,status) VALUES (1,1,'REQUESTED'),(2,1,'REQUESTED'),(3,1,'REQUESTED'),(4,1,'REQUESTED')"
    ))
    .await
    .unwrap();
    let catalog = spike_v2_read::external::snapshot(&db, &schema).await.unwrap();
    facts["externalStorage"] = json!({"mapping":{"schema":schema},"structureDigest":spike_v1_fixture::digest(&catalog)});

    let wire = IdWire::DecimalString;
    let fingerprint = contract_fingerprint_with_all_extensions(&facts, wire);
    let server = listen_with_prototype_deployment_launch(
        facts,
        wire,
        "127.0.0.1:0".parse().unwrap(),
        DATABASE.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: owned.0.clone() }),
        ServerOptions::default(),
        true,
        WorkerLaunchConfig { isolation: WorkerIsolation::None, ..Default::default() },
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        false,
    )
    .await
    .unwrap();

    let mut failures = vec![];
    for round in 0..12 {
        db.batch_execute(&format!("UPDATE {schema}.booking SET status='REQUESTED'")).await.unwrap();
        let calls: Vec<_> = (1..=4)
            .map(|booking| {
                let (address, token, fingerprint) = (server.address, server.keys.issue(booking, 60), fingerprint.clone());
                let body = json!({"key": format!("ser-{round}-{booking}"), "request": {"extension": "Booking.confirmRoom", "input": {"id": booking.to_string()}}});
                tokio::spawn(async move { post(address, &token, &fingerprint, &body).await })
            })
            .collect();
        let mut outcomes = vec![];
        for call in calls {
            let result = call.await.unwrap();
            outcomes.push(if result["ok"] == true { "OK".to_string() } else { result["code"].as_str().unwrap_or("?").to_string() });
        }
        outcomes.sort();
        let confirmed: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.booking WHERE status='CONFIRMED'"), &[]).await.unwrap().get(0);
        if confirmed != 1 || outcomes.iter().filter(|outcome| *outcome == "OK").count() != 1 {
            failures.push(format!("round {round}: expected exactly one confirmed booking, got {confirmed} and {outcomes:?}"));
        }
        if outcomes.iter().any(|outcome| outcome != "OK" && outcome != "WIRE.CONFLICT") {
            failures.push(format!("round {round}: a lost serializable race must be WIRE.CONFLICT, got {outcomes:?}"));
        }
    }
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
