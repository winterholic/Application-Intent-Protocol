#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Sprint 13-A end to end: a real HTTP `/apply` against the real `aip service serve` binary on MySQL 8.4, which starts a
//! real Node or Python worker subprocess under the platform default isolation (Bubblewrap on Linux, Seatbelt on macOS).
//! The worker's `ctx.data.apply` callbacks, the audit rows and the idempotency result share one catalog-fenced MySQL
//! transaction; every failing action must leave the business rows untouched.
//!
//! Nothing here is scripted: no in-process session, no `ScriptedPeer`, no `Isolation::None`. The service is configured
//! the way an operator would (`workers.enable_writes`, a JWKS-verified bearer token bound to a principal, the contract
//! fingerprint from the listening event). Run through `scripts/verify-mysql-worker-e2e.sh`, which supplies the run-owned
//! MySQL fixture and a Linux environment where Bubblewrap can create namespaces.
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/mysql-worker-e2e/app.aip");
const MAPPING: &str = include_str!("../../../product/tests/fixtures/mysql-worker-e2e/mapping.json");
const SOURCE_SQL: &str = include_str!("../../../product/tests/fixtures/mysql-worker-e2e/source.sql");
const WORKER_MJS: &str = include_str!("../../../product/tests/fixtures/mysql-worker-e2e/extensions/worker.mjs");
const WORKER_PY: &str = include_str!("../../../product/tests/fixtures/mysql-worker-e2e/extensions/worker.py");
// Fixed schema names are safe: the fixture container is run-owned and nothing else writes to it.
const META: &str = "aip_e2e_meta";
const SOURCE: &str = "aip_e2e_src";
const ISSUER: &str = "https://issuer.example";
const HOOK_SECRET: &str = "e2e-webhook-secret-0123456789";

struct Env {
    adopt_url: String,
    runtime_url: String,
}

fn cli(env: &Env, args: &[&str], config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aip"));
    command.arg("service");
    if args.first() == Some(&"principal") {
        command.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        command.args(args).arg("--config").arg(config);
    }
    command.env("AIP_E2E_ADOPT_URL", &env.adopt_url).env("AIP_E2E_RUNTIME_URL", &env.runtime_url).env("AIP_E2E_HOOK_SECRET", HOOK_SECRET);
    command
}

async fn run(env: &Env, args: &[&str], config: &Path) -> Result<Value, Value> {
    let output = cli(env, args, config).output().await.expect("run aip");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).expect("stdout JSON"))
    } else {
        Err(serde_json::from_slice(&output.stderr).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&output.stderr)})))
    }
}

async fn start_server(env: &Env, file: &Path, config: &Path) -> Result<(Child, Value), Value> {
    let mut child = cli(env, &["serve", file.to_str().unwrap()], config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start aip serve");
    let mut line = String::new();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let count = tokio::time::timeout(Duration::from_secs(30), reader.read_line(&mut line)).await.expect("startup deadline").expect("read startup");
    if count == 0 {
        let output = child.wait_with_output().await.unwrap();
        return Err(serde_json::from_slice(&output.stderr).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&output.stderr)})));
    }
    tokio::spawn(async move { while reader.read_line(&mut String::new()).await.is_ok_and(|count| count > 0) {} });
    // An undrained stderr pipe would eventually block the server, and its events are the only record of why a request failed.
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    tokio::spawn(async move {
        let mut line = String::new();
        while stderr.read_line(&mut line).await.is_ok_and(|count| count > 0) {
            eprintln!("server: {}", line.trim_end());
            line.clear();
        }
    });
    Ok((child, serde_json::from_str(&line).expect("listening event")))
}

