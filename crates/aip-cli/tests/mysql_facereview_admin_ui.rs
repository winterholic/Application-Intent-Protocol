#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.9 / 7.2.10: the FaceReview administrator console through the real `aip service` binary, the generated TypeScript binding and
//! (when the Verify workflow supplies the pinned admin UI and Playwright) the real React console in a browser.
//!
//! The service is the password-login deployment of `mysql_facereview_auth` plus the write workers the approval job needs, on the real MySQL 8.4
//! schema of the original (pinned new-facereview@7948b7e7) and a run-owned MongoDB. Before any browser starts, the server's own policy is
//! checked over HTTP: a member and a guest are refused every administrator aggregate and the approval job. The browser journey then
//! logs in as the fixture SUPER_ADMIN, reads the lists and the dashboard, approves one request (a durable job: video row, two MongoDB
//! documents, ACCEPTED request) and rejects another, and shows that a non-administrator login is refused by the console. The test finishes by
//! asserting the MySQL and MongoDB rows. No YouTube request is part of the approval job; the two YouTube integrations exist only because the
//! definition declares other jobs that use them, and they point at a closed loopback port.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up` (or the run-owned fixtures of the Verify workflow).
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path, process::Command as StdCommand, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
const OPERATIONS: &str = include_str!("../../../product/tests/fixtures/facereview-auth/operations.aip");
const MAPPING: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/mapping.json");
const SCHEMA: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/schema.sql");
const DEFAULTS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/db-defaults.sql");
const SEED: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/seed.sql");
const WERKZEUG: &str = include_str!("../../../product/tests/fixtures/password-auth/werkzeug-3.0.2.json");
const PASSWORD_AUTH_PY: &str = include_str!("../../../product/providers/password_auth.py");
const EXTENSIONS: [(&str, &str); 16] = [
    ("slice13f_watch.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice13f_watch.py")),
    ("slice13b_lists.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice13b_lists.py")),
    ("slice13d_recommend.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice13d_recommend.py")),
    ("recommendation_math.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/recommendation_math.py")),
    ("recommendation_pool_builder.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/recommendation_pool_builder.py")),
    ("slice14a_pool.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14a_pool.py")),
    ("slice14b_trending.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14b_trending.py")),
    ("youtube_common.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/youtube_common.py")),
    ("slice14c_category_fill.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14c_category_fill.py")),
    ("slice14d_approve.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14d_approve.py")),
    ("slice14e_mypage.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14e_mypage.py")),
    ("slice14g_request.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14g_request.py")),
    ("slice14h_dashboard.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14h_dashboard.py")),
    ("slice14j_dummy.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14j_dummy.py")),
    ("slice14k_watch.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14k_watch.py")),
    ("slice14l_frame.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice14l_frame.py")),
];
const META: &str = "aip_t_aui_meta";
const SOURCE: &str = "aip_t_aui_src";
const MONGO_DB: &str = "aip_t_aui_fr";
const ISSUER: &str = "https://login.facereview.example";
const AUDIENCE: &str = "aip-api";
const ALICE: &str = "11111111-1111-4111-8111-111111111111";
const CAROL: &str = "33333333-3333-4333-8333-333333333333";
const BOB: &str = "22222222-2222-4222-8222-222222222222";
const DAVE: &str = "44444444-4444-4444-8444-444444444444";
const ERIN: &str = "55555555-5555-4555-8555-555555555555";
const MEMBER: &str = "77777777-7777-4777-8777-777777777777";
const APPROVED_REQUEST: &str = "ffffffff-0000-4000-8000-000000000001";
const REJECTED_REQUEST: &str = "ffffffff-0000-4000-8000-000000000002";
const USER_COUNT: u64 = 6;
// Short enough to let a login expire during the journey, long enough that the login and dashboard load finish inside it.
const REISSUE_ACCESS_TTL_SECS: u32 = 20;

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn binding_fingerprint(source: &str) -> Option<&str> {
    source.split_once("export const contractFingerprint = \"")?.1.split_once('\"').map(|(fingerprint, _)| fingerprint)
}

struct Env {
    adopt_url: String,
    runtime_url: String,
    mongo_url: String,
    redis_url: Option<String>,
}

