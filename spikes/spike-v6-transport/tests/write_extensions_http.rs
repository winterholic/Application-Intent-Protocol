use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url,
    id_wire::{emit_id, IdWire},
    sqlgen,
};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_options, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{fs, net::SocketAddr, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
const SPEC: &str = include_str!("fixtures/write-extension.aip");
struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
async fn post(address: SocketAddr, path: &str, token: Option<&str>, fp: Option<&str>, body: &Value) -> Value {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let body = body.to_string();
    let auth = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    let contract = fp.map(|fp| format!("x-aip-contract: {fp}\r\n")).unwrap_or_default();
    socket
        .write_all(format!("POST {path} HTTP/1.1\r\nhost: {address}\r\n{auth}{contract}content-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut bytes = vec![];
    timeout(Duration::from_secs(5), socket.read_to_end(&mut bytes)).await.unwrap().unwrap();
    let response = String::from_utf8(bytes).unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}
#[tokio::test]
async fn write_extensions_require_session_contract_and_opt_in_then_replay_atomic_results() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-write-http-{unique}"));
    fs::create_dir(&path).unwrap();
    let owned = OwnedDir(path);
    fs::write(owned.0.join("write.mjs"),"export async function run(input,ctx){const r=await ctx.data.apply('Event','mark',[input.id]);return {id:r.changed[0]??r.unchanged[0],count:1};}\n").unwrap();
    fs::write(owned.0.join("write.py"),"async def run(input,ctx):\n    r=await ctx.data.apply('Event','mark',[input['id']])\n    return {'id':(r['changed']+r['unchanged'])[0],'count':1}\n").unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap().execution;
    let schema = format!("aip_write_http_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures = vec![];
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES(1),(2);INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(99,2,false)"
        ))
        .await?;
        for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
            for lang in [WorkerLang::Node, WorkerLang::Python] {
                let config = ReadExtensions { lang, dir: owned.0.clone() };
                let fp = contract_fingerprint_with_all_extensions(&facts, wire);
                let input = emit_id(11, wire).unwrap();
                let request = json!({"extension":"Event.run","input":{"id":input}});
                let body = json!({"key":"write-http","request":request});
                let denied = listen_with_prototype_options(
                    facts.clone(),
                    wire,
                    "127.0.0.1:0".parse().unwrap(),
                    "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1".into(),
                    Some(config.clone()),
                    ServerOptions::default(),
                    true,
                )
                .await
                .unwrap();
                let token = denied.keys.issue(1, 60);
                let wrong_id = if wire == IdWire::SafeNumber { json!("11") } else { json!(11) };
                let cases = [
                    (None, Some(fp.as_str()), body.clone(), "UNAUTHENTICATED"),
                    (Some(token.as_str()), None, body.clone(), "CONTRACT_MISMATCH"),
                    (
                        Some(token.as_str()),
                        Some(fp.as_str()),
                        json!({"key":"invalid","request":{"extension":"Event.run","input":{"id":wrong_id}}}),
                        "BAD_VALUE",
                    ),
                    (
                        Some(token.as_str()),
                        Some(fp.as_str()),
                        json!({"key":"invalid","request":{"extension":"Event.run","input":{"id":input},"apply":"Event.mark"}}),
                        "BAD_REQUEST",
                    ),
                    (
                        Some(token.as_str()),
                        Some(fp.as_str()),
                        json!({"key":"invalid","request":{"extension":"Event.missing","input":{"id":input}}}),
                        "NOT_EXPOSED",
                    ),
                ];
                for (token, contract, request, code) in cases {
                    let result = post(denied.address, "/apply", token, contract, &request).await;
                    if result["code"] != code {
                        failures.push(format!("{wire:?} {lang:?} pre-DB {code}: {result}"));
                    }
                }
                denied.shutdown().await.unwrap();
                let disabled = listen_with_prototype_options(
                    facts.clone(),
                    wire,
                    "127.0.0.1:0".parse().unwrap(),
                    "host=127.0.0.1 port=1 dbname=postgres".into(),
                    Some(config.clone()),
                    ServerOptions::default(),
                    false,
                )
                .await
                .unwrap();
                let token = disabled.keys.issue(1, 60);
                let result = post(disabled.address, "/apply", Some(&token), Some(&fp), &body).await;
                if result["code"] != "NOT_FOUND" {
                    failures.push(format!("{wire:?} {lang:?} opt-out: {result}"));
                }
                disabled.shutdown().await.unwrap();
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false;TRUNCATE {schema}.aip_idem")).await?;
                let server = listen_with_prototype_options(
                    facts.clone(),
                    wire,
                    "127.0.0.1:0".parse().unwrap(),
                    "host=localhost dbname=postgres".into(),
                    Some(config),
                    ServerOptions::default(),
                    true,
                )
                .await
                .unwrap();
                let token = server.keys.issue(1, 60);
                let first = post(server.address, "/apply", Some(&token), Some(&fp), &body).await;
                if first["ok"] != true || first["output"] != json!({"id":input,"count":1}) || first["contractFingerprint"] != fp {
                    failures.push(format!("{wire:?} {lang:?} first: {first}"));
                }
                let replay = post(server.address, "/apply", Some(&token), Some(&fp), &body).await;
                if replay["ok"] != true || replay["replayed"] != true || replay["output"] != first["output"] {
                    failures.push(format!("{wire:?} {lang:?} replay: {replay}"));
                }
                let status = post(server.address, "/status", Some(&token), Some(&fp), &json!({"key":"write-http","request":request})).await;
                if status["ok"] != true || status["output"] != first["output"] {
                    failures.push(format!("{wire:?} {lang:?} status: {status}"));
                }
                server.shutdown().await.unwrap();
                let row = db
                    .query_one(&format!("SELECT (SELECT count(*) FROM {schema}.event WHERE checked),(SELECT count(*) FROM {schema}.aip_idem)"), &[])
                    .await?;
                if row.get::<_, i64>(0) != 1 || row.get::<_, i64>(1) != 1 {
                    failures.push(format!("{wire:?} {lang:?} effect/result atomicity"));
                }
            }
        }
        Ok(())
    }
    .await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
