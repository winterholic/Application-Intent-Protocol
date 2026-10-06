#![cfg(target_os = "macos")]
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_owned_with_url,
    id_wire::{emit_id, IdWire},
    sqlgen,
};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_options, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{
    fs,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::timeout,
};

// Relay actual authentication and SQL. Hold only a successful COMMIT reply, after PostgreSQL commits.
async fn hold_commit(socket: TcpStream, remaining: Arc<AtomicUsize>, held: Arc<AtomicUsize>, allow_commit: bool, after_clock: bool) {
    let db = TcpStream::connect("127.0.0.1:5432").await.unwrap();
    let (mut input, mut output) = socket.into_split();
    let (mut db_input, mut db_output) = db.into_split();
    let frontend = async { tokio::io::copy(&mut input, &mut db_output).await };
    let backend = async {
        let mut holding = false;
        let mut clock_row = false;
        loop {
            let kind = db_input.read_u8().await?;
            let length = db_input.read_u32().await?;
            if !(4..=1_048_576).contains(&length) {
                return Err(std::io::Error::other("invalid PG packet"));
            }
            let mut bytes = vec![0; (length - 4) as usize];
            db_input.read_exact(&mut bytes).await?;
            if allow_commit
                && kind == b'C'
                && bytes == b"COMMIT\0"
                && remaining.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
            {
                holding = true;
                held.fetch_add(1, Ordering::SeqCst);
            }
            if !holding {
                output.write_u8(kind).await?;
                output.write_u32(length).await?;
                output.write_all(&bytes).await?;
            }
            if kind == b'D' {
                clock_row = true;
            }
            if after_clock && clock_row && kind == b'Z' {
                holding = true;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    };
    tokio::select! {_=frontend=>{},_=backend=>{}}
}
async fn post(address: SocketAddr, token: &str, fp: &str, path: &str, body: &Value) -> Result<Value, String> {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.map_err(|e| e.to_string())?;
    socket.write_all(format!("POST {path} HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.map_err(|e|e.to_string())?;
    let mut bytes = vec![];
    timeout(Duration::from_secs(4), socket.read_to_end(&mut bytes))
        .await
        .map_err(|_| "commit response remained blocked".to_string())?
        .map_err(|e| e.to_string())?;
    let response = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    serde_json::from_str(response.split_once("\r\n\r\n").ok_or("missing body")?.1).map_err(|e| e.to_string())
}

#[tokio::test]
async fn committed_but_missing_reply_stays_unknown_and_recovers_without_duplicate_effects() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-commit-lifetime-{unique}"));
    fs::create_dir(&path).unwrap();
    fs::write(path.join("write.mjs"),"import fs from 'node:fs';export async function run(input,ctx){fs.appendFileSync(new URL('./counter',import.meta.url),'1\\n');const r=await ctx.data.apply('Event','mark',[input.id]);return {id:r.changed[0]??r.unchanged[0],count:1};}\n").unwrap();
    let facts = load_str(include_str!("fixtures/write-extension.aip"), Form::A).unwrap().execution;
    let schema = format!("aip_commit_life_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_owned_with_url("host=localhost dbname=postgres").await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let remaining = Arc::new(AtomicUsize::new(0));
    let held = Arc::new(AtomicUsize::new(0));
    let (r, h) = (remaining.clone(), held.clone());
    let sdk_case = Arc::new(AtomicBool::new(false));
    let connections = Arc::new(AtomicUsize::new(0));
    let (s, n) = (sdk_case.clone(), connections.clone());
    let proxy = tokio::spawn(async move {
        let mut peers = JoinSet::new();
        loop {
            tokio::select! {
                socket=listener.accept()=>{let(socket,_)=socket.unwrap();let number=n.fetch_add(1,Ordering::SeqCst)+1;let sdk=s.load(Ordering::SeqCst);peers.spawn(hold_commit(socket,r.clone(),h.clone(),!sdk||number==2,sdk&&number==3));}
                _=peers.join_next(),if !peers.is_empty()=>{}
            }
        }
    });
    let mut failures = vec![];
    let result: Result<(), tokio_postgres::Error> = async {
        for statement in &ddl[1..] {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1);INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false)"))
            .await?;
        for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
            sdk_case.store(false, Ordering::SeqCst);
            let server = listen_with_prototype_options(
                facts.clone(),
                wire,
                "127.0.0.1:0".parse().unwrap(),
                format!("host=127.0.0.1 port={} dbname=postgres", address.port()),
                Some(ReadExtensions { lang: WorkerLang::Node, dir: path.clone() }),
                ServerOptions::default(),
                true,
            )
            .await
            .unwrap();
            let token = server.keys.issue(1, 60);
            let fp = contract_fingerprint_with_all_extensions(&facts, wire);
            for extension in [false, true] {
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false;TRUNCATE {schema}.aip_idem")).await?;
                fs::write(path.join("counter"), "").unwrap();
                let id = emit_id(11, wire).unwrap();
                let request =
                    if extension { json!({"extension":"Event.run","input":{"id":id}}) } else { json!({"apply":"Event.mark","target":{"ids":[id]}}) };
                let body = json!({"key":"held-commit","request":request});
                remaining.store(1, Ordering::SeqCst);
                let first = post(server.address, &token, &fp, "/apply", &body).await;
                if !matches!(&first,Ok(v) if v["code"]=="COMMIT_UNKNOWN") {
                    failures.push(format!("{wire:?} extension={extension} first: {first:?}"));
                }
                if remaining.load(Ordering::SeqCst) != 0 {
                    failures.push("PostgreSQL did not produce a successful COMMIT reply".to_string());
                }
                let row =
                    db.query_one(&format!("SELECT checked,(SELECT count(*) FROM {schema}.aip_idem) FROM {schema}.event WHERE id=11"), &[]).await?;
                if !row.get::<_, bool>(0) || row.get::<_, i64>(1) != 1 {
                    failures.push(format!("{wire:?} extension={extension} committed effect/result absent"));
                }
                let status = post(server.address, &token, &fp, "/status", &body).await;
                if !matches!(&status,Ok(v) if v["ok"]==true&&v["replayed"]==true) {
                    failures.push(format!("{wire:?} status: {status:?}"));
                }
                let replay = post(server.address, &token, &fp, "/apply", &body).await;
                if !matches!(&replay,Ok(v) if v["ok"]==true&&v["replayed"]==true) {
                    failures.push(format!("{wire:?} replay: {replay:?}"));
                }
                if fs::read_to_string(path.join("counter")).unwrap().lines().count() != usize::from(extension) {
                    failures.push(format!("{wire:?} duplicate worker invocation"));
                }
            }
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false;TRUNCATE {schema}.aip_idem")).await?;
            fs::write(path.join("counter"), "").unwrap();
            connections.store(0, Ordering::SeqCst);
            sdk_case.store(true, Ordering::SeqCst);
            remaining.store(1, Ordering::SeqCst);
            let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prototype/sdk/index.ts").canonicalize().unwrap();
            let import = serde_json::to_string(sdk.to_str().unwrap()).unwrap();
            let module = spike_v5_sdk::contract_module_with_all_extensions(&facts, sdk.to_str().unwrap(), wire);
            fs::write(path.join("contract.ts"), module).unwrap();
            fs::write(path.join("package.json"), "{\"type\":\"module\",\"private\":true}").unwrap();
            let id = emit_id(11, wire).unwrap().to_string();
            let script = format!(
                r#"import assert from 'node:assert/strict';
import {{connect,WriteUnsettled}} from {import};
import {{contract}} from './contract.ts';
const client=connect(process.env.AIP_COMMIT_TEST_URL,process.env.AIP_COMMIT_TEST_TOKEN,contract);
const query={{read:'Event',select:['id','checked']}};
await client.read(query);assert.equal((await client.read(query)).cached,true);
await assert.rejects(client.writeExtension('Event.run',{{id:{id}}},{{key:'sdk-recovery',retries:0}}),WriteUnsettled);
assert.deepEqual(client.pending(),['sdk-recovery']);
await assert.rejects(client.writeExtension('Event.run',{{id:{id}}},{{key:'sdk-recovery',retries:0}}),WriteUnsettled);
assert.deepEqual(client.pending(),['sdk-recovery']);
assert.equal((await client.read(query)).cached,false);assert.equal((await client.read(query)).cached,false);
const recovered=await client.retryPending();assert.equal(recovered[0].replayed,true);assert.deepEqual(client.pending(),[]);
const after=await client.read(query);assert.equal(after.rows[0].checked,true);assert.equal((await client.read(query)).cached,true);
console.log(JSON.stringify({{ok:true,recovered:true,pendingPreserved:true}}));
"#
            );
            fs::write(path.join("recover.mts"), script).unwrap();
            let mut node = tokio::process::Command::new("node");
            node.args(["--experimental-strip-types", path.join("recover.mts").to_str().unwrap()])
                .env("AIP_COMMIT_TEST_URL", format!("http://{}", server.address))
                .env("AIP_COMMIT_TEST_TOKEN", &token)
                .kill_on_drop(true);
            match timeout(Duration::from_secs(10), node.output()).await {
                Ok(Ok(output)) if output.status.success() => {}
                Ok(Ok(output)) => failures.push(format!("{wire:?} actual SDK retry lost pending: {}", String::from_utf8_lossy(&output.stderr))),
                other => failures.push(format!("{wire:?} actual SDK process: {other:?}")),
            }
            if fs::read_to_string(path.join("counter")).unwrap().lines().count() != 1 {
                failures.push(format!("{wire:?} SDK recovery duplicated worker"));
            }
            let records = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await?.get::<_, i64>(0);
            if records != 1 {
                failures.push(format!("{wire:?} SDK recovery records={records}"));
            }
            server.shutdown().await.unwrap();
        }
        Ok(())
    }
    .await;
    proxy.abort();
    let _ = proxy.await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    fs::remove_dir_all(path).unwrap();
    assert!(result.is_ok(), "owned test setup: {result:?}");
    assert!(cleanup.is_ok(), "cleanup: {cleanup:?}");
    assert_eq!(held.load(Ordering::SeqCst), 6);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