fn cli(env: &Env, args: &[&str], config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aip"));
    command.arg("service").args(args).arg("--config").arg(config);
    command.env("AIP_T_AUI_ADOPT_URL", &env.adopt_url).env("AIP_T_AUI_RUNTIME_URL", &env.runtime_url).env("AIP_T_AUI_MONGO_URL", &env.mongo_url);
    if let Some(redis) = &env.redis_url {
        command.env("AIP_T_AUI_REDIS_URL", redis);
    }
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
    let count = tokio::time::timeout(Duration::from_secs(60), reader.read_line(&mut line)).await.expect("startup deadline").expect("read startup");
    if count == 0 {
        let output = child.wait_with_output().await.unwrap();
        return Err(serde_json::from_slice(&output.stderr).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&output.stderr)})));
    }
    tokio::spawn(async move { while reader.read_line(&mut String::new()).await.is_ok_and(|count| count > 0) {} });
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

struct Reply {
    status: u16,
    headers: HashMap<String, String>,
    body: Value,
}

async fn http(address: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> Reply {
    let text = body.unwrap_or("");
    let extra: String = headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect();
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect");
    socket
        .write_all(format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\n{extra}Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(30), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
    let (head, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    let mut lines = head.lines();
    let status = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|line| line.split_once(": ")).map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned())).collect();
    Reply { status, headers, body: serde_json::from_str(body).unwrap_or(Value::Null) }
}

async fn login(address: &str, origin: &str, email: &str, password: &str) -> String {
    let text = json!({"email": email, "password": password}).to_string();
    let reply = http(address, "POST", "/auth/login", &[("Content-Type", "application/json"), ("Origin", origin)], Some(&text)).await;
    assert_eq!(reply.status, 200, "the fixture account logs in: {} {:?}", reply.body, reply.headers.keys().collect::<Vec<_>>());
    reply.body["access_token"].as_str().expect("an access token").to_owned()
}

async fn post(address: &str, path: &str, token: Option<&str>, fingerprint: &str, body: &Value) -> Value {
    let bearer = token.map(|token| format!("Bearer {token}"));
    let mut headers = vec![];
    if let Some(bearer) = bearer.as_deref() {
        headers.push(("Authorization", bearer));
    }
    headers.push(("x-aip-contract", fingerprint));
    http(address, "POST", path, &headers, Some(&body.to_string())).await.body
}