async fn post(address: &str, path: &str, token: Option<&str>, fingerprint: &str, body: &Value) -> Value {
    let text = body.to_string();
    let auth = token.map(|token| format!("Authorization: Bearer {token}\r\n")).unwrap_or_default();
    let contract = if fingerprint.is_empty() { String::new() } else { format!("x-aip-contract: {fingerprint}\r\n") };
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect");
    socket
        .write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\n{auth}{contract}Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(20), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
    let (_, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    serde_json::from_str(body).unwrap_or_else(|_| panic!("not JSON: {response}"))
}

/// One inbound webhook delivery as a provider sends it: the raw body plus an optional signature header. Returns the HTTP status.
async fn webhook(address: &str, path: &str, signature: Option<&str>, body: &str) -> (u16, Value) {
    let header = signature.map(|value| format!("X-Hook-Signature: {value}\r\n")).unwrap_or_default();
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect");
    socket
        .write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\n{header}Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(20), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
    let status = response.split_whitespace().nth(1).and_then(|status| status.parse().ok()).expect("status line");
    let (_, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    (status, serde_json::from_str(body).unwrap_or_else(|_| panic!("not JSON: {response}")))
}

/// A loopback HTTP peer for the `mail` integration: answers 200 and counts the requests that reached it. Returns its port.
async fn mail_peer(requests: Arc<AtomicUsize>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let requests = requests.clone();
            tokio::spawn(async move {
                let mut received = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let Ok(read) = socket.read(&mut chunk).await else { return };
                    if read == 0 {
                        return;
                    }
                    received.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&received).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase().strip_prefix("content-length:").map(|value| value.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break;
                        }
                    }
                }
                requests.fetch_add(1, Ordering::SeqCst);
                let body = r#"{"queued":true}"#;
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    port
}

fn hook_signature(body: &str) -> String {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(HOOK_SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", mac.finalize().into_bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

fn token(private: &RsaPrivateKey, subject: &str) -> String {
    let key = private.to_pkcs1_der().unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let issued = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    encode(
        &header,
        &json!({"iss": ISSUER, "aud": "aip-api", "sub": subject, "iat": issued, "nbf": issued, "exp": issued + 300}),
        &EncodingKey::from_rsa_der(key.as_bytes()),
    )
    .unwrap()
}

async fn run_script(conn: &mut aip_migrate::mysql_driver::mysql_async::Conn, script: &str) {
    for statement in script.split(";\n").map(str::trim).filter(|statement| !statement.is_empty()) {
        conn.query_drop(statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
}

fn code(response: &Value) -> &str {
    assert_eq!(response["ok"], false, "expected a failure: {response}");
    response["code"].as_str().unwrap_or_else(|| panic!("failure without code: {response}"))
}

/// Name of the table the run script creates in the fixture's admin database to prove it started this MySQL.
const OWNER_TABLE: &str = "aip_e2e_owner";

/// Fail closed unless the fixture proves it belongs to this run. A high-port loopback URL alone is not ownership:
/// the marker is random per run, written by `scripts/verify-mysql-worker-e2e.sh` into the container it just created, and
/// handed to this test through `AIP_E2E_FIXTURE_MARKER`. The test runs destructive DDL only after this returns Ok.
fn fixture_owner_decision(expected: Option<&str>, found: Result<Vec<String>, String>) -> Result<(), String> {
    let expected =
        expected.filter(|marker| !marker.is_empty()).ok_or("AIP_E2E_FIXTURE_MARKER is not set; run through scripts/verify-mysql-worker-e2e.sh")?;
    if expected.len() < 16 {
        return Err("AIP_E2E_FIXTURE_MARKER is too short to be a run marker".into());
    }
    let rows = found.map_err(|reason| format!("the fixture has no readable {OWNER_TABLE} marker ({reason}); it was not started by this run"))?;
    match rows.as_slice() {
        [marker] if marker == expected => Ok(()),
        [_] => Err("the fixture's owner marker does not match this run; refusing destructive DDL".into()),
        other => Err(format!("the fixture must hold exactly one owner marker, found {}", other.len())),
    }
}

async fn assert_fixture_owned(admin: &MysqlDriver, admin_url: &str) {
    let database = admin_url.split_once('@').and_then(|(_, rest)| rest.split_once('/')).map(|(_, tail)| tail.split('?').next().unwrap_or(""));
    let found = match database {
        Some(database) if !database.is_empty() => match admin.pool().get_conn().await {
            Ok(mut conn) => {
                conn.query::<String, _>(format!("SELECT marker FROM `{database}`.`{OWNER_TABLE}`")).await.map_err(|error| error.to_string())
            }
            Err(error) => Err(error.to_string()),
        },
        _ => Err("the admin URL names no database".into()),
    };
    if let Err(reason) = fixture_owner_decision(std::env::var("AIP_E2E_FIXTURE_MARKER").ok().as_deref(), found) {
        panic!("fixture ownership not proven, no DDL was run: {reason}");
    }
}

/// Polls a count until the asynchronous server loops have produced it. Jobs and relays run on the server's own schedule.
async fn wait_for(db: &mut Db, sql: &str, done: impl Fn(i64) -> bool, what: &str) {
    let mut last = 0;
    for _ in 0..300 {
        last = db.count(sql).await;
        if done(last) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}; last value {last}; query: {sql}");
}

struct Db(aip_migrate::mysql_driver::mysql_async::Conn);

impl Db {
    async fn count(&mut self, sql: &str) -> i64 {
        self.0.query_first::<i64, _>(sql).await.unwrap().unwrap()
    }
    async fn views(&mut self, id: i64) -> i64 {
        self.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id = {id}")).await
    }
    async fn audit(&mut self, key: &str) -> i64 {
        self.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_audit` WHERE request_key = '{key}'")).await
    }
    async fn snapshot(&mut self) -> [i64; 3] {
        [self.views(10).await, self.views(11).await, self.views(12).await]
    }
    async fn reset(&mut self) {
        self.0.query_drop(format!("UPDATE `{SOURCE}`.`video` SET views = 0")).await.unwrap();
        for table in ["aip_event_log", "aip_event_streams", "aip_event_keys"] {
            self.0.query_drop(format!("DELETE FROM `{META}`.`{table}` WHERE owner <> ''")).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a run-owned MySQL 8.4 fixture and a Linux host where Bubblewrap can create namespaces; run scripts/verify-mysql-worker-e2e.sh"]
async fn http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("app.aip");
    std::fs::write(&file, DEFINITION).unwrap();
    let extensions = directory.path().join("extensions");
    std::fs::create_dir(&extensions).unwrap();
    std::fs::write(extensions.join("worker.mjs"), WORKER_MJS).unwrap();
    std::fs::write(extensions.join("worker.py"), WORKER_PY).unwrap();

    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    assert_fixture_owned(&admin, &admin_url).await;
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
    setup.query_drop(format!("USE `{SOURCE}`")).await.unwrap();
    run_script(&mut setup, SOURCE_SQL).await;
    // The runtime account may read and write the business tables and the AIP schema, nothing more.
    setup.query_drop("DROP USER IF EXISTS 'aip_e2e_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_e2e_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_e2e_rt'@'%'")).await.unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env { adopt_url: admin_url.clone(), runtime_url: format!("mysql://aip_e2e_rt:runtime_test_only@{host}/{SOURCE}") };

    std::fs::write(directory.path().join("mapping.json"), MAPPING).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(
        directory.path().join("jwks.json"),
        json!({"keys": [{"kty": "RSA", "kid": "current", "alg": "RS256", "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()), "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]})
        .to_string(),
    )
    .unwrap();
    let mail_requests = Arc::new(AtomicUsize::new(0));
    let mail_port = mail_peer(mail_requests.clone()).await;
    let config_for = |language: &str| {
        let config = directory.path().join(format!("service-{language}.json"));
        std::fs::write(
            &config,
            json!({
                "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
                "database_url_env": "AIP_E2E_RUNTIME_URL", "migrate_database_url_env": "AIP_E2E_ADOPT_URL",
                "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
                "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
                "workers": {"directory": extensions, "language": language, "enable_writes": true,
                    "integrations": [{"name": "mail", "url": format!("http://127.0.0.1:{mail_port}/send"), "method": "POST", "timeout_ms": 2000,
                        "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true}]},
                // Server triggers act as the bound `svc` principal: Alice's account, so the hook may increment her videos.
                "schedules": [{"name": "tick", "operation": "tickJob", "cron": "* * * * *", "input": {"note": "tick"}, "principal": "svc"}],
                "webhooks": [{"name": "hook", "path": "/hooks/video", "operation": "hookJob", "principal": "svc", "secret_env": "AIP_E2E_HOOK_SECRET",
                    "signature": {"kind": "hmac-sha256", "header": "x-hook-signature", "prefix": "sha256="}}],
                "outbox_handlers": [{"topic": "video.viewed", "operation": "viewedJob", "principal": "svc"}],
            })
            .to_string(),
        )
        .unwrap();
        config
    };
    let node_config = config_for("node");
    let file_arg = file.to_str().unwrap();

    let plan = run(&env, &["adopt-external", file_arg], &node_config).await.expect("adoption plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &node_config).await.expect("adopt");
    // Alice owns videos 10 and 12, Bob owns 11. A valid token for an unbound subject must not act.
    for (subject, actor) in [("alice", "1"), ("bob", "2"), ("svc", "1")] {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &node_config).await.expect("bind");
    }
    let alice = token(&private, "alice");
    let mallory = token(&private, "mallory");
    let mut db = Db(admin.pool().get_conn().await.unwrap());

    for language in ["node", "python"] {
        db.reset().await;
        let (child, listening) = match start_server(&env, &file, &config_for(language)).await {
            Ok(started) => started,
            Err(error) => panic!("serve with a {language} worker did not start (a Linux host that denies Bubblewrap namespaces ends here): {error}"),
        };
        let address = listening["address"].as_str().unwrap().to_string();
        let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
        let apply = |token: Option<&str>, fingerprint: &str, key: String, request: Value| {
            let (address, fingerprint, token) = (address.clone(), fingerprint.to_string(), token.map(str::to_string));
            async move { post(&address, "/apply", token.as_deref(), &fingerprint, &json!({"key": key, "request": request})).await }
        };
        let view = |id: &str| json!({"extension": "Video.view", "input": {"id": id}});
        let key = |name: &str| format!("{name}-{language}");

        // Guest, stale contract and an unbound principal never reach a worker, a business row or an audit row.
        let guest = apply(None, &fingerprint, key("guest"), view("10")).await;
        assert_eq!(code(&guest), "WIRE.UNAUTHENTICATED", "{language}: {guest}");
        assert_eq!((db.snapshot().await, db.audit(&key("guest")).await), ([0, 0, 0], 0), "{language}: guest changed state");
        let stale = apply(Some(&alice), &"0".repeat(64), key("stale"), view("10")).await;
        assert_eq!(code(&stale), "WIRE.CONTRACT_MISMATCH", "{language}: {stale}");
        assert_eq!((db.snapshot().await, db.audit(&key("stale")).await), ([0, 0, 0], 0), "{language}: stale contract changed state");
        // A verified JWT whose subject has no aip_principals row: MysqlAuthenticator (mysql_service.rs) answers
        // AuthError::BadSignature, which the /apply transport (spike-v6 lib.rs) reports as WIRE.UNAUTHENTICATED.
        let unbound = apply(Some(&mallory), &fingerprint, key("unbound"), view("10")).await;
        assert_eq!(code(&unbound), "WIRE.UNAUTHENTICATED", "{language}: {unbound}");
        assert_eq!((db.snapshot().await, db.audit(&key("unbound")).await), ([0, 0, 0], 0), "{language}: unbound principal changed state");

        // Minimal success: one callback, valid output, +1 and one audit row in the same transaction.
        let first = apply(Some(&alice), &fingerprint, key("ok"), view("10")).await;
        assert_eq!(first["ok"], true, "{language}: {first}");
        assert_eq!(first["output"], json!({"count": 1}), "{language}: {first}");
        assert_eq!((db.views(10).await, db.audit(&key("ok")).await), (1, 1), "{language}");

        // Idempotent replay: same key and request returns the stored result and changes nothing.
        let replay = apply(Some(&alice), &fingerprint, key("ok"), view("10")).await;
        assert_eq!(replay["ok"], true, "{language}: {replay}");
        assert_eq!(replay["replayed"], true, "{language}: {replay}");
        assert_eq!(replay["output"], first["output"], "{language}");
        assert_eq!((db.views(10).await, db.audit(&key("ok")).await), (1, 1), "{language}");
        let conflict = apply(Some(&alice), &fingerprint, key("ok"), view("12")).await;
        assert_eq!(code(&conflict), "WIRE.IDEMPOTENCY_MISMATCH", "{language}: {conflict}");
        assert_eq!(db.views(12).await, 0, "{language}");

        // First callback succeeds on Alice's video 12, the second is Bob's video 11 and is refused: both roll back.
        let twice =
            apply(Some(&alice), &fingerprint, key("twice"), json!({"extension": "Video.twice", "input": {"first": "12", "second": "11"}})).await;
        println!("{language}: second callback refused -> {twice}");
        assert_eq!(code(&twice), "WIRE.EXTENSION_ERROR", "{language}: {twice}");
        assert_eq!((db.views(12).await, db.views(11).await, db.audit(&key("twice")).await), (0, 0, 0), "{language}: first callback leaked");

        // Output violating Int(0..10): the callback ran, the transaction must not commit.
        let bad = apply(Some(&alice), &fingerprint, key("bad"), json!({"extension": "Video.badOutput", "input": {"id": "12"}})).await;
        assert_eq!(code(&bad), "WIRE.OUTPUT_INVALID", "{language}: {bad}");
        assert_eq!((db.views(12).await, db.audit(&key("bad")).await), (0, 0), "{language}: invalid output committed");

        // A transition the extension does not declare is refused after a declared callback already applied.
        let undeclared = apply(Some(&alice), &fingerprint, key("undeclared"), json!({"extension": "Video.undeclared", "input": {"id": "12"}})).await;
        println!("{language}: undeclared grant -> {undeclared}");
        assert!(["WIRE.ACCESS_NOT_DECLARED", "WIRE.EXTENSION_ERROR"].contains(&code(&undeclared)), "{language}: {undeclared}");
        assert_eq!((db.views(12).await, db.audit(&key("undeclared")).await), (0, 0), "{language}: undeclared grant leaked a write");

        // None of the failures poisoned the service: a new key still commits.
        let after = apply(Some(&alice), &fingerprint, key("after"), view("12")).await;
        assert_eq!(after["ok"], true, "{language}: {after}");
        assert_eq!((db.views(12).await, db.audit(&key("after")).await), (1, 1), "{language}");

        // Durable HTTP jobs execute real workers. A retry replays its committed effect instead of incrementing twice.
        for operation in ["incrementJob", "retryJob", "nestedJob"] {
            let body = json!({"operation":operation,"input":{"id":"10"},"key":key(operation),"delayMs":0});
            let before = db.views(10).await;
            let accepted = post(&address, "/jobs/enqueue", Some(&alice), &fingerprint, &body).await;
            assert_eq!(accepted["ok"], true, "{language}/{operation}: {accepted}");
            let id = accepted["run"]["id"].as_str().unwrap();
            let duplicate = post(&address, "/jobs/enqueue", Some(&alice), &fingerprint, &body).await;
            assert_eq!(duplicate["run"]["id"], id, "same key converges on one durable run");
            let mut final_run = Value::Null;
            for _ in 0..200 {
                final_run = post(&address, "/jobs/status", Some(&alice), &fingerprint, &json!({"id":id})).await;
                assert_eq!(final_run["ok"], true, "{final_run}");
                if ["SUCCEEDED", "FAILED", "CANCELLED"].contains(&final_run["run"]["state"].as_str().unwrap()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(final_run["run"]["state"], "SUCCEEDED", "{language}/{operation}: {final_run}");
            assert_eq!(final_run["run"]["output"], Value::Null, "status never exposes an output without result authorization");
            if operation == "retryJob" {
                assert_eq!(final_run["run"]["attempt"], 2);
            }
            let result = post(&address, "/jobs/result", Some(&alice), &fingerprint, &json!({"id":id})).await;
            assert_eq!(result["output"], json!({"count":1}), "{language}: {result}");
            assert_eq!(db.views(10).await, before + 1, "committed effect is not repeated");
            assert_eq!(
                db.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_effects` WHERE run_id='{id}' AND state='SUCCEEDED'")).await,
                if operation == "nestedJob" { 2 } else { 1 }
            );
        }
        let delayed = post(
            &address,
            "/jobs/enqueue",
            Some(&alice),
            &fingerprint,
            &json!({"operation":"incrementJob","input":{"id":"10"},"key":key("cancel-job"),"delayMs":60000}),
        )
        .await;
        assert_eq!(delayed["ok"], true, "{delayed}");
        let cancelled = post(&address, "/jobs/cancel", Some(&alice), &fingerprint, &json!({"id":delayed["run"]["id"]})).await;
        assert_eq!(cancelled["run"]["state"], "CANCELLED", "{cancelled}");
        let slow = post(
            &address,
            "/jobs/enqueue",
            Some(&alice),
            &fingerprint,
            &json!({"operation":"slowJob","input":{},"key":key("running-cancel"),"delayMs":0}),
        )
        .await;
        assert_eq!(slow["ok"], true, "{slow}");
        let slow_id = slow["run"]["id"].clone();
        let mut running = Value::Null;
        for _ in 0..100 {
            running = post(&address, "/jobs/status", Some(&alice), &fingerprint, &json!({"id":slow_id})).await;
            if running["run"]["state"] == "RUNNING" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(running["run"]["state"], "RUNNING", "{running}");
        let cancel = post(&address, "/jobs/cancel", Some(&alice), &fingerprint, &json!({"id":slow_id})).await;
        assert_eq!(cancel["ok"], true, "{cancel}");
        for _ in 0..100 {
            running = post(&address, "/jobs/status", Some(&alice), &fingerprint, &json!({"id":slow_id})).await;
            if running["run"]["state"] == "CANCELLED" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(running["run"]["state"], "CANCELLED", "{language}: cancellation must follow the observed worker stop: {running}");

        // Server triggers start the same durable jobs as the bound `svc` principal, and each source is verified by the server.
        // Signed webhook: a missing or wrong signature never reaches the job store; a good delivery is accepted once.
        let hook_body = json!({"id": "10", "language": language}).to_string();
        let hook_jobs = format!("SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE operation = 'hookJob'");
        let hook_before = db.count(&hook_jobs).await;
        let (status, unsigned) = webhook(&address, "/hooks/video", None, &hook_body).await;
        assert_eq!((status, code(&unsigned)), (401, "WIRE.SIGNATURE_INVALID"), "{language}: {unsigned}");
        let (status, forged) = webhook(&address, "/hooks/video", Some("sha256=00"), &hook_body).await;
        assert_eq!((status, code(&forged)), (401, "WIRE.SIGNATURE_INVALID"), "{language}: {forged}");
        assert_eq!(db.count(&hook_jobs).await, hook_before, "{language}: a rejected delivery created a job");
        let views_before = db.views(10).await;
        let signature = hook_signature(&hook_body);
        let (status, delivered) = webhook(&address, "/hooks/video", Some(&signature), &hook_body).await;
        assert_eq!((status, delivered["ok"].clone()), (202, json!(true)), "{language}: {delivered}");
        let (status, redelivered) = webhook(&address, "/hooks/video", Some(&signature), &hook_body).await;
        assert_eq!((status, &redelivered["id"]), (202, &delivered["id"]), "{language}: a redelivery converges on one job");
        let hook_id = delivered["id"].as_str().unwrap().to_string();
        let hook_done = format!("SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE id = '{hook_id}' AND state = 'SUCCEEDED'");
        wait_for(&mut db, &hook_done, |count| count == 1, "the webhook job to succeed").await;
        assert_eq!(db.count(&hook_jobs).await, hook_before + 1, "{language}: one delivery, one job");
        assert_eq!(db.views(10).await, views_before + 1, "{language}: the worker incremented through the service principal");
        // A user cannot reach the trigger-fed operations or the server-minted key namespaces.
        let forged_job = post(
            &address,
            "/jobs/enqueue",
            Some(&alice),
            &fingerprint,
            &json!({"operation": "hookJob", "input": {"body": "{\"id\":\"10\"}"}, "key": key("forged-hook"), "delayMs": 0}),
        )
        .await;
        assert_eq!(code(&forged_job), "WIRE.ACCESS_DENIED", "{language}: {forged_job}");
        let claimed_key = post(
            &address,
            "/jobs/enqueue",
            Some(&alice),
            &fingerprint,
            &json!({"operation": "incrementJob", "input": {"id": "10"}, "key": "webhook:hook:claimed", "delayMs": 0}),
        )
        .await;
        assert_eq!(code(&claimed_key), "WIRE.BAD_VALUE", "{language}: {claimed_key}");
        // Schedule: the current slot ran once, however many servers or restarts evaluated it.
        let ticks = format!(
            "SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE operation = 'tickJob' AND state = 'SUCCEEDED' AND idempotency_key LIKE 'schedule:tick:%'"
        );
        wait_for(&mut db, &ticks, |count| count >= 1, "a schedule slot to run").await;
        let duplicate_slots = format!("SELECT COUNT(*) - COUNT(DISTINCT idempotency_key) FROM `{META}`.`aip_jobs` WHERE operation = 'tickJob'");
        assert_eq!(db.count(&duplicate_slots).await, 0, "{language}: one job per slot");
        // Outbox: every committed `notify` row became exactly one job and one receipt, and the worker saw the event.
        let undelivered = format!(
            "SELECT (SELECT COUNT(*) FROM `{META}`.`aip_outbox` WHERE topic = 'video.viewed') - (SELECT COUNT(*) FROM `{META}`.`aip_outbox_dispatch`)"
        );
        wait_for(&mut db, &undelivered, |count| count == 0, "every outbox row to be relayed").await;
        let viewed = format!(
            "SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE operation = 'viewedJob' AND state = 'SUCCEEDED' AND JSON_EXTRACT(output, '$.count') = 1"
        );
        let receipts = format!("SELECT COUNT(*) FROM `{META}`.`aip_outbox_dispatch`");
        wait_for(&mut db, &format!("SELECT ({viewed}) - ({receipts})"), |gap| gap == 0, "every relayed event to be handled").await;
        assert!(db.count(&receipts).await >= 1, "{language}: the earlier increments must have produced outbox rows");

        // An outbound HTTP call is journaled: the retry after the call completed replays its answer and sends nothing.
        let mail_before = mail_requests.load(Ordering::SeqCst);
        let mail = post(
            &address,
            "/jobs/enqueue",
            Some(&alice),
            &fingerprint,
            &json!({"operation": "mailJob", "input": {"id": "10"}, "key": key("mail"), "delayMs": 0}),
        )
        .await;
        assert_eq!(mail["ok"], true, "{language}: {mail}");
        let mail_id = mail["run"]["id"].as_str().unwrap().to_string();
        let mail_done = format!("SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE id = '{mail_id}' AND state = 'SUCCEEDED'");
        wait_for(&mut db, &mail_done, |count| count == 1, "the mail job to succeed").await;
        let mail_attempts = format!("SELECT attempt FROM `{META}`.`aip_jobs` WHERE id = '{mail_id}'");
        assert_eq!(db.count(&mail_attempts).await, 2, "{language}: the first attempt asked to be retried");
        assert_eq!(mail_requests.load(Ordering::SeqCst) - mail_before, 1, "{language}: the retry must replay the recorded call");
        let mail_effects = format!("SELECT COUNT(*) FROM `{META}`.`aip_effects` WHERE run_id = '{mail_id}' AND state = 'SUCCEEDED'");
        assert_eq!(db.count(&mail_effects).await, 1, "{language}: one journaled call");

        // A published event is journaled too: the retry replays the cursor, appends nothing, and the client reads it back.
        let event = post(
            &address,
            "/jobs/enqueue",
            Some(&alice),
            &fingerprint,
            &json!({"operation": "eventJob", "input": {"id": "10"}, "key": key("event"), "delayMs": 0}),
        )
        .await;
        assert_eq!(event["ok"], true, "{language}: {event}");
        let event_id = event["run"]["id"].as_str().unwrap().to_string();
        let event_done = format!("SELECT COUNT(*) FROM `{META}`.`aip_jobs` WHERE id = '{event_id}' AND state = 'SUCCEEDED'");
        wait_for(&mut db, &event_done, |count| count == 1, "the event job to succeed").await;
        let event_attempts = format!("SELECT attempt FROM `{META}`.`aip_jobs` WHERE id = '{event_id}'");
        assert_eq!(db.count(&event_attempts).await, 2, "{language}: the first attempt asked to be retried");
        assert_eq!(
            db.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_event_log` WHERE stream = 'audit'")).await,
            1,
            "{language}: the retry must not append again"
        );
        let read = post(&address, "/events/read", Some(&alice), &fingerprint, &json!({"stream": "audit", "after": "0", "limit": 10})).await;
        assert_eq!(read["ok"], true, "{language}: {read}");
        assert_eq!((read["events"][0]["payload"].clone(), read["cursor"].clone()), (json!({"video": "10"}), json!("1")), "{language}: {read}");
        drop(child);
    }
}

#[cfg(test)]
mod ownership_guard {
    use super::*;

    const MARKER: &str = "aip-e2e-run-0123456789abcdef";

    #[test]
    fn only_the_runs_own_marker_proves_ownership() {
        assert!(fixture_owner_decision(Some(MARKER), Ok(vec![MARKER.into()])).is_ok());
    }

    #[test]
    fn missing_short_wrong_or_unreadable_markers_fail_closed() {
        assert!(fixture_owner_decision(None, Ok(vec![MARKER.into()])).is_err(), "marker env not set");
        assert!(fixture_owner_decision(Some(""), Ok(vec![String::new()])).is_err(), "empty marker env");
        assert!(fixture_owner_decision(Some("short"), Ok(vec!["short".into()])).is_err(), "short marker");
        assert!(fixture_owner_decision(Some(MARKER), Err("table does not exist".into())).is_err(), "another fixture without the table");
        assert!(fixture_owner_decision(Some(MARKER), Ok(vec![])).is_err(), "table without a row");
        assert!(fixture_owner_decision(Some(MARKER), Ok(vec!["aip-e2e-run-other-run-marker".into()])).is_err(), "marker of another run");
        assert!(fixture_owner_decision(Some(MARKER), Ok(vec![MARKER.into(), MARKER.into()])).is_err(), "more than one marker row");
    }

    /// Source-order guard: the proof runs before the first destructive statement of the live test.
    #[test]
    fn the_proof_precedes_every_destructive_statement() {
        let source = include_str!("mysql_worker_write_e2e.rs");
        let live = &source[source.find("async fn http_apply_runs_real").unwrap()..source.find("#[cfg(test)]").unwrap()];
        let proof = live.find("assert_fixture_owned(&admin").expect("the live test must prove ownership");
        for statement in ["DROP DATA", "CREATE DATA", "DROP USER", "CREATE USER", "GRANT "] {
            assert!(proof < live.find(statement).expect(statement), "{statement} must come after the ownership proof");
        }
    }
}
