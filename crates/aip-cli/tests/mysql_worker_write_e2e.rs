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
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
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
    command.env("AIP_E2E_ADOPT_URL", &env.adopt_url).env("AIP_E2E_RUNTIME_URL", &env.runtime_url);
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
    async fn reset(&mut self) {
        self.0.query_drop(format!("UPDATE `{SOURCE}`.`video` SET views = 0")).await.unwrap();
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
    let config_for = |language: &str| {
        let config = directory.path().join(format!("service-{language}.json"));
        std::fs::write(
            &config,
            json!({
                "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
                "database_url_env": "AIP_E2E_RUNTIME_URL", "migrate_database_url_env": "AIP_E2E_ADOPT_URL",
                "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
                "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
                "workers": {"directory": extensions, "language": language, "enable_writes": true},
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
    for (subject, actor) in [("alice", "1"), ("bob", "2")] {
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

        // Guest, stale contract and an unbound principal never reach a worker or a business row.
        let guest = apply(None, &fingerprint, key("guest"), view("10")).await;
        assert_eq!(code(&guest), "WIRE.UNAUTHENTICATED", "{language}: {guest}");
        let stale = apply(Some(&alice), "stale-contract", key("stale"), view("10")).await;
        assert_eq!(code(&stale), "WIRE.CONTRACT_MISMATCH", "{language}: {stale}");
        let unbound = apply(Some(&mallory), &fingerprint, key("unbound"), view("10")).await;
        println!("{language}: unbound principal -> {unbound}");
        assert_eq!(unbound["ok"], false, "{language}: an unbound subject must not run a write action: {unbound}");
        assert_eq!((db.views(10).await, db.audit(&key("guest")).await, db.audit(&key("unbound")).await), (0, 0, 0), "{language}");

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
        drop(child);
    }
}