async fn run_script(conn: &mut aip_migrate::mysql_driver::mysql_async::Conn, script: &str) {
    for chunk in script.split(";\n") {
        let statement: String = chunk.lines().filter(|line| !line.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
        if statement.trim().is_empty() {
            continue;
        }
        conn.query_drop(&statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
}

async fn cell(conn: &mut aip_migrate::mysql_driver::mysql_async::Conn, sql: &str) -> Option<String> {
    conn.query_first::<Option<String>, _>(sql).await.unwrap().flatten()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 (AIP_TEST_MYSQL_URL), MongoDB (AIP_TEST_MONGO_URL) and a worker sandbox; the browser journey also needs AIP_FACEREVIEW_ADMIN_UI_DIR; run with -- --ignored"]
async fn facereview_admin_console_runs_through_the_generated_sdk_and_the_servers_own_policy() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let mongo_url = std::env::var("AIP_TEST_MONGO_URL").expect("set AIP_TEST_MONGO_URL to a run-owned MongoDB");
    let redis_url = std::env::var("AIP_TEST_REDIS_URL").ok();
    for (name, value) in [("MongoDB", Some(&mongo_url)), ("Redis", redis_url.as_ref())] {
        let Some(value) = value else { continue };
        let parsed = url::Url::parse(value).expect("fixture URL");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"), "the {name} fixture must bind loopback only");
        assert!(parsed.port().is_some_and(|port| port > 1024 && port != 27017 && port != 6379), "the {name} fixture must use a dedicated high port");
    }
    // A local run may move off a port another dev server holds; the browser origin the service allows follows it.
    let ui_port = std::env::var("AIP_FACEREVIEW_UI_PORT").unwrap_or_else(|_| "5173".to_owned());
    let origin = format!("http://localhost:{ui_port}");

    let directory = tempfile::tempdir().unwrap();
    let expected_definition = format!("{DEFINITION}\n{OPERATIONS}").into_bytes();
    let file = if let Ok(path) = std::env::var("AIP_FACEREVIEW_DEFINITION_FILE") {
        let path = Path::new(&path);
        let supplied = std::fs::read(path).expect("read the exact definition used to generate the admin UI binding");
        assert!(
            supplied == expected_definition,
            "server and admin UI generation definitions differ: file={} sha256={} server_composed_sha256={}",
            path.display(),
            sha256_hex(&supplied),
            sha256_hex(&expected_definition)
        );
        path.to_path_buf()
    } else {
        let path = directory.path().join("facereview.aip");
        std::fs::write(&path, &expected_definition).unwrap();
        path
    };
    let extensions = directory.path().join("extensions");
    std::fs::create_dir(&extensions).unwrap();
    std::fs::write(extensions.join("password_auth.py"), PASSWORD_AUTH_PY).unwrap();
    for (name, source) in EXTENSIONS {
        std::fs::write(extensions.join(name), source).unwrap();
    }

    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}` DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci")).await.unwrap();
    setup.query_drop(format!("USE `{SOURCE}`")).await.unwrap();
    setup.query_drop("SET FOREIGN_KEY_CHECKS=0").await.unwrap();
    run_script(&mut setup, SCHEMA).await;
    run_script(&mut setup, SEED).await;
    setup.query_drop("SET FOREIGN_KEY_CHECKS=1").await.unwrap();
    run_script(&mut setup, DEFAULTS).await;

    let mongo = mongodb::Client::with_uri_str(&mongo_url).await.expect("MongoDB fixture");
    let database = mongo.database(MONGO_DB);
    database.drop().await.expect("reset the test-owned MongoDB database");

    // Real Werkzeug 3.0.2 scrypt hashes (the seed's are placeholders): a SUPER_ADMIN who logs in to the console, and a member who must not get in.
    let vectors: Value = serde_json::from_str(WERKZEUG).unwrap();
    let (member_password, member_hash) = (vectors["cases"][0]["password"].as_str().unwrap(), vectors["cases"][0]["hash"].as_str().unwrap());
    let (admin_password, admin_hash) = (vectors["cases"][3]["password"].as_str().unwrap(), vectors["cases"][3]["hash"].as_str().unwrap());
    for user_id in [CAROL, BOB] {
        setup.exec_drop("UPDATE user SET password = ? WHERE user_id = ?", (admin_hash, user_id)).await.unwrap();
    }
    setup
        .exec_drop(
            "INSERT INTO user (user_id, email, password, name, role, is_tutorial_done, is_verify_email_done, is_deleted, created_at, updated_at) \
             VALUES (?, 'legacy@example.com', ?, '일반 표본회원', 'GENERAL', 1, 1, 0, NOW(), NOW())",
            (MEMBER, member_hash),
        )
        .await
        .unwrap();
    // The dashboard's signup trend is read per period, so the seeded accounts get fixed ages relative to now (the journey asserts the sum of
    // each period: 7d 2, 30d 3, 3m 4, 1y 5, 3y 6; the member inserted above is today). Alice starts as an ADMIN: the console's "ADMIN 해제"
    // is the SUPER_ADMIN's ADMIN to GENERAL change, and the database shows both ends of it.
    for (user, days) in [(ALICE, 3), (BOB, 15), (CAROL, 60), (DAVE, 200), (ERIN, 800)] {
        setup
            .exec_drop(
                "UPDATE user SET created_at = NOW() - INTERVAL ? DAY, updated_at = NOW() - INTERVAL ? DAY WHERE user_id = ?",
                (days, days, user),
            )
            .await
            .unwrap();
    }
    setup.exec_drop("UPDATE user SET role = 'ADMIN' WHERE user_id = ?", (ALICE,)).await.unwrap();
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{ALICE}'")).await.as_deref(),
        Some("ADMIN|0"),
        "the seeded ADMIN the console demotes starts as an active ADMIN"
    );
    // The seed has one PENDING request; a second one is what the console rejects.
    setup
        .exec_drop(
            "INSERT INTO video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, admin_comment, created_at, updated_at) \
             VALUES (?, ?, 'reqvideo002', 'https://www.youtube.com/watch?v=reqvideo002', 'PENDING', NULL, '2026-10-14 10:00:00', '2026-10-14 10:00:00')",
            (REJECTED_REQUEST, DAVE),
        )
        .await
        .unwrap();

    setup.query_drop("DROP USER IF EXISTS 'aip_t_aui_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_aui_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_aui_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_aui_rt:runtime_test_only@{host}/{SOURCE}"),
        mongo_url: mongo_url.clone(),
        redis_url: redis_url.clone(),
    };

    let mut mapping: Value = serde_json::from_str(MAPPING).unwrap();
    mapping["schema"] = json!(SOURCE);
    std::fs::write(directory.path().join("mapping.json"), mapping.to_string()).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(directory.path().join("signing.pem"), private.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap().as_bytes()).unwrap();
    std::fs::write(
        directory.path().join("jwks.json"),
        json!({"keys": [{"kty": "RSA", "kid": "login-1", "alg": "RS256", "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()), "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]})
        .to_string(),
    )
    .unwrap();
    let youtube = |name: &str, path: &str| {
        json!({"name": name, "url": format!("http://127.0.0.1:9/youtube/v3/{path}"), "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024,
            "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]})
    };
    let mut workers = json!({
        "directory": "extensions", "language": "python", "enable_writes": true,
        "mongo": {"uri_env": "AIP_T_AUI_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000,
            "write_collections": ["video_distribution", "video_timeline_emotion_count"], "ensure_indexes": true},
        "integrations": [youtube("youtube_videos", "videos"), youtube("youtube_search", "search")],
    });
    if redis_url.is_some() {
        workers["redis"] = json!({"uri_env": "AIP_T_AUI_REDIS_URL", "prefixes": ["facereview:reco:", "facereview:metrics:"], "max_in_flight": 4, "timeout_ms": 3000});
    }
    let credentials = json!({
        "resource": "User", "secret_column": "password", "identifier_field": "email", "name_field": "name", "role_field": "role",
        "active_field": "is_deleted", "false_fields": ["is_tutorial_done", "is_verify_email_done"], "time_fields": ["created_at", "updated_at"],
        "default_role": "GENERAL",
        "favorites": {"resource": "UserFavoriteGenre", "user_field": "user", "value_field": "genre", "time_fields": ["created_at", "updated_at"]},
        "verify_operation": "verifyPassword", "hash_operation": "hashPassword",
        "signing_key_file": "signing.pem", "kid": "login-1", "access_ttl_secs": 300,
    });
    let config = directory.path().join("service.json");
    let service_config = json!({
        "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
        "database_url_env": "AIP_T_AUI_RUNTIME_URL", "migrate_database_url_env": "AIP_T_AUI_ADOPT_URL",
        "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null, "allowed_origins": [origin],
        "auth": {"issuer": ISSUER, "audience": AUDIENCE, "jwks": {"kind": "file", "path": "jwks.json"}},
        "workers": workers, "credentials": credentials,
    });
    std::fs::write(&config, service_config.to_string()).unwrap();
    let file_arg = file.to_str().unwrap();
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts with password login and write workers");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();

    // --- the server's own policy, before any browser: the console's role check is a convenience, not the boundary. ---
    let admin_token = login(&address, &origin, "c@example.com", admin_password).await;
    let regular_admin_token = login(&address, &origin, "b@example.com", admin_password).await;
    let member_token = login(&address, &origin, "legacy@example.com", member_password).await;
    // Every administrator aggregate with a valid request of its own. The administrator is the control: a probe that the server answers for
    // the administrator is well formed, so the member's and the guest's refusal below is the policy and not a malformed request.
    let queries: [(&str, Value); 6] = [
        ("User.totalCount", json!({"aggregate": "User.totalCount", "input": {}, "filter": []})),
        ("VideoRequest.totalCount", json!({"aggregate": "VideoRequest.totalCount", "input": {}, "filter": []})),
        ("Video.categoryViews", json!({"aggregate": "Video.categoryViews", "input": {"category": "vlog"}})),
        ("User.signupCount", json!({"aggregate": "User.signupCount", "input": {"from": "2026-01-01T00:00:00Z", "to": "2026-12-31T00:00:00Z"}})),
        ("User.weeklyActive", json!({"aggregate": "User.weeklyActive", "input": {"since": "2026-01-01T00:00:00Z"}})),
        ("VideoRequest.pendingCount", json!({"aggregate": "VideoRequest.pendingCount", "input": {}})),
    ];
    let extension_names = ["User.systemStatus", "Video.contentHealth", "VideoRequest.pipeline"];
    for (name, query) in &queries {
        let reply = post(&address, "/read", Some(&admin_token), &fingerprint, &json!({"query": query})).await;
        assert_eq!(reply["ok"], true, "the administrator is answered {name}: {reply}");
        if *name == "User.totalCount" {
            assert_eq!(reply["rows"][0], json!(USER_COUNT), "the administrator counts the fixture's six members: {reply}");
        }
    }
    for extension in extension_names {
        let reply = post(&address, "/extension", Some(&admin_token), &fingerprint, &json!({"extension": extension, "input": {}})).await;
        assert_eq!(reply["ok"], true, "the administrator is answered {extension}: {reply}");
    }
    let approval_input = json!({"operation": "approveVideoRequest", "key": "policy-probe", "delayMs": 0, "input": {
        "request": REJECTED_REQUEST, "youtube_title": "정책 확인", "channel_name": "채널", "duration": 10, "category": "etc"}});
    // A read extension has no allow of its own; its first step is the administrator-only aggregate, whose refusal the caller sees as an
    // extension error. A guest has no session at all, so the extension and job routes refuse them earlier.
    for (who, token, extension_code, job_code) in [
        ("member", Some(member_token.as_str()), "WIRE.EXTENSION_ERROR", "WIRE.ACCESS_DENIED"),
        ("guest", None, "WIRE.UNAUTHENTICATED", "WIRE.UNAUTHENTICATED"),
    ] {
        for (name, query) in &queries {
            let reply = post(&address, "/read", token, &fingerprint, &json!({"query": query})).await;
            assert_eq!(reply["code"], "WIRE.ACCESS_DENIED", "{who} must be refused the administrator aggregate {name}: {reply}");
        }
        for extension in extension_names {
            let reply = post(&address, "/extension", token, &fingerprint, &json!({"extension": extension, "input": {}})).await;
            assert_eq!(reply["code"], extension_code, "{who} must be refused the administrator extension {extension}: {reply}");
        }
        let enqueued = post(&address, "/jobs/enqueue", token, &fingerprint, &approval_input).await;
        assert_eq!(enqueued["code"], job_code, "{who} must be refused the approval job: {enqueued}");
        println!("policy: {who} refused 6 administrator aggregates, 3 administrator extensions and the approval job");
    }
    // The member's own write works with this envelope (control), and the same envelope cannot raise a role.
    let own_write = post(
        &address,
        "/apply",
        Some(&member_token),
        &fingerprint,
        &json!({"key": "member-control", "request": {"apply": "User.completeTutorial", "target": {"ids": [MEMBER]}}}),
    )
    .await;
    assert_eq!(own_write["ok"], true, "a member may complete their own tutorial: {own_write}");
    let promote = post(
        &address,
        "/apply",
        Some(&member_token),
        &fingerprint,
        &json!({"key": "member-escalate", "request": {"apply": "User.changeRole", "target": {"ids": [MEMBER]}, "values": {"role": "SUPER_ADMIN"}}}),
    )
    .await;
    assert_eq!(promote["code"], "WIRE.FORBIDDEN", "a member must not raise a role, not even their own: {promote}");
    println!("policy: member role escalation refused with {}", promote["code"]);
    assert_eq!(cell(&mut setup, &format!("SELECT role FROM `{SOURCE}`.user WHERE user_id='{MEMBER}'")).await.as_deref(), Some("GENERAL"));
    assert_eq!(
        cell(&mut setup, &format!("SELECT COUNT(*) FROM `{SOURCE}`.video WHERE youtube_url='reqvideo002'")).await.as_deref(),
        Some("0"),
        "a refused approval made no video"
    );

    // SUPER_ADMIN may assign ADMIN. Restore the fixture row before the browser journey so it can prove the same direction from the UI.
    let superadmin_promote = post(
        &address,
        "/apply",
        Some(&admin_token),
        &fingerprint,
        &json!({"key": "superadmin-promote-general", "request": {"apply": "User.changeRole", "target": {"ids": [MEMBER]}, "values": {"role": "ADMIN"}}}),
    )
    .await;
    assert_eq!(superadmin_promote["ok"], true, "SUPER_ADMIN may assign ADMIN: {superadmin_promote}");
    assert_eq!(cell(&mut setup, &format!("SELECT role FROM `{SOURCE}`.user WHERE user_id='{MEMBER}'")).await.as_deref(), Some("ADMIN"));
    setup.exec_drop(format!("UPDATE `{SOURCE}`.user SET role='GENERAL' WHERE user_id='{MEMBER}'"), ()).await.unwrap();

    // ADMIN may deactivate GENERAL. A later browser step checks this through the actual console action.
    let regular_deactivate = post(
        &address,
        "/apply",
        Some(&regular_admin_token),
        &fingerprint,
        &json!({"key": "admin-deactivate-general", "request": {"apply": "User.deactivate", "target": {"ids": [MEMBER]}}}),
    )
    .await;
    assert_eq!(regular_deactivate["ok"], true, "ADMIN may deactivate GENERAL: {regular_deactivate}");
    assert_eq!(cell(&mut setup, &format!("SELECT is_deleted FROM `{SOURCE}`.user WHERE user_id='{MEMBER}'")).await.as_deref(), Some("1"));
    setup.exec_drop(format!("UPDATE `{SOURCE}`.user SET is_deleted=0 WHERE user_id='{MEMBER}'"), ()).await.unwrap();

    // Forged target ids and role values reach the real AIP write policy and cannot elevate or deactivate protected roles.
    for (label, request) in [
        ("admin-targets-admin", json!({"apply": "User.deactivate", "target": {"ids": [ERIN]}})),
        ("admin-targets-superadmin", json!({"apply": "User.deactivate", "target": {"ids": [CAROL]}})),
        ("admin-forges-role", json!({"apply": "User.changeRole", "target": {"ids": [ERIN]}, "values": {"role": "SUPER_ADMIN"}})),
        ("admin-targets-role", json!({"apply": "User.changeRole", "target": {"ids": [CAROL]}, "values": {"role": "GENERAL"}})),
    ] {
        let reply = post(&address, "/apply", Some(&regular_admin_token), &fingerprint, &json!({"key": label, "request": request})).await;
        assert_eq!(reply["code"], "WIRE.FORBIDDEN", "ADMIN forged {label} must be refused: {reply}");
    }
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{BOB}'")).await.as_deref(),
        Some("ADMIN|0")
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{ERIN}'")).await.as_deref(),
        Some("ADMIN|0")
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{CAROL}'")).await.as_deref(),
        Some("SUPER_ADMIN|0")
    );
    println!("policy: SUPER_ADMIN assigns ADMIN; ADMIN deactivates GENERAL; forged protected targets and role refused with WIRE.FORBIDDEN");

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Some(ui_root_value) = std::env::var_os("AIP_FACEREVIEW_ADMIN_UI_DIR") else {
        eprintln!("React admin browser journey skipped: AIP_FACEREVIEW_ADMIN_UI_DIR is unset; the SDK/API policy checks above are separate");
        return;
    };
    let ui_root = Path::new(&ui_root_value);
    let ui_binding = std::fs::read_to_string(ui_root.join("src/aip/facereview.ts")).expect("read binding consumed by the admin UI");
    assert_eq!(
        binding_fingerprint(&ui_binding).expect("the UI binding carries its contract fingerprint"),
        fingerprint,
        "the admin UI binding and the running server must share one contract"
    );
    let output = std::env::var("AIP_FACEREVIEW_BROWSER_OUTPUT").expect("a run-owned browser artifact directory");
    let python = std::env::var("AIP_FACEREVIEW_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    let journey = StdCommand::new(python)
        .arg(root.join("product/tests/facereview-ui-admin-browser.py"))
        .args(["--ui", ui_root.to_str().unwrap(), "--api", &format!("http://{address}"), "--out", &output])
        .env("AIP_FACEREVIEW_ADMIN_EMAIL", "c@example.com")
        .env("AIP_FACEREVIEW_ADMIN_PASSWORD", admin_password)
        .env("AIP_FACEREVIEW_MEMBER_EMAIL", "legacy@example.com")
        .env("AIP_FACEREVIEW_MEMBER_PASSWORD", member_password)
        .env("AIP_FACEREVIEW_SECONDARY_ADMIN_EMAIL", "b@example.com")
        .env("AIP_FACEREVIEW_SECONDARY_ADMIN_PASSWORD", admin_password)
        .env("AIP_FACEREVIEW_EXPECTED_CONTRACT", &fingerprint)
        .env("AIP_FACEREVIEW_UI_PORT", &ui_port)
        .env("AIP_FACEREVIEW_REDIS_CONFIGURED", if redis_url.is_some() { "1" } else { "0" })
        .output()
        .expect("start the real React admin browser journey");
    assert!(
        journey.status.success(),
        "React admin browser journey failed (status {:?})\nstdout: {}\nstderr: {}",
        journey.status.code(),
        String::from_utf8_lossy(&journey.stdout),
        String::from_utf8_lossy(&journey.stderr)
    );
    print!("{}", String::from_utf8_lossy(&journey.stdout));

    // --- what the console did, read back from the stores it wrote to. ---
    assert_eq!(
        cell(&mut setup, &format!("SELECT status FROM `{SOURCE}`.video_request WHERE video_request_id='{APPROVED_REQUEST}'")).await.as_deref(),
        Some("ACCEPTED"),
        "the approval job accepted the request"
    );
    assert_eq!(
        cell(
            &mut setup,
            &format!("SELECT CONCAT_WS('|', title, channel_name, duration, category, view_count, is_deleted) FROM `{SOURCE}`.video WHERE youtube_url='reqvideo001'")
        )
        .await
        .as_deref(),
        Some("승인 표본 영상|표본 채널|321|vlog|0|0"),
        "the video carries what the administrator typed in the approval form"
    );
    let video_id =
        cell(&mut setup, &format!("SELECT video_id FROM `{SOURCE}`.video WHERE youtube_url='reqvideo001'")).await.expect("the approved video");
    for collection in ["video_distribution", "video_timeline_emotion_count"] {
        let count =
            database.collection::<mongodb::bson::Document>(collection).count_documents(mongodb::bson::doc! {"video_id": &video_id}).await.unwrap();
        assert_eq!(count, 1, "the approval job wrote one initial {collection} document");
    }
    assert_eq!(
        cell(
            &mut setup,
            &format!("SELECT CONCAT_WS('|', status, admin_comment) FROM `{SOURCE}`.video_request WHERE video_request_id='{REJECTED_REQUEST}'")
        )
        .await
        .as_deref(),
        Some("REJECTED|중복 영상입니다"),
        "the console rejection stored the administrator's reason"
    );
    assert_eq!(cell(&mut setup, &format!("SELECT COUNT(*) FROM `{SOURCE}`.video WHERE youtube_url='reqvideo002'")).await.as_deref(), Some("0"));
    assert_eq!(cell(&mut setup, &format!("SELECT role FROM `{SOURCE}`.user WHERE user_id='{MEMBER}'")).await.as_deref(), Some("ADMIN"));
    // Role assignment and ADMIN's allowed deactivation are independently read back after real browser SDK calls.
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{CAROL}'")).await.as_deref(),
        Some("SUPER_ADMIN|0"),
        "the SUPER_ADMIN target was unchanged by forged requests"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{BOB}'")).await.as_deref(),
        Some("ADMIN|0"),
        "the ADMIN actor could not promote their own role"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{ERIN}'")).await.as_deref(),
        Some("ADMIN|0"),
        "the ADMIN target of the forged deactivation and role requests is unchanged after the browser's direct SDK attempts"
    );
    // The four other console writes, each against the original AdminService's rows: only the fields the original changes are changed.
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{DAVE}'")).await.as_deref(),
        Some("GENERAL|1"),
        "deactivation soft-deleted the member and kept the role"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT is_deleted FROM `{SOURCE}`.comment WHERE comment_id='cccccccc-0000-4000-8000-000000000005'"))
            .await
            .as_deref(),
        Some("1"),
        "deactivation hid the member's comment in the same transaction, as the original does"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT COUNT(*) FROM `{SOURCE}`.comment WHERE user_id='{DAVE}' AND is_deleted=0")).await.as_deref(),
        Some("0")
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{MEMBER}'")).await.as_deref(),
        Some("ADMIN|0")
    );
    assert_eq!(cell(&mut setup, &format!("SELECT role FROM `{SOURCE}`.user WHERE user_id='{MEMBER}'")).await.as_deref(), Some("ADMIN"));
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{ALICE}'")).await.as_deref(),
        Some("GENERAL|0"),
        "the SUPER_ADMIN's ADMIN 해제 returned the ADMIN to GENERAL and nothing else about the row"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT is_deleted FROM `{SOURCE}`.video WHERE video_id='aaaaaaaa-0000-4000-8000-000000000002'")).await.as_deref(),
        Some("1"),
        "the console video delete is the original's soft delete"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT is_deleted FROM `{SOURCE}`.comment WHERE comment_id='cccccccc-0000-4000-8000-000000000001'"))
            .await
            .as_deref(),
        Some("1"),
        "the console comment delete is the original's soft delete"
    );
    assert_eq!(
        cell(&mut setup, &format!("SELECT COUNT(*) FROM `{SOURCE}`.comment")).await.as_deref(),
        Some("5"),
        "no comment row was physically deleted"
    );
    // --- token reissue: the same service with a short access TTL, so the access token of a real login expires while the console is open. ---
    let reissue_config = directory.path().join("service-reissue.json");
    let mut short_lived = service_config.clone();
    short_lived["credentials"]["access_ttl_secs"] = json!(REISSUE_ACCESS_TTL_SECS);
    std::fs::write(&reissue_config, short_lived.to_string()).unwrap();
    let (_short_lived_server, short_lived_listening) =
        start_server(&env, &file, &reissue_config).await.expect("serve starts again with a short access TTL");
    let short_lived_address = short_lived_listening["address"].as_str().unwrap().to_owned();
    assert_eq!(short_lived_listening["fingerprint"].as_str().unwrap(), fingerprint, "the short-lived service serves the same contract");
    let reissue_journey = StdCommand::new(std::env::var("AIP_FACEREVIEW_PYTHON").unwrap_or_else(|_| "python3".to_owned()))
        .arg(root.join("product/tests/facereview-ui-admin-reissue-browser.py"))
        .args(["--ui", ui_root.to_str().unwrap(), "--api", &format!("http://{short_lived_address}"), "--out", &output])
        .env("AIP_FACEREVIEW_ADMIN_EMAIL", "c@example.com")
        .env("AIP_FACEREVIEW_ADMIN_PASSWORD", admin_password)
        .env("AIP_FACEREVIEW_ACCESS_TTL_SECS", REISSUE_ACCESS_TTL_SECS.to_string())
        .env("AIP_FACEREVIEW_UI_PORT", &ui_port)
        .output()
        .expect("start the real React admin reissue journey");
    assert!(
        reissue_journey.status.success(),
        "React admin reissue journey failed (status {:?})\nstdout: {}\nstderr: {}",
        reissue_journey.status.code(),
        String::from_utf8_lossy(&reissue_journey.stdout),
        String::from_utf8_lossy(&reissue_journey.stderr)
    );
    print!("{}", String::from_utf8_lossy(&reissue_journey.stdout));
    assert_eq!(
        cell(&mut setup, &format!("SELECT CONCAT_WS('|', role, is_deleted) FROM `{SOURCE}`.user WHERE user_id='{CAROL}'")).await.as_deref(),
        Some("SUPER_ADMIN|0"),
        "reissuing the SUPER_ADMIN's access token left the account as it was"
    );
    println!(
        "React admin console journey: SUPER_ADMIN login, dashboard, users/requests/videos/comments lists, the five console writes and the approval job (MySQL video + 2 Mongo documents), non-admin login refused"
    );
}
