//! `ctx.data.read` of a write extension runs the caller's public read inside the extension's transaction. PostgreSQL reports a
//! serialization failure (SERIALIZABLE) or a deadlock on whichever statement notices it first, and that can be a read. The read
//! path used to turn every database error it did not know into `WIRE.INTERNAL`, so the extension ended as `WIRE.EXTENSION_ERROR`
//! (a failing extension, nothing to retry) instead of the retryable `WIRE.CONFLICT` that `ctx.data.apply` already reports for the same
//! SQLSTATEs. The failure is injected the way `apply_idem_lookup_error` does it: the read target is replaced by a view whose filter
//! raises a chosen SQLSTATE, so the read statement fails and nothing else does.
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
resource Room {
  fields { id: Id; name: Text }
  rows read when true
  expose read { select id, name; filter id.eq; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource Booking {
  fields { id: Id; room: Room; status: Text }
  rows read when true
  transition confirm { allow actor != null; from status = "REQUESTED"; to status = "CONFIRMED" }
  expose apply confirm { target id; bulk maxRows 1 }
  extension write readThenConfirm {
    input { id: Booking.Id }
    output { ok: Bool }
    access read.Room, Booking.confirm
    effect db
    deadline 2s
    implementation "write.readThenConfirm"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_serialization_failure_or_deadlock_in_ctx_data_read_is_a_retryable_conflict() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-ext-read-conflict-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(
        owned.0.join("write.mjs"),
        "export async function readThenConfirm(input,ctx){await ctx.data.read({read:'Room',select:['id','name'],filter:[{field:'id',op:'eq',value:'1'}],limit:1});await ctx.data.apply('Booking','confirm',[input.id]);return {ok:true};}\n",
    )
    .unwrap();

    let mut facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_ext_read_conflict_{unique}");
    let digest = "e".repeat(64);
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DATABASE).await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("new isolated schema");
    for statement in ddl.iter().skip(1) {
        db.batch_execute(statement).await.unwrap();
    }
    db.batch_execute(&idempotency_ddl()).await.unwrap();
    // `STABLE` with no column argument makes the view's filter a one-time gate that also runs for an empty read.
    db.batch_execute(&format!(
        "CREATE TABLE {schema}.aip_migrate_meta(singleton boolean PRIMARY KEY, facts_digest text NOT NULL); INSERT INTO {schema}.aip_migrate_meta VALUES(true,'{digest}');\
         INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.room(id,name) VALUES (1,'r');\
         INSERT INTO {schema}.booking(id,room_id,status) VALUES (1,1,'REQUESTED');\
         ALTER TABLE {schema}.room RENAME TO room_real;\
         CREATE TABLE {schema}.read_failure(sqlstate text NOT NULL);\
         CREATE FUNCTION {schema}.read_gate() RETURNS boolean LANGUAGE plpgsql STABLE AS $$ \
           DECLARE f record; \
           BEGIN \
             SELECT * INTO f FROM {schema}.read_failure; \
             IF f.sqlstate IS NOT NULL THEN RAISE EXCEPTION 'injected ctx.data.read failure' USING ERRCODE = f.sqlstate; END IF; \
             RETURN true; \
           END $$; \
         CREATE VIEW {schema}.room AS SELECT * FROM {schema}.room_real WHERE {schema}.read_gate()"
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
    let token = server.keys.issue(1, 60);
    let confirm = |key: &str| json!({"key": key, "request": {"extension": "Booking.readThenConfirm", "input": {"id": "1"}}});
    let inject = |state: &'static str| format!("TRUNCATE {schema}.read_failure; INSERT INTO {schema}.read_failure VALUES ('{state}')");
    let status = || async { db.query_one(&format!("SELECT status FROM {schema}.booking WHERE id=1"), &[]).await.unwrap().get::<_, String>(0) };
    let journal = || async { db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get::<_, i64>(0) };

    for state in ["40001", "40P01"] {
        db.batch_execute(&inject(state)).await.unwrap();
        let outcome = post(server.address, &token, &fingerprint, &confirm("conflict-key")).await;
        assert_eq!(outcome["code"], "WIRE.CONFLICT", "{state} in ctx.data.read must be the retryable conflict: {outcome}");
        assert_eq!(status().await, "REQUESTED", "{state}: nothing the extension wrote may stay");
        assert_eq!(journal().await, 0, "{state}: a refused call leaves no idempotency record");
    }

    // A failure that is not about concurrency keeps its meaning: the extension failed, nothing to retry.
    db.batch_execute(&inject("42501")).await.unwrap();
    let denied = post(server.address, &token, &fingerprint, &confirm("denied-key")).await;
    assert_eq!(denied["code"], "WIRE.EXTENSION_ERROR", "{denied}");
    assert_eq!(status().await, "REQUESTED");

    // Once the database answers, the same key runs the extension once.
    db.batch_execute(&format!("TRUNCATE {schema}.read_failure")).await.unwrap();
    let retried = post(server.address, &token, &fingerprint, &confirm("conflict-key")).await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(status().await, "CONFIRMED");
    assert_eq!(journal().await, 1);

    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
