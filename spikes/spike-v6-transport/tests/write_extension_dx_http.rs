//! A PostgreSQL write extension over HTTP, against a real PostgreSQL and the official Node and Python workers:
//! C1 the string key of `ctx.data.apply`, X1 the cause of a failed extension (log class always, response only in development mode),
//! X2 `ctx.reject` as `WIRE.EXTENSION_REJECTED` with the business code, rolled back, and no idempotency record.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
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

const SPEC: &str = include_str!("fixtures/write-extension-dx.aip");
const NODE: &str = r#"
export async function keyed(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id], 'mark-1'); return { ok: true }; }
export async function no_access(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); await ctx.data.apply('Event', 'other', [input.id]); return { ok: true }; }
export async function bad_filter(input, ctx) {
  await ctx.data.apply('Event', 'mark', [input.id]);
  await ctx.data.read({ read: 'Event', select: ['id'], filter: [{ field: 'member', op: 'eq', value: '1' }], limit: 1 });
  return { ok: true };
}
export async function gate(input, ctx) {
  const page = await ctx.data.read({ read: 'Event', select: ['id', 'checked'], filter: [{ field: 'id', op: 'eq', value: String(input.id) }], limit: 1 });
  if (!page.rows[0].checked) throw ctx.reject('NOT_READY', '아직 준비되지 않았습니다');
  await ctx.data.apply('Event', 'other', [input.id]);
  return { ok: true };
}
export async function crashes(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); throw new Error('boom: secret-token-123'); }
"#;
const PYTHON: &str = r#"
async def keyed(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']], 'mark-1')
    return {'ok': True}

async def no_access(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    await ctx.data.apply('Event', 'other', [input['id']])
    return {'ok': True}

async def bad_filter(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    await ctx.data.read({'read': 'Event', 'select': ['id'], 'filter': [{'field': 'member', 'op': 'eq', 'value': '1'}], 'limit': 1})
    return {'ok': True}

async def gate(input, ctx):
    page = await ctx.data.read({'read': 'Event', 'select': ['id', 'checked'], 'filter': [{'field': 'id', 'op': 'eq', 'value': str(input['id'])}], 'limit': 1})
    if not page['rows'][0]['checked']:
        raise ctx.reject('NOT_READY', '아직 준비되지 않았습니다')
    await ctx.data.apply('Event', 'other', [input['id']])
    return {'ok': True}

async def crashes(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    raise RuntimeError('boom: secret-token-123')
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn post(address: SocketAddr, token: &str, fp: &str, body: &Value) -> Value {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let body = body.to_string();
    socket
        .write_all(
            format!("POST /apply HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = vec![];
    timeout(Duration::from_secs(8), socket.read_to_end(&mut bytes)).await.unwrap().unwrap();
    let response = String::from_utf8(bytes).unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn string_key_causes_and_business_rejection_over_http() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-write-dx-http-{unique}"));
    fs::create_dir(&path).unwrap();
    let owned = OwnedDir(path);
    fs::write(owned.0.join("dx.mjs"), NODE).unwrap();
    fs::write(owned.0.join("dx.py"), PYTHON).unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!("aip_write_dx_http_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_with_url(aip_test_support::test_database_url!()).await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures: Vec<String> = vec![];
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false)"))
            .await?;
        let wire = IdWire::DecimalString;
        let fp = contract_fingerprint_with_all_extensions(&facts, wire);
        let checked = || format!("SELECT checked FROM {schema}.event WHERE id=11");
        let idem = || format!("SELECT count(*) FROM {schema}.aip_idem");
        for lang in [WorkerLang::Node, WorkerLang::Python] {
            for developer in [false, true] {
                let config = ReadExtensions { lang, dir: owned.0.clone() };
                let server = listen_with_prototype_options(
                    facts.clone(),
                    wire,
                    "127.0.0.1:0".parse().unwrap(),
                    aip_test_support::test_database_url!().into(),
                    Some(config),
                    ServerOptions { developer_diagnostics: developer, ..ServerOptions::default() },
                    true,
                )
                .await
                .unwrap();
                let token = server.keys.issue(1, 60);
                let tag = format!("{lang:?} dev={developer}");
                let call = |name: &str, key: &str| json!({"key": key, "request": {"extension": format!("Event.{name}"), "input": {"id": "11"}}});
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;

                // C1: the documented signature with a string key runs the transition and commits with the stored result.
                let keyed = post(server.address, &token, &fp, &call("keyed", "k-keyed")).await;
                if keyed["ok"] != true || keyed["output"] != json!({"ok": true}) {
                    failures.push(format!("{tag} keyed: {keyed}"));
                }
                if !db.query_one(&checked(), &[]).await?.get::<_, bool>(0) || db.query_one(&idem(), &[]).await?.get::<_, i64>(0) != 1 {
                    failures.push(format!("{tag} keyed: effect and idempotency record are not both committed"));
                }
                let replay = post(server.address, &token, &fp, &call("keyed", "k-keyed")).await;
                if replay["replayed"] != true {
                    failures.push(format!("{tag} keyed replay: {replay}"));
                }

                // X1: refused calls and raising extensions. Nothing is committed; the response names the cause only in development mode.
                for (name, cause) in
                    [("noAccess", "ctx.apply.access_not_declared"), ("badFilter", "ctx.read.filter_not_exposed"), ("crashes", "extension.exception")]
                {
                    db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;
                    let failed = post(server.address, &token, &fp, &call(name, &format!("k-{name}"))).await;
                    let body = failed.to_string();
                    let shaped = failed["ok"] == false
                        && failed["code"] == "WIRE.EXTENSION_ERROR"
                        && failed["msg"] == "공식 WRITE 확장 호출 거부"
                        && failed["retry"] == "never"
                        && failed.get("appCode").is_none();
                    let named = if developer {
                        failed["cause"] == cause && failed["detail"].is_string()
                    } else {
                        failed.get("cause").is_none() && failed.get("detail").is_none()
                    };
                    if !shaped || !named {
                        failures.push(format!("{tag} {name}: {failed}"));
                    }
                    if !developer && (body.contains("filter") || body.contains("secret-token-123") || body.contains(cause)) {
                        failures.push(format!("{tag} {name}: production response leaks internals: {failed}"));
                    }
                    if developer && name == "crashes" && !failed["detail"].as_str().unwrap_or("").contains("boom") {
                        failures.push(format!("{tag} {name}: development detail lacks the exception text: {failed}"));
                    }
                    if db.query_one(&checked(), &[]).await?.get::<_, bool>(0) || db.query_one(&idem(), &[]).await?.get::<_, i64>(0) != 0 {
                        failures.push(format!("{tag} {name}: a failed extension left a write or an idempotency record"));
                    }
                }

                // X2: a business refusal: appCode and text reach the caller, the call is rolled back, no idempotency record remains, and the
                // same key sent again judges the state as it is then.
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;
                let gate = call("gate", "k-gate");
                let rejected = post(server.address, &token, &fp, &gate).await;
                let ok_shape = rejected["ok"] == false
                    && rejected["code"] == "WIRE.EXTENSION_REJECTED"
                    && rejected["appCode"] == "NOT_READY"
                    && rejected["msg"] == "아직 준비되지 않았습니다"
                    && rejected["retry"] == "never"
                    && rejected.get("cause").is_none();
                if !ok_shape || db.query_one(&idem(), &[]).await?.get::<_, i64>(0) != 0 {
                    failures.push(format!("{tag} gate rejection: {rejected}"));
                }
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=true")).await?;
                let accepted = post(server.address, &token, &fp, &gate).await;
                if accepted["ok"] != true || accepted.get("replayed").is_some() || db.query_one(&idem(), &[]).await?.get::<_, i64>(0) != 1 {
                    failures.push(format!("{tag} same key after the rejection must run again: {accepted}"));
                }
                server.shutdown().await.unwrap();
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

/// X1, the server log: one process runs the failing calls (`log_child`), this one reads its stderr. The request log is written by
/// a writer thread to the process stderr, so a test thread cannot capture it; a child process can be read.
#[test]
fn the_request_log_names_the_cause_class_and_never_the_exception_text() {
    if std::env::var_os("AIP_DX_LOG_CHILD").is_some() {
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["log_child", "--exact", "--nocapture", "--test-threads=1"])
        .env("AIP_DX_LOG_CHILD", "1")
        .env_remove("AIP_REQUEST_LOG")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() && stdout.contains("CHILD_DONE"), "child failed:\n{stdout}\n{stderr}");
    let bodies: Vec<Value> =
        stdout.lines().filter_map(|line| line.split_once("BODY ")).map(|(_, body)| serde_json::from_str(body).unwrap()).collect();
    assert_eq!(bodies.len(), 3, "{stdout}");
    let lines: Vec<Value> =
        stderr.lines().filter_map(|line| serde_json::from_str(line).ok()).filter(|line: &Value| line["event"] == "request").collect();
    let line_of = |body: &Value| {
        lines.iter().find(|line| line["requestId"] == body["requestId"]).unwrap_or_else(|| panic!("no log line for {body}\n{stderr}")).clone()
    };
    let expected = [
        ("WIRE.EXTENSION_ERROR", "ctx.apply.access_not_declared"),
        ("WIRE.EXTENSION_ERROR", "extension.exception"),
        ("WIRE.EXTENSION_REJECTED", "rejected:NOT_READY"),
    ];
    for (body, (code, cause)) in bodies.iter().zip(expected) {
        let line = line_of(body);
        assert_eq!((line["code"].as_str(), line["cause"].as_str()), (Some(code), Some(cause)), "{line}");
        assert!(body.get("cause").is_none(), "the production response must not carry the cause: {body}");
    }
    for secret in ["secret-token-123", "boom", "Event.other"] {
        assert!(
            !stderr.lines().any(|line| line.contains("\"event\":\"request\"") && line.contains(secret)),
            "a request log line leaks `{secret}`:\n{stderr}"
        );
    }
}

#[tokio::test]
async fn log_child() {
    if std::env::var_os("AIP_DX_LOG_CHILD").is_none() {
        return;
    }
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-write-dx-log-{unique}"));
    fs::create_dir(&path).unwrap();
    let owned = OwnedDir(path);
    fs::write(owned.0.join("dx.mjs"), NODE).unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!("aip_write_dx_log_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_with_url(aip_test_support::test_database_url!()).await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false)"))
            .await?;
        let wire = IdWire::DecimalString;
        let fp = contract_fingerprint_with_all_extensions(&facts, wire);
        let server = listen_with_prototype_options(
            facts.clone(),
            wire,
            "127.0.0.1:0".parse().unwrap(),
            aip_test_support::test_database_url!().into(),
            Some(ReadExtensions { lang: WorkerLang::Node, dir: owned.0.clone() }),
            ServerOptions::default(),
            true,
        )
        .await
        .unwrap();
        let token = server.keys.issue(1, 60);
        for (name, key) in [("noAccess", "log-1"), ("crashes", "log-2"), ("gate", "log-3")] {
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await?;
            let body =
                post(server.address, &token, &fp, &json!({"key": key, "request": {"extension": format!("Event.{name}"), "input": {"id": "11"}}}))
                    .await;
            println!("BODY {body}");
        }
        server.shutdown().await.unwrap();
        // The log line of a request is written after its reply; give the writer thread time to flush.
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }
    .await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "cleanup: {cleanup:?}");
    println!("CHILD_DONE");
}
