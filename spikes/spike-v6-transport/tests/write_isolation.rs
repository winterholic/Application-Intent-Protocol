use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_options, ReadExtensions, ServerOptions, WorkerLang},
    write_extension,
};
use std::{fs, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const SPEC: &str = include_str!("fixtures/write-extension.aip");
const DB: &str = "host=localhost dbname=postgres";

struct Owned(PathBuf);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn post_apply(address: SocketAddr, token: &str, fingerprint: &str, body: &Value) -> Value {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let body = body.to_string();
    socket.write_all(format!(
        "POST /apply HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    ).as_bytes()).await.unwrap();
    let mut bytes = vec![];
    timeout(Duration::from_secs(5), socket.read_to_end(&mut bytes)).await.unwrap().unwrap();
    let response = String::from_utf8(bytes).unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn write_extension_pins_its_transaction_to_read_committed() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-isolation-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = Owned(dir);
    fs::write(owned.0.join("write.mjs"), "export async function run(input,ctx){const r=await ctx.data.apply('Event','mark',[input.id]);return {id:r.changed[0]??r.unchanged[0],count:1};}\n").unwrap();

    let source = SPEC.replace("actor Member\n", "actor Member\nlimit maxChecked on Event = atMost 2 where checked = true\n").replace(
        " fields { id: Id; member: Member; checked: Bool }",
        "  fields { id: Id; member: Member; checked: Bool }\n invariant maxChecked per member",
    );
    let facts: Value = load_str(&source, Form::A).unwrap().execution;
    let schema = format!("aip_isolation_v6_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&idempotency_ddl()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(12,1,true),(13,1,false),(21,2,false),(22,2,true);
         CREATE TABLE {schema}.isolation_seen(level text NOT NULL);
         CREATE FUNCTION {schema}.capture_isolation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO {schema}.isolation_seen VALUES(current_setting('transaction_isolation')); RETURN NEW; END $$;
         CREATE TRIGGER capture_isolation BEFORE UPDATE ON {schema}.event FOR EACH ROW EXECUTE FUNCTION {schema}.capture_isolation();
         SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL REPEATABLE READ"
    )).await.unwrap();

    let caller = Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() };
    let request = json!({"extension":"Event.run","input":{"id":"11"}});
    let result = write_extension::apply(
        &mut db,
        &facts,
        &json!({"key":"isolation","request":request}),
        &caller,
        IdWire::DecimalString,
        &ReadExtensions { lang: WorkerLang::Node, dir: owned.0.clone() },
    )
    .await;
    assert_eq!(result["ok"], true, "{result}");
    let rejected = write_extension::apply(
        &mut db,
        &facts,
        &json!({"key":"capacity","request":{"extension":"Event.run","input":{"id":"13"}}}),
        &caller,
        IdWire::DecimalString,
        &ReadExtensions { lang: WorkerLang::Node, dir: owned.0.clone() },
    )
    .await;
    assert_eq!(rejected["code"], "INVARIANT_VIOLATED", "{rejected}");

    let server_db = "host=localhost dbname=postgres options='-c default_transaction_isolation=serializable'";
    let server = listen_with_prototype_options(
        facts.clone(),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        server_db.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: owned.0.clone() }),
        ServerOptions::default(),
        true,
    )
    .await
    .unwrap();
    let token = server.keys.issue(2, 60);
    let fingerprint = contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
    let standard =
        post_apply(server.address, &token, &fingerprint, &json!({"key":"standard","request":{"apply":"Event.mark","target":{"ids":["21"]}}})).await;
    assert_eq!(standard["ok"], true, "{standard}");
    let levels: Vec<String> =
        db.query(&format!("SELECT level FROM {schema}.isolation_seen ORDER BY ctid"), &[]).await.unwrap().iter().map(|row| row.get(0)).collect();
    assert_eq!(levels, ["read committed", "read committed"]);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    server.shutdown().await.unwrap();
}
