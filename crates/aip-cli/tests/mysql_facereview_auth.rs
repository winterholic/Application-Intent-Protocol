#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.2: signup, login, token reissue, logout, guest and privilege boundaries of the FaceReview port, through the real
//! `aip service` binary on the real MySQL 8.4 schema of the original (pinned new-facereview@7948b7e7), checking passwords
//! with the official Python extension in the platform's worker sandbox.
//!
//! Nothing is scripted: the HTTP requests, the cookies, the sessions table and the user rows are what a browser and an
//! operator would produce and see. The original's refresh replay (a used refresh token minting tokens again) is the one
//! behaviour deliberately not reproduced; the test asserts it is refused.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up`; the adoption account is the container admin.
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    process::Command as StdCommand,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
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

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn binding_fingerprint(source: &str) -> Option<&str> {
    source.split_once("export const contractFingerprint = \"")?.1.split_once('\"').map(|(fingerprint, _)| fingerprint)
}
// The definition declares the home and watch list extensions, and `serve` refuses a worker directory that lacks any of them.
const LIST_EXTENSIONS: [(&str, &str); 16] = [
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
const META: &str = "aip_t_auth_meta";
const SOURCE: &str = "aip_t_auth_src";
const MONGO_DB: &str = "aip_t_auth_fr";
const ISSUER: &str = "https://login.facereview.example";
const AUDIENCE: &str = "aip-api";
const ORIGIN: &str = "http://localhost:5173";
const CAROL: &str = "33333333-3333-4333-8333-333333333333";
const LEGACY: &str = "77777777-7777-4777-8777-777777777777";

struct Env {
    adopt_url: String,
    runtime_url: String,
    mongo_url: String,
}

fn cli(env: &Env, args: &[&str], config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aip"));
    command.arg("service");
    if args.first() == Some(&"principal") {
        command.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        command.args(args).arg("--config").arg(config);
    }
    command.env("AIP_T_AUTH_ADOPT_URL", &env.adopt_url).env("AIP_T_AUTH_RUNTIME_URL", &env.runtime_url).env("AIP_T_AUTH_MONGO_URL", &env.mongo_url);
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

async fn auth(address: &str, path: &str, body: &Value, cookie: Option<&str>) -> Reply {
    let text = body.to_string();
    let cookie_header = cookie.map(|value| format!("refresh_token={value}"));
    let mut headers = vec![("Content-Type", "application/json"), ("Origin", ORIGIN)];
    if let Some(cookie) = cookie_header.as_deref() {
        headers.push(("Cookie", cookie));
    }
    http(address, "POST", path, &headers, Some(&text)).await
}

async fn post(address: &str, path: &str, token: Option<&str>, fingerprint: &str, body: &Value) -> Value {
    let bearer = token.map(|token| format!("Bearer {token}"));
    let mut headers = vec![];
    if let Some(bearer) = bearer.as_deref() {
        headers.push(("Authorization", bearer));
    }
    if !fingerprint.is_empty() {
        headers.push(("x-aip-contract", fingerprint));
    }
    http(address, "POST", path, &headers, Some(&body.to_string())).await.body
}

fn set_cookie(reply: &Reply) -> String {
    reply.headers.get("set-cookie").unwrap_or_else(|| panic!("a Set-Cookie header: {:?} {}", reply.headers, reply.body)).clone()
}

fn refresh_value(cookie: &str) -> String {
    cookie.strip_prefix("refresh_token=").unwrap().split(';').next().unwrap().to_owned()
}

fn access(reply: &Reply) -> String {
    reply.body["access_token"].as_str().unwrap_or_else(|| panic!("an access token: {} {}", reply.status, reply.body)).to_owned()
}

fn claims(token: &str) -> Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token.split('.').nth(1).unwrap()).unwrap()).unwrap()
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

fn run_facereview_ui_browser_journey(address: &str, fingerprint: &str, phase: &str, root: &Path) -> std::process::Output {
    let ui_root = std::env::var("AIP_FACEREVIEW_UI_DIR").expect("CI must provide the pinned FaceReview UI checkout");
    let output = std::env::var("AIP_FACEREVIEW_BROWSER_OUTPUT").expect("CI must provide a run-owned browser artifact directory");
    let python = std::env::var("AIP_FACEREVIEW_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    StdCommand::new(python)
        .arg(root.join("product/tests/facereview-ui-member-browser.py"))
        .args(["--ui", &ui_root, "--api", &format!("http://{address}"), "--phase", phase, "--out", &format!("{output}/{phase}")])
        .env("AIP_FACEREVIEW_JOURNEY_EMAIL", "legacy@example.com")
        .env("AIP_FACEREVIEW_JOURNEY_PASSWORD", "password123")
        .env("AIP_FACEREVIEW_EXPECTED_CONTRACT", fingerprint)
        .output()
        .expect("start the real React browser journey")
}

fn run_facereview_ui_profile_browser_journey(address: &str, fingerprint: &str, root: &Path) -> std::process::Output {
    let ui_root = std::env::var("AIP_FACEREVIEW_UI_DIR").expect("CI must provide the pinned FaceReview UI checkout");
    let output = std::env::var("AIP_FACEREVIEW_BROWSER_OUTPUT").expect("CI must provide a run-owned browser artifact directory");
    let python = std::env::var("AIP_FACEREVIEW_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    StdCommand::new(python)
        .arg(root.join("product/tests/facereview-ui-profile-browser.py"))
        .args(["--ui", &ui_root, "--api", &format!("http://{address}"), "--out", &format!("{output}/profile")])
        .env("AIP_FACEREVIEW_JOURNEY_EMAIL", "legacy@example.com")
        .env("AIP_FACEREVIEW_JOURNEY_PASSWORD", "password123")
        .env("AIP_FACEREVIEW_EXPECTED_CONTRACT", fingerprint)
        .current_dir(root)
        .output()
        .expect("start the real React profile browser journey")
}

/// A stand-in mail provider: it records every JSON body it is sent and answers 200, or 500 while `fail` is set.
struct Mailer {
    port: u16,
    sent: Arc<Mutex<Vec<Value>>>,
    fail: Arc<AtomicBool>,
}

async fn start_mailer() -> Mailer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sent, fail) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
    let (log, failing) = (sent.clone(), fail.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (log, failing) = (log.clone(), failing.clone());
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() < length {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(body) {
                        log.lock().unwrap().push(value);
                    }
                    let status = if failing.load(Ordering::SeqCst) { "500 Internal Server Error" } else { "200 OK" };
                    let _ = socket
                        .write_all(
                            format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}")
                                .as_bytes(),
                        )
                        .await;
                    return;
                }
            });
        }
    });
    Mailer { port, sent, fail }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh) and a worker sandbox; run with -- --ignored"]
async fn facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let mongo_url = std::env::var("AIP_TEST_MONGO_URL").expect("set AIP_TEST_MONGO_URL to a run-owned MongoDB");
    let parsed_mongo = url::Url::parse(&mongo_url).expect("Mongo URL");
    assert_eq!(parsed_mongo.host_str(), Some("127.0.0.1"), "the MongoDB fixture must bind loopback only");
    assert!(parsed_mongo.port().is_some_and(|port| port > 1024 && port != 27017), "the MongoDB fixture must use a dedicated high port");
    let directory = tempfile::tempdir().unwrap();
    let expected_definition = format!("{DEFINITION}\n{OPERATIONS}").into_bytes();
    let file = if let Ok(path) = std::env::var("AIP_FACEREVIEW_DEFINITION_FILE") {
        let path = Path::new(&path);
        let supplied = std::fs::read(path).expect("read the exact definition used to generate the pinned UI binding");
        let expected_sha = sha256_hex(&expected_definition);
        let supplied_sha = sha256_hex(&supplied);
        if let Ok(promised_sha) = std::env::var("AIP_FACEREVIEW_DEFINITION_SHA256") {
            assert_eq!(
                supplied_sha,
                promised_sha,
                "UI generation definition changed after overlay generation: file={} expected_sha256={} actual_sha256={}",
                path.display(),
                promised_sha,
                supplied_sha
            );
        }
        assert!(
            supplied == expected_definition,
            "server and UI generation definitions differ: file={} sha256={} server_composed_sha256={}",
            path.display(),
            supplied_sha,
            expected_sha
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
    for (name, source) in LIST_EXTENSIONS {
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
    // This journey uses one model-less video document: its emotion fields are fixture data, not ML output.
    let mongo = mongodb::Client::with_uri_str(&mongo_url).await.expect("MongoDB fixture");
    let database = mongo.database(MONGO_DB);
    database.drop().await.expect("reset the test-owned MongoDB database");
    database
        .collection::<mongodb::bson::Document>("video_distribution")
        .insert_one(mongodb::bson::doc! {
            "video_id": "aaaaaaaa-0000-4000-8000-000000000001",
            "dominant_emotion": "neutral",
            "emotion_averages": {"neutral": 0.5, "happy": 0.2, "surprise": 0.1, "sad": 0.1, "angry": 0.1},
            "created_at": mongodb::bson::DateTime::from_millis(1_760_000_000_000_i64),
        })
        .await
        .expect("seed the dedicated model-less video fixture");
    // An account the original wrote: a real Werkzeug 3.0.2 scrypt hash of "password123" (the seed's hashes are placeholders).
    let vectors: Value = serde_json::from_str(WERKZEUG).unwrap();
    let (legacy_password, legacy_hash) = (vectors["cases"][0]["password"].as_str().unwrap(), vectors["cases"][0]["hash"].as_str().unwrap());
    assert_eq!(legacy_password, "password123");
    setup
        .exec_drop(
            "INSERT INTO user (user_id, email, password, name, role, is_tutorial_done, is_verify_email_done, is_deleted, created_at, updated_at) \
             VALUES (?, 'legacy@example.com', ?, 'AIP 프로필 표본회원', 'GENERAL', 1, 1, 0, NOW(), NOW())",
            (LEGACY, legacy_hash),
        )
        .await
        .unwrap();
    setup
        .exec_drop(
            "INSERT INTO user_favorite_genre (user_favorite_genre_id, user_id, genre, created_at, updated_at) VALUES \
         ('88888888-8888-4888-8888-888888888801', ?, 'eating', NOW(), NOW()), \
         ('88888888-8888-4888-8888-888888888802', ?, 'horror', NOW(), NOW())",
            (LEGACY, LEGACY),
        )
        .await
        .unwrap();
    // Carol is the seed's SUPER_ADMIN; give her a real hash so she can log in for the escalation checks.
    setup.exec_drop("UPDATE user SET password = ? WHERE user_id = ?", (vectors["cases"][3]["hash"].as_str().unwrap(), CAROL)).await.unwrap();
    let carol_password = vectors["cases"][3]["password"].as_str().unwrap().to_owned();

    setup.query_drop("DROP USER IF EXISTS 'aip_t_auth_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_auth_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_auth_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_auth_rt:runtime_test_only@{host}/{SOURCE}"),
        mongo_url: mongo_url.clone(),
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
    let mailer = start_mailer().await;
    let mail_url = format!("http://127.0.0.1:{}/send", mailer.port);
    let config = directory.path().join("service.json");
    let write_config = |credentials: Value| {
        std::fs::write(
            &config,
            json!({
                "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
                "database_url_env": "AIP_T_AUTH_RUNTIME_URL", "migrate_database_url_env": "AIP_T_AUTH_ADOPT_URL",
                "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null, "allowed_origins": [ORIGIN],
                "auth": {"issuer": ISSUER, "audience": AUDIENCE, "jwks": {"kind": "file", "path": "jwks.json"}},
                "workers": {"directory": "extensions", "language": "python", "mongo": {"uri_env": "AIP_T_AUTH_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000}, "integrations": [{"name": "youtube_videos", "url": "http://127.0.0.1:9/youtube/v3/videos", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}, {"name": "youtube_search", "url": "http://127.0.0.1:9/youtube/v3/search", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}, {"name": "mail_send", "url": mail_url, "method": "POST", "timeout_ms": 3000, "max_request_bytes": 4096, "max_response_bytes": 4096, "allow_loopback": true}]},
                "credentials": credentials,
            })
            .to_string(),
        )
        .unwrap();
    };
    let credentials = json!({
        "resource": "User", "secret_column": "password", "identifier_field": "email", "name_field": "name", "role_field": "role",
        "active_field": "is_deleted", "false_fields": ["is_tutorial_done", "is_verify_email_done"], "time_fields": ["created_at", "updated_at"],
        "default_role": "GENERAL",
        "favorites": {"resource": "UserFavoriteGenre", "user_field": "user", "value_field": "genre", "time_fields": ["created_at", "updated_at"]},
        "verify_operation": "verifyPassword", "hash_operation": "hashPassword",
        "signing_key_file": "signing.pem", "kid": "login-1", "access_ttl_secs": 300,
        "mail": {"integration": "mail_send", "verified_field": "is_verify_email_done", "updated_field": "updated_at"},
    });
    write_config(credentials.clone());
    let file_arg = file.to_str().unwrap();

    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    let session_tables =
        cell(&mut setup, &format!("SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA='{META}' AND TABLE_NAME LIKE 'aip_auth_%'"))
            .await;
    assert_eq!(session_tables.as_deref(), Some("3"), "adoption creates the session tables and the table of mail challenges");

    // A signing key the verifier does not trust must fail the start, not mint tokens the server then rejects.
    let other = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    std::fs::write(directory.path().join("untrusted.pem"), other.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap().as_bytes()).unwrap();
    let mut untrusted = credentials.clone();
    untrusted["signing_key_file"] = json!("untrusted.pem");
    write_config(untrusted);
    let refused = start_server(&env, &file, &config).await.expect_err("an untrusted signing key is refused at startup");
    assert_eq!(refused["code"], "CREDENTIALS_CONFIG", "{refused}");
    let mut mapped = credentials.clone();
    mapped["secret_column"] = json!("email");
    write_config(mapped);
    let refused = start_server(&env, &file, &config).await.expect_err("a secret column that a resource maps is refused");
    assert_eq!(refused["code"], "CREDENTIALS_CONFIG", "{refused}");
    write_config(credentials);

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts with password login");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();

    // Exercise the real generated TypeScript binding, installed SDK build, and the pinned UI adapters against
    // this fixture's actual login service and MySQL tables. Credentials are test-only and are passed via env,
    // never printed into the CI log.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binding = directory.path().join("facereview-generated.ts");
    let generated = StdCommand::new(env!("CARGO_BIN_EXE_aip"))
        .args(["service", "gen", "--lang", "ts", "--out", binding.to_str().unwrap(), "--sdk-import", "@aip/sdk", file_arg])
        .current_dir(&root)
        .output()
        .expect("start FaceReview TypeScript binding generation");
    assert!(generated.status.success(), "TypeScript binding generation failed: {}", String::from_utf8_lossy(&generated.stderr));
    let generated_binding = std::fs::read(&binding).expect("read generated FaceReview binding");
    let generated_fingerprint = binding_fingerprint(std::str::from_utf8(&generated_binding).expect("generated binding is UTF-8"))
        .expect("generated binding contains its contract fingerprint")
        .to_owned();
    assert_eq!(
        generated_fingerprint,
        fingerprint,
        "server definition and SDK binding differ: definition={} sha256={} binding={} sha256={} binding_fingerprint={} listening_fingerprint={}",
        file.display(),
        sha256_hex(&std::fs::read(&file).unwrap()),
        binding.display(),
        sha256_hex(&generated_binding),
        generated_fingerprint,
        fingerprint,
    );
    let sdk_dir = directory.path().join("sdk");
    let build = StdCommand::new("node")
        .arg(root.join("product/tools/build-sdk.mjs"))
        .arg("--out")
        .arg(&sdk_dir)
        .current_dir(&root)
        .output()
        .expect("start the installed SDK build");
    assert!(build.status.success(), "SDK build failed: {}", String::from_utf8_lossy(&build.stderr));
    let consumer = directory.path().join("sdk-consumer");
    std::fs::create_dir(&consumer).unwrap();
    std::fs::write(consumer.join("package.json"), "{\"private\":true,\"type\":\"module\"}\n").unwrap();
    let packed = StdCommand::new("npm")
        .args(["pack", "--offline", "--ignore-scripts"])
        .current_dir(&sdk_dir)
        .output()
        .expect("start the offline @aip/sdk pack");
    assert!(packed.status.success(), "SDK pack failed: {}", String::from_utf8_lossy(&packed.stderr));
    let install = StdCommand::new("npm")
        .args(["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund"])
        .arg(sdk_dir.join("aip-sdk-0.1.0.tgz"))
        .current_dir(&consumer)
        .output()
        .expect("start the offline @aip/sdk install");
    assert!(install.status.success(), "SDK install failed: {}", String::from_utf8_lossy(&install.stderr));
    let journey = StdCommand::new("node")
        .arg("--experimental-strip-types")
        .arg(root.join("product/tests/facereview-member-sdk-journey.mjs"))
        .arg(format!("http://{address}"))
        .arg(&binding)
        .arg(consumer.join("node_modules/@aip/sdk/product/sdk/index.js"))
        .env("AIP_FACEREVIEW_JOURNEY_EMAIL", "legacy@example.com")
        .env("AIP_FACEREVIEW_JOURNEY_PASSWORD", "password123")
        .current_dir(&root)
        .output()
        .expect("start the generated SDK member journey");
    assert!(
        journey.status.success(),
        "generated SDK member journey failed (status {:?})\nstdout: {}\nstderr: {}",
        journey.status.code(),
        String::from_utf8_lossy(&journey.stdout),
        String::from_utf8_lossy(&journey.stderr)
    );
    print!("{}", String::from_utf8_lossy(&journey.stdout));
    eprint!("{}", String::from_utf8_lossy(&journey.stderr));

    // Run real browser interactions only when the Verify workflow supplies the pinned UI and Playwright.
    // The UI test uses the same live AIP service and fixture DB as the SDK journey above.
    if std::env::var_os("AIP_FACEREVIEW_UI_DIR").is_some() {
        let browser_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let ui_root_value = std::env::var("AIP_FACEREVIEW_UI_DIR").unwrap();
        let ui_root = Path::new(&ui_root_value);
        let ui_binding_path = ui_root.join("src/aip/facereview.ts");
        let ui_binding = std::fs::read(&ui_binding_path).expect("read binding consumed by pinned React UI");
        let ui_fingerprint = binding_fingerprint(std::str::from_utf8(&ui_binding).expect("UI binding is UTF-8"))
            .expect("pinned UI binding contains its contract fingerprint");
        let browser_output = std::env::var("AIP_FACEREVIEW_BROWSER_OUTPUT").expect("browser output directory");
        let definition_sha = sha256_hex(&std::fs::read(&file).unwrap());
        let ui_binding_sha = sha256_hex(&ui_binding);
        let generated_binding_sha = sha256_hex(&generated_binding);
        let evidence = json!({
            "relational_fixture_sha256": sha256_hex(DEFINITION.as_bytes()),
            "auth_operations_fixture_sha256": sha256_hex(OPERATIONS.as_bytes()),
            "server_definition": {"path": file.display().to_string(), "sha256": definition_sha},
            "ui_generation_definition": {"path": std::env::var("AIP_FACEREVIEW_DEFINITION_FILE").unwrap_or_else(|_| file.display().to_string()), "sha256": definition_sha, "workflow_declared_sha256": std::env::var("AIP_FACEREVIEW_DEFINITION_SHA256").unwrap_or_else(|_| "not_set".to_owned())},
            "server_listening_fingerprint": fingerprint,
            "generated_binding": {"path": binding.display().to_string(), "sha256": generated_binding_sha, "fingerprint": generated_fingerprint},
            "ui_binding": {"path": ui_binding_path.display().to_string(), "sha256": ui_binding_sha, "fingerprint": ui_fingerprint},
            "browser_header_capture": "each phase result records the actual x-aip-contract header value",
            "credentials_recorded": false
        });
        std::fs::create_dir_all(&browser_output).unwrap();
        std::fs::write(Path::new(&browser_output).join("contract-evidence.json"), format!("{}\n", serde_json::to_string_pretty(&evidence).unwrap()))
            .unwrap();
        assert_eq!(
            ui_fingerprint,
            fingerprint,
            "browser must not start with a contract mismatch: UI binding={} sha256={} fingerprint={}, server definition={} sha256={} generated binding={} sha256={} fingerprint={} listening_fingerprint={}",
            ui_binding_path.display(),
            ui_binding_sha,
            ui_fingerprint,
            file.display(),
            definition_sha,
            binding.display(),
            generated_binding_sha,
            generated_fingerprint,
            fingerprint,
        );
        let profile = run_facereview_ui_profile_browser_journey(&address, &fingerprint, &browser_root);
        assert!(
            profile.status.success(),
            "React profile browser journey failed (status {:?})\nstdout: {}\nstderr: {}",
            profile.status.code(),
            String::from_utf8_lossy(&profile.stdout),
            String::from_utf8_lossy(&profile.stderr)
        );
        print!("{}", String::from_utf8_lossy(&profile.stdout));
        assert_eq!(cell(&mut setup, "SELECT CAST(COUNT(*) AS CHAR) FROM video_bookmark WHERE user_id='77777777-7777-4777-8777-777777777777' AND video_id='aaaaaaaa-0000-4000-8000-000000000001'").await.as_deref(), Some("0"), "fixture member starts without a bookmark");
        let add = run_facereview_ui_browser_journey(&address, &fingerprint, "add", &browser_root);
        assert!(
            add.status.success(),
            "React add-bookmark browser journey failed (status {:?})\nstdout: {}\nstderr: {}",
            add.status.code(),
            String::from_utf8_lossy(&add.stdout),
            String::from_utf8_lossy(&add.stderr)
        );
        print!("{}", String::from_utf8_lossy(&add.stdout));
        assert_eq!(cell(&mut setup, "SELECT CAST(COUNT(*) AS CHAR) FROM video_bookmark WHERE user_id='77777777-7777-4777-8777-777777777777' AND video_id='aaaaaaaa-0000-4000-8000-000000000001'").await.as_deref(), Some("1"), "browser bookmark add persists one row in MySQL");

        let remove = run_facereview_ui_browser_journey(&address, &fingerprint, "remove", &browser_root);
        assert!(
            remove.status.success(),
            "React remove-bookmark browser journey failed (status {:?})\nstdout: {}\nstderr: {}",
            remove.status.code(),
            String::from_utf8_lossy(&remove.stdout),
            String::from_utf8_lossy(&remove.stderr)
        );
        print!("{}", String::from_utf8_lossy(&remove.stdout));
        assert_eq!(cell(&mut setup, "SELECT CAST(COUNT(*) AS CHAR) FROM video_bookmark WHERE user_id='77777777-7777-4777-8777-777777777777' AND video_id='aaaaaaaa-0000-4000-8000-000000000001'").await.as_deref(), Some("0"), "browser bookmark remove deletes the member row in MySQL");
        println!("React browser journey: login, member video list, bookmark on/reload (MySQL rows=1), off/reload (MySQL rows=0)");
    } else {
        eprintln!("React browser journey skipped: AIP_FACEREVIEW_UI_DIR is unset; SDK/API journey is separate");
    }

    let read_users = |token: Option<String>| {
        let (address, fingerprint) = (address.clone(), fingerprint.clone());
        async move {
            post(&address, "/read", token.as_deref(), &fingerprint, &json!({"query": {"read": "User", "select": ["id", "email", "role"]}})).await
        }
    };

    // --- check-email: the original's public duplicate check.
    let taken = auth(&address, "/auth/check-email", &json!({"email": "legacy@example.com"}), None).await;
    assert_eq!((taken.status, taken.body["is_duplicate"].clone()), (200, json!(true)), "{}", taken.body);
    let free = auth(&address, "/auth/check-email", &json!({"email": "new@example.com"}), None).await;
    assert_eq!((free.status, free.body["is_duplicate"].clone()), (200, json!(false)));
    assert_eq!(auth(&address, "/auth/check-email", &json!({"email": "not-an-email"}), None).await.status, 422);

    // --- signup: the client cannot choose a role, an id, an owner or anything else the server decides.
    for (body, reason) in [
        (
            json!({"email": "x@example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": ["drama"], "role": "SUPER_ADMIN"}),
            "unknown_field",
        ),
        (
            json!({"email": "x@example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": ["drama"], "user_id": CAROL}),
            "unknown_field",
        ),
        (
            json!({"email": "x@example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": ["drama"], "is_deleted": false}),
            "unknown_field",
        ),
        (json!({"email": "x@example.com", "password": "short", "name": "홍길동", "favorite_genres": ["drama"]}), "password_length"),
        (json!({"email": "x@example.com", "password": "longenough1", "name": "홍", "favorite_genres": ["drama"]}), "name_length"),
        (
            json!({"email": "x@example.com", "password": "longenough1", "name": "열한글자열한글자열한글자", "favorite_genres": ["drama"]}),
            "name_length",
        ),
        (json!({"email": "x@example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": []}), "favorite_genres_required"),
        (json!({"email": "x@example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": ["opera"]}), "favorite_genre_unknown"),
        (json!({"email": "x@example", "password": "longenough1", "name": "홍길동", "favorite_genres": ["drama"]}), "email_invalid"),
    ] {
        let refused = auth(&address, "/auth/signup", &body, None).await;
        assert_eq!(
            (refused.status, refused.body["code"].clone(), refused.body["reason"].clone()),
            (422, json!("WIRE.BAD_VALUE"), json!(reason)),
            "{body}"
        );
        assert!(!refused.headers.contains_key("set-cookie"));
    }
    assert_eq!(
        cell(&mut setup, "SELECT COUNT(*) FROM user WHERE email = 'x@example.com'").await.as_deref(),
        Some("0"),
        "a refused signup writes nothing"
    );

    let signup = auth(
        &address,
        "/auth/signup",
        &json!({"email": "Newbie@Example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": ["drama", "music", "drama"]}),
        None,
    )
    .await;
    assert_eq!(signup.status, 201, "{}", signup.body);
    let newbie = access(&signup);
    let cookie = set_cookie(&signup);
    for attribute in ["HttpOnly", "Secure", "SameSite=None", "Path=/auth", "Max-Age=604800"] {
        assert!(cookie.contains(attribute), "{attribute} in {cookie}");
    }
    assert_eq!(signup.headers.get("cache-control").map(String::as_str), Some("no-store"));
    assert_eq!(signup.headers.get("access-control-allow-origin").map(String::as_str), Some(ORIGIN));
    assert_eq!(signup.headers.get("access-control-allow-credentials").map(String::as_str), Some("true"));
    assert!(!signup.body.to_string().contains("refresh"), "the refresh token is only in the cookie: {}", signup.body);
    let newbie_claims = claims(&newbie);
    let mut keys: Vec<_> = newbie_claims.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["aud", "exp", "iat", "iss", "nbf", "sid", "sub"], "no role, actor or owner claim: {newbie_claims}");
    let user_id = newbie_claims["sub"].as_str().unwrap().to_owned();
    assert_eq!(user_id.len(), 36);
    let row: (String, String, String, i64, i64, i64) = setup
        .exec_first("SELECT email, role, password, is_deleted, is_tutorial_done, is_verify_email_done FROM user WHERE user_id = ?", (&user_id,))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.0.as_str(), row.1.as_str(), row.3, row.4, row.5), ("Newbie@Example.com", "GENERAL", 0, 0, 0), "role is the server's default");
    assert!(row.2.starts_with("scrypt:32768:8:1$") && !row.2.contains("longenough1"), "a Werkzeug scrypt hash, not the password: {}", row.2);
    let genres: Vec<String> = setup.exec("SELECT genre FROM user_favorite_genre WHERE user_id = ? ORDER BY genre", (&user_id,)).await.unwrap();
    assert_eq!(genres, ["drama", "music"], "duplicates in the request are one row");
    assert_eq!(cell(&mut setup, "SELECT COUNT(*) FROM user WHERE created_at IS NULL OR updated_at IS NULL").await.as_deref(), Some("0"));
    let duplicate = auth(
        &address,
        "/auth/signup",
        &json!({"email": "newbie@example.com", "password": "longenough1", "name": "홍길동", "favorite_genres": ["drama"]}),
        None,
    )
    .await;
    assert_eq!(
        (duplicate.status, duplicate.body["code"].clone(), duplicate.body["reason"].clone()),
        (409, json!("WIRE.ALREADY_EXISTS"), json!("duplicate_email"))
    );

    // --- concurrent signups: different emails all succeed (no gap-lock deadlock), the same email succeeds exactly once.
    let mut signups = Vec::new();
    for index in 0..6 {
        let address = address.clone();
        let email = if index < 4 { format!("burst{index}@example.com") } else { "twin@example.com".to_owned() };
        signups.push(tokio::spawn(async move {
            auth(
                &address,
                "/auth/signup",
                &json!({"email": email, "password": "longenough1", "name": "동시가입", "favorite_genres": ["music"]}),
                None,
            )
            .await
        }));
    }
    let mut replies = Vec::new();
    for signup in signups {
        replies.push(signup.await.unwrap());
    }
    let mut statuses: Vec<u16> = replies.iter().map(|reply| reply.status).collect();
    let bodies: Vec<String> = replies.iter().map(|reply| reply.body.to_string()).collect();
    assert_eq!(statuses[..4], [201, 201, 201, 201], "{statuses:?} {bodies:?}");
    statuses[4..].sort_unstable();
    assert_eq!(statuses[4..], [201, 409], "{statuses:?} {bodies:?}");
    assert_eq!(cell(&mut setup, "SELECT COUNT(*) FROM user WHERE email = 'twin@example.com'").await.as_deref(), Some("1"));

    // --- the token works as the bound actor, and as nothing more.
    let own = read_users(Some(newbie.clone())).await;
    assert_eq!(own["ok"], true, "{own}");
    assert_eq!(own["rows"].as_array().unwrap().len(), 1, "a GENERAL account sees only itself: {own}");
    assert_eq!((own["rows"][0]["id"].as_str(), own["rows"][0]["role"].as_str()), (Some(user_id.as_str()), Some("GENERAL")));
    let session = post(&address, "/session", Some(&newbie), "", &json!({})).await;
    assert_eq!(session["ok"], true, "{session}");
    // Guest: no token is an anonymous session that reads no account.
    let guest = post(&address, "/session", None, "", &json!({})).await;
    assert_eq!(guest["ok"], true, "{guest}");
    assert!(guest["principal"]["actorId"].is_null(), "{guest}");
    let guest_users = read_users(None).await;
    assert_eq!(guest_users["rows"].as_array().map(Vec::len), Some(0), "a guest reads no account row: {guest_users}");

    // --- login: the original's account written with Werkzeug, a wrong password, an unknown email, a placeholder hash.
    let wrong = auth(&address, "/auth/login", &json!({"email": "legacy@example.com", "password": "password124"}), None).await;
    let unknown = auth(&address, "/auth/login", &json!({"email": "nobody@example.com", "password": "password123"}), None).await;
    let placeholder = auth(&address, "/auth/login", &json!({"email": "a@example.com", "password": "pbkdf2:hash-a"}), None).await;
    for refused in [&wrong, &unknown, &placeholder] {
        assert_eq!(
            (refused.status, refused.body["code"].clone(), refused.body["reason"].clone()),
            (401, json!("WIRE.UNAUTHENTICATED"), json!("invalid_credentials"))
        );
        assert!(!refused.headers.contains_key("set-cookie"));
    }
    let without_request_id = |reply: &Reply| {
        let mut body = reply.body.clone();
        body.as_object_mut().unwrap().remove("requestId");
        body
    };
    assert_eq!(without_request_id(&wrong), without_request_id(&unknown), "a wrong password and an unknown email answer alike");
    assert_eq!(
        auth(&address, "/auth/login", &json!({"email": "legacy@example.com", "password": "password123", "role": "ADMIN"}), None).await.status,
        422
    );
    let login = auth(&address, "/auth/login", &json!({"email": "legacy@example.com", "password": "password123"}), None).await;
    assert_eq!(login.status, 200, "{}", login.body);
    assert_eq!(claims(&access(&login))["sub"], LEGACY);
    let legacy_cookie = set_cookie(&login);
    let legacy_read = read_users(Some(access(&login))).await;
    assert_eq!(legacy_read["rows"][0]["id"], LEGACY, "the first login bound the original account to its actor: {legacy_read}");

    // --- reissue rotates; a used refresh token ends the session (the original accepted it again).
    let first_refresh = refresh_value(&legacy_cookie);
    let reissued = auth(&address, "/auth/reissue", &json!({}), Some(&first_refresh)).await;
    assert_eq!(reissued.status, 200, "{}", reissued.body);
    let second_refresh = refresh_value(&set_cookie(&reissued));
    assert_ne!(second_refresh, first_refresh);
    assert_eq!(claims(&access(&reissued))["sid"], claims(&access(&login))["sid"], "one login session");
    let replay = auth(&address, "/auth/reissue", &json!({}), Some(&first_refresh)).await;
    assert_eq!((replay.status, replay.body["reason"].clone()), (401, json!("refresh_token_reused")));
    assert!(set_cookie(&replay).contains("Max-Age=0"), "a refused refresh clears the cookie");
    let after_replay = auth(&address, "/auth/reissue", &json!({}), Some(&second_refresh)).await;
    assert_eq!(
        (after_replay.status, after_replay.body["reason"].clone()),
        (401, json!("session_revoked")),
        "the thief's replay also ended the owner's session"
    );
    let dead = post(&address, "/session", Some(&access(&reissued)), "", &json!({})).await;
    assert_eq!(dead["code"], "WIRE.UNAUTHENTICATED", "an access token dies with its session: {dead}");
    for bad_cookie in [None, Some("0".repeat(64)), Some("zz".to_string()), Some("a".repeat(65))] {
        let refused = auth(&address, "/auth/reissue", &json!({}), bad_cookie.as_deref()).await;
        assert_eq!(refused.status, 401, "{bad_cookie:?}");
    }

    // --- eight browsers' worth of the same refresh token: one wins.
    let racer = auth(&address, "/auth/login", &json!({"email": "legacy@example.com", "password": "password123"}), None).await;
    let race_cookie = refresh_value(&set_cookie(&racer));
    let mut racers = Vec::new();
    for _ in 0..8 {
        let (address, cookie) = (address.clone(), race_cookie.clone());
        racers.push(tokio::spawn(async move { auth(&address, "/auth/reissue", &json!({}), Some(&cookie)).await.status }));
    }
    let mut statuses = Vec::new();
    for racer in racers {
        statuses.push(racer.await.unwrap());
    }
    assert_eq!(statuses.iter().filter(|status| **status == 200).count(), 1, "{statuses:?}");

    // --- logout ends the session: the cookie, and the access token with it.
    let session_login = auth(&address, "/auth/login", &json!({"email": "legacy@example.com", "password": "password123"}), None).await;
    let (session_access, session_refresh) = (access(&session_login), refresh_value(&set_cookie(&session_login)));
    assert_eq!(post(&address, "/session", Some(&session_access), "", &json!({})).await["ok"], true);
    let logout = auth(&address, "/auth/logout", &json!({}), Some(&session_refresh)).await;
    assert_eq!(logout.status, 200);
    assert!(set_cookie(&logout).contains("Max-Age=0"));
    assert_eq!(auth(&address, "/auth/reissue", &json!({}), Some(&session_refresh)).await.status, 401, "the logged-out cookie no longer reissues");
    assert_eq!(
        post(&address, "/session", Some(&session_access), "", &json!({})).await["code"],
        "WIRE.UNAUTHENTICATED",
        "nor does its access token work"
    );
    let again = auth(&address, "/auth/logout", &json!({}), Some(&session_refresh)).await;
    assert_eq!((again.status, again.body["ok"].clone()), (200, json!(true)), "logout is idempotent and no oracle");
    assert_eq!(auth(&address, "/auth/logout", &json!({}), None).await.status, 200);

    // --- a suspended account (the transition the admin screen runs sets is_deleted) loses its sessions at the next refresh and cannot log in.
    let suspended = auth(&address, "/auth/login", &json!({"email": "newbie@example.com", "password": "longenough1"}), None).await;
    let suspended_cookie = refresh_value(&set_cookie(&suspended));
    setup.exec_drop("UPDATE user SET is_deleted = 1 WHERE user_id = ?", (&user_id,)).await.unwrap();
    let denied = auth(&address, "/auth/reissue", &json!({}), Some(&suspended_cookie)).await;
    assert_eq!((denied.status, denied.body["reason"].clone()), (401, json!("account_inactive")));
    assert_eq!(auth(&address, "/auth/login", &json!({"email": "newbie@example.com", "password": "longenough1"}), None).await.status, 401);
    assert_eq!(
        cell(&mut setup, &format!("SELECT COUNT(*) FROM `{META}`.aip_auth_sessions WHERE subject = '{user_id}' AND revoked_at IS NULL"))
            .await
            .as_deref(),
        Some("0")
    );
    setup.exec_drop("UPDATE user SET is_deleted = 0 WHERE user_id = ?", (&user_id,)).await.unwrap();

    // --- an operator's `principal revoke` outranks a correct password: login must not undo it.
    run(&env, &["principal", "revoke", "--subject", &user_id], &config).await.expect("revoke");
    let revoked = auth(&address, "/auth/login", &json!({"email": "newbie@example.com", "password": "longenough1"}), None).await;
    assert_eq!((revoked.status, revoked.body["reason"].clone()), (403, json!("account_disabled")));
    assert_eq!(post(&address, "/session", Some(&newbie), "", &json!({})).await["code"], "WIRE.UNAUTHENTICATED");

    // --- privilege boundaries: the token is the only identity, and only the server's key makes one.
    let carol_login = auth(&address, "/auth/login", &json!({"email": "c@example.com", "password": carol_password}), None).await;
    assert_eq!(carol_login.status, 200, "{}", carol_login.body);
    let carol_all = read_users(Some(access(&carol_login))).await;
    assert_eq!(
        carol_all["rows"].as_array().unwrap().len(),
        12,
        "the SUPER_ADMIN reads every account (5 seed, legacy, signed-up, 5 from the burst) by the row policy and not by a claim: {carol_all}"
    );
    let carol_sid = claims(&access(&carol_login))["sid"].as_str().unwrap().to_owned();
    let mint = |subject: &str, sid: &str, key: &RsaPrivateKey, kid: &str, extra: Value| {
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some(kid.into());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let mut body = json!({"iss": ISSUER, "aud": AUDIENCE, "sub": subject, "iat": now, "nbf": now, "exp": now + 300});
        if !sid.is_empty() {
            body["sid"] = json!(sid);
        }
        for (name, value) in extra.as_object().unwrap() {
            body[name] = value.clone();
        }
        encode(&header, &body, &EncodingKey::from_rsa_der(key.to_pkcs1_der().unwrap().as_bytes())).unwrap()
    };
    for (label, forged) in [
        ("another user's session id under a normal user's subject", mint(LEGACY, &carol_sid, &private, "login-1", json!({}))),
        ("a token without a session id", mint(CAROL, "", &private, "login-1", json!({}))),
        ("a session that never existed", mint(CAROL, "00000000-0000-4000-8000-000000000000", &private, "login-1", json!({}))),
        ("an unknown signing key", mint(CAROL, &carol_sid, &other, "login-1", json!({}))),
    ] {
        let denied = post(&address, "/session", Some(&forged), "", &json!({})).await;
        assert_eq!(denied["code"], "WIRE.UNAUTHENTICATED", "{label}: {denied}");
    }
    // A valid session of a normal account with an extra `role` claim, signed by the server's own key: the claim changes nothing.
    let fresh = auth(&address, "/auth/login", &json!({"email": "legacy@example.com", "password": "password123"}), None).await;
    let fresh_sid = claims(&access(&fresh))["sid"].as_str().unwrap().to_owned();
    let claimed = mint(LEGACY, &fresh_sid, &private, "login-1", json!({"role": "SUPER_ADMIN", "actorId": CAROL}));
    let claimed_rows = read_users(Some(claimed)).await;
    assert_eq!(claimed_rows["rows"].as_array().map(Vec::len), Some(1), "a role claim does not widen what a GENERAL account reads: {claimed_rows}");
    assert_eq!(claimed_rows["rows"][0]["id"], LEGACY);
    // A logged-in GENERAL account cannot raise its own role or touch an administrator, and nothing changes in the table.
    let fresh_token = access(&fresh);
    for (key, request) in [
        ("auth-escalate-self", json!({"apply": "User.changeRole", "target": {"ids": [LEGACY]}, "values": {"role": "SUPER_ADMIN"}})),
        ("auth-escalate-other", json!({"apply": "User.changeRole", "target": {"ids": [CAROL]}, "values": {"role": "GENERAL"}})),
        ("auth-deactivate-admin", json!({"apply": "User.deactivate", "target": {"ids": [CAROL]}})),
    ] {
        let attempt = post(&address, "/apply", Some(&fresh_token), &fingerprint, &json!({"key": key, "request": request})).await;
        assert_ne!(attempt["ok"], true, "{key}: {attempt}");
    }
    let roles: Vec<(String, String)> =
        setup.exec("SELECT user_id, role FROM user WHERE user_id IN (?, ?) ORDER BY user_id", (LEGACY, CAROL)).await.unwrap();
    assert_eq!(roles, [(CAROL.to_owned(), "SUPER_ADMIN".to_owned()), (LEGACY.to_owned(), "GENERAL".to_owned())]);
    assert_eq!(cell(&mut setup, &format!("SELECT is_deleted FROM user WHERE user_id = '{CAROL}'")).await.as_deref(), Some("0"));
    let tampered = format!("{}x", access(&carol_login));
    assert_eq!(post(&address, "/session", Some(&tampered), "", &json!({})).await["code"], "WIRE.UNAUTHENTICATED");

    // --- transport: the origin allow-list, credentials only for the allowed origin, JSON only, POST only, bounded bodies.
    let preflight = http(
        &address,
        "OPTIONS",
        "/auth/login",
        &[("Origin", ORIGIN), ("Access-Control-Request-Method", "POST"), ("Access-Control-Request-Headers", "content-type")],
        None,
    )
    .await;
    assert_eq!(preflight.headers.get("access-control-allow-credentials").map(String::as_str), Some("true"), "{:?}", preflight.headers);
    let foreign = http(
        &address,
        "POST",
        "/auth/login",
        &[("Origin", "http://localhost:9999"), ("Content-Type", "application/json")],
        Some("{\"email\":\"legacy@example.com\",\"password\":\"password123\"}"),
    )
    .await;
    assert_eq!(foreign.body["code"], "WIRE.ORIGIN_NOT_ALLOWED", "{}", foreign.body);
    assert!(!foreign.headers.contains_key("set-cookie") && !foreign.headers.contains_key("access-control-allow-credentials"));
    let form = http(
        &address,
        "POST",
        "/auth/login",
        &[("Origin", ORIGIN), ("Content-Type", "application/x-www-form-urlencoded")],
        Some("{\"email\":\"legacy@example.com\",\"password\":\"password123\"}"),
    )
    .await;
    assert_eq!(form.status, 400, "a body must declare itself JSON");
    assert_eq!(http(&address, "GET", "/auth/login", &[], None).await.status, 405);
    let huge = format!("{{\"email\":\"{}\"}}", "a".repeat(20_000));
    assert_eq!(http(&address, "POST", "/auth/check-email", &[("Content-Type", "application/json")], Some(&huge)).await.status, 413);
    assert_eq!(http(&address, "POST", "/auth/nope", &[("Content-Type", "application/json")], Some("{}")).await.body["code"], "WIRE.NOT_FOUND");
    // The check-email, login and signup bodies never echo a secret.
    for reply in [&wrong, &unknown, &signup, &login] {
        let text = reply.body.to_string();
        assert!(!text.contains("scrypt:") && !text.contains("password123") && !text.contains("longenough1"), "{text}");
    }

    // --- WBS 7.2.8: a hash that is weaker than today's default is replaced after a correct login, and only then.
    let cases = vectors["cases"].as_array().unwrap();
    let (weak_password, weak_hash) = (cases[4]["password"].as_str().unwrap(), cases[4]["hash"].as_str().unwrap());
    assert!(weak_hash.starts_with("pbkdf2:sha1:1000$"), "{weak_hash}");
    setup
        .exec_drop(
            "INSERT INTO user (user_id, email, password, name, role, is_tutorial_done, is_verify_email_done, is_deleted, created_at, updated_at) \
             VALUES ('88888888-8888-4888-8888-888888888888', 'weak@example.com', ?, '약한해시', 'GENERAL', 1, 1, 0, '2026-10-01 00:00:00', '2026-10-01 00:00:00')",
            (weak_hash,),
        )
        .await
        .unwrap();
    let stored = |email: &'static str| {
        let sql = format!("SELECT password FROM user WHERE email = '{email}'");
        async move { sql }
    };
    let weak_sql = stored("weak@example.com").await;
    let refused = auth(&address, "/auth/login", &json!({"email": "weak@example.com", "password": "not-the-password"}), None).await;
    assert_eq!(refused.status, 401);
    assert_eq!(cell(&mut setup, &weak_sql).await.as_deref(), Some(weak_hash), "a wrong password never rewrites the hash");
    let upgraded = auth(&address, "/auth/login", &json!({"email": "weak@example.com", "password": weak_password}), None).await;
    assert_eq!(upgraded.status, 200, "{}", upgraded.body);
    let new_hash = cell(&mut setup, &weak_sql).await.unwrap();
    assert!(new_hash.starts_with("scrypt:32768:8:1$") && new_hash != weak_hash, "the hash was replaced by today's default: {new_hash}");
    assert_eq!(
        cell(&mut setup, "SELECT updated_at FROM user WHERE email = 'weak@example.com'").await.as_deref(),
        Some("2026-10-01 00:00:00"),
        "the account's updated_at is not touched by a rehash"
    );
    let again = auth(&address, "/auth/login", &json!({"email": "weak@example.com", "password": weak_password}), None).await;
    assert_eq!(again.status, 200, "the new hash verifies the same password");
    assert_eq!(cell(&mut setup, &weak_sql).await.as_deref(), Some(new_hash.as_str()), "a current hash is left alone");
    let legacy_sql = stored("legacy@example.com").await;
    assert_eq!(cell(&mut setup, &legacy_sql).await.as_deref(), Some(legacy_hash), "the original's own scrypt hash is already the default");

    // --- WBS 7.2.8: the account screens (`/auth/me`, tutorial, profile, withdrawal) through the exposed reads and transitions.
    let signed = auth(
        &address,
        "/auth/signup",
        &json!({"email": "profile@example.com", "password": "longenough1", "name": "프로필", "favorite_genres": ["drama", "music"]}),
        None,
    )
    .await;
    assert_eq!(signed.status, 201, "{}", signed.body);
    let (member, member_cookie) = (access(&signed), refresh_value(&set_cookie(&signed)));
    let member_id = claims(&member)["sub"].as_str().unwrap().to_owned();
    let apply = |token: String, key: &'static str, request: Value| {
        let (address, fingerprint) = (address.clone(), fingerprint.clone());
        async move { post(&address, "/apply", Some(&token), &fingerprint, &json!({"key": key, "request": request})).await }
    };
    let me = |token: String| {
        let (address, fingerprint) = (address.clone(), fingerprint.clone());
        async move {
            let user = post(
                &address,
                "/read",
                Some(&token),
                &fingerprint,
                &json!({"query": {"read": "User", "select": ["id", "email", "name", "profile_image_id", "is_tutorial_done", "is_verify_email_done", "role", "created_at"]}}),
            )
            .await;
            let genres =
                post(&address, "/read", Some(&token), &fingerprint, &json!({"query": {"read": "UserFavoriteGenre", "select": ["id", "genre"]}}))
                    .await;
            (user, genres)
        }
    };
    let (profile, genre_rows) = me(member.clone()).await;
    assert_eq!(profile["rows"].as_array().map(Vec::len), Some(1), "{profile}");
    let row = &profile["rows"][0];
    assert_eq!(
        (
            row["id"].as_str(),
            row["email"].as_str(),
            row["name"].as_str(),
            row["role"].as_str(),
            row["is_tutorial_done"].clone(),
            row["is_verify_email_done"].clone()
        ),
        (Some(member_id.as_str()), Some("profile@example.com"), Some("프로필"), Some("GENERAL"), json!(false), json!(false)),
        "/auth/me: the member's own row"
    );
    assert!(row["profile_image_id"].is_null() || row["profile_image_id"] == json!(0), "{row}");
    let mut names: Vec<_> = genre_rows["rows"].as_array().unwrap().iter().map(|g| g["genre"].as_str().unwrap().to_owned()).collect();
    names.sort();
    assert_eq!(names, ["drama", "music"], "and the member's favorite genres");

    // tutorial: once done, a repeat changes nothing; nobody else's row can be completed.
    let others = apply(member.clone(), "tutorial-other", json!({"apply": "User.completeTutorial", "target": {"ids": [LEGACY]}})).await;
    assert_ne!(others["ok"], true, "{others}");
    assert_eq!(cell(&mut setup, &format!("SELECT is_tutorial_done FROM user WHERE user_id = '{LEGACY}'")).await.as_deref(), Some("1"));
    let done = apply(member.clone(), "tutorial-1", json!({"apply": "User.completeTutorial", "target": {"ids": [member_id]}})).await;
    assert_eq!(done["ok"], true, "{done}");
    let sql = format!("SELECT CONCAT(is_tutorial_done, ' ', updated_at) FROM user WHERE user_id = '{member_id}'");
    let after_first = cell(&mut setup, &sql).await.unwrap();
    assert!(after_first.starts_with("1 "), "{after_first}");
    setup.exec_drop("UPDATE user SET updated_at = '2026-10-02 00:00:00' WHERE user_id = ?", (&member_id,)).await.unwrap();
    let repeat = apply(member.clone(), "tutorial-2", json!({"apply": "User.completeTutorial", "target": {"ids": [member_id]}})).await;
    assert_eq!(repeat["ok"], true, "{repeat}");
    assert_eq!(cell(&mut setup, &sql).await.as_deref(), Some("1 2026-10-02 00:00:00"), "a repeat leaves the row as it is");

    // profile: each value only when sent; the genres are replaced in the same bundle.
    let rename =
        apply(member.clone(), "profile-1", json!({"apply": "User.updateProfile", "target": {"ids": [member_id]}, "values": {"name": "새이름"}}))
            .await;
    assert_eq!(rename["ok"], true, "{rename}");
    let image =
        apply(member.clone(), "profile-2", json!({"apply": "User.updateProfile", "target": {"ids": [member_id]}, "values": {"profile_image_id": 7}}))
            .await;
    assert_eq!(image["ok"], true, "{image}");
    let row: (String, Option<i64>) =
        setup.exec_first("SELECT name, profile_image_id FROM user WHERE user_id = ?", (&member_id,)).await.unwrap().unwrap();
    assert_eq!(row, ("새이름".to_owned(), Some(7)), "a value that was not sent stays");
    let foreign =
        apply(member.clone(), "profile-foreign", json!({"apply": "User.updateProfile", "target": {"ids": [LEGACY]}, "values": {"name": "가로채기"}}))
            .await;
    assert_ne!(foreign["ok"], true, "{foreign}");
    let forbidden =
        apply(member.clone(), "profile-role", json!({"apply": "User.updateProfile", "target": {"ids": [member_id]}, "values": {"role": "ADMIN"}}))
            .await;
    assert_ne!(forbidden["ok"], true, "{forbidden}");
    assert_eq!(cell(&mut setup, &format!("SELECT role FROM user WHERE user_id = '{member_id}'")).await.as_deref(), Some("GENERAL"));
    let (_, genre_rows) = me(member.clone()).await;
    let old_ids: Vec<Value> = genre_rows["rows"].as_array().unwrap().iter().map(|g| g["id"].clone()).collect();
    assert_eq!(old_ids.len(), 2);
    let replaced = apply(
        member.clone(),
        "profile-genres",
        json!({"atomic": [
            {"apply": "UserFavoriteGenre.remove", "target": {"ids": old_ids}},
            {"create": "UserFavoriteGenre", "values": {"user": member_id, "genre": "horror"}},
            {"create": "UserFavoriteGenre", "values": {"user": member_id, "genre": "music"}},
        ]}),
    )
    .await;
    assert_eq!(replaced["ok"], true, "{replaced}");
    let genres: Vec<String> =
        setup.exec("SELECT genre FROM user_favorite_genre WHERE user_id = ? ORDER BY CAST(genre AS CHAR)", (&member_id,)).await.unwrap();
    assert_eq!(genres, ["horror", "music"], "the old genres are gone and the new ones are in");
    let stolen = apply(
        member.clone(),
        "profile-genre-steal",
        json!({"atomic": [{"create": "UserFavoriteGenre", "values": {"user": LEGACY, "genre": "horror"}}]}),
    )
    .await;
    assert_ne!(stolen["ok"], true, "{stolen}");
    let duplicate = apply(
        member.clone(),
        "profile-genre-twice",
        json!({"atomic": [{"create": "UserFavoriteGenre", "values": {"user": member_id, "genre": "horror"}}]}),
    )
    .await;
    assert_ne!(duplicate["ok"], true, "a genre the member already has is refused by the unique index: {duplicate}");
    let genres: Vec<String> =
        setup.exec("SELECT genre FROM user_favorite_genre WHERE user_id = ? ORDER BY CAST(genre AS CHAR)", (&member_id,)).await.unwrap();
    assert_eq!(genres, ["horror", "music"], "and a refused bundle changes nothing");

    // withdrawal: the member's own transition, then every session of the account is over.
    let second = auth(&address, "/auth/login", &json!({"email": "profile@example.com", "password": "longenough1"}), None).await;
    let (second_access, second_cookie) = (access(&second), refresh_value(&set_cookie(&second)));
    let withdrawn = apply(member.clone(), "withdraw-1", json!({"apply": "User.withdraw", "target": {"ids": [member_id]}})).await;
    assert_eq!(withdrawn["ok"], true, "{withdrawn}");
    assert_eq!(cell(&mut setup, &format!("SELECT is_deleted FROM user WHERE user_id = '{member_id}'")).await.as_deref(), Some("1"));
    assert_eq!(
        cell(&mut setup, &format!("SELECT COUNT(*) FROM user_favorite_genre WHERE user_id = '{member_id}'")).await.as_deref(),
        Some("0"),
        "the favorite genres are deleted with the account, as the original does"
    );
    let (gone, _) = me(member.clone()).await;
    assert_eq!(gone["rows"].as_array().map(Vec::len), Some(0), "a withdrawn account reads nothing, not even its own row: {gone}");
    for (cookie, reason) in [(&member_cookie, "account_inactive"), (&second_cookie, "session_revoked")] {
        let denied = auth(&address, "/auth/reissue", &json!({}), Some(cookie)).await;
        assert_eq!((denied.status, denied.body["reason"].clone()), (401, json!(reason)), "{}", denied.body);
    }
    assert_eq!(
        cell(&mut setup, &format!("SELECT COUNT(*) FROM `{META}`.aip_auth_sessions WHERE subject = '{member_id}' AND revoked_at IS NULL"))
            .await
            .as_deref(),
        Some("0"),
        "the first refresh after the withdrawal ended every session of the account, the other device's too"
    );
    assert_eq!(post(&address, "/session", Some(&second_access), "", &json!({})).await["code"], "WIRE.UNAUTHENTICATED");
    assert_eq!(auth(&address, "/auth/login", &json!({"email": "profile@example.com", "password": "longenough1"}), None).await.status, 401);

    // --- WBS 7.2.8: proving the mailbox (email code, password reset token), for a logged-in account only.
    let mail_post = |path: &'static str, token: Option<String>, body: Value| {
        let address = address.clone();
        async move {
            let bearer = token.map(|token| format!("Bearer {token}"));
            let mut headers = vec![("Content-Type", "application/json"), ("Origin", ORIGIN)];
            if let Some(bearer) = bearer.as_deref() {
                headers.push(("Authorization", bearer));
            }
            http(&address, "POST", path, &headers, Some(&body.to_string())).await
        }
    };
    let signed = auth(
        &address,
        "/auth/signup",
        &json!({"email": "mailer@example.com", "password": "longenough1", "name": "메일러", "favorite_genres": ["drama"]}),
        None,
    )
    .await;
    let (mailer_token, mailer_id) = (access(&signed), claims(&access(&signed))["sub"].as_str().unwrap().to_owned());
    let other = auth(
        &address,
        "/auth/signup",
        &json!({"email": "bystander@example.com", "password": "longenough1", "name": "곁사람", "favorite_genres": ["drama"]}),
        None,
    )
    .await;
    let other_token = access(&other);
    let challenges = |purpose: &'static str, who: String| {
        format!("SELECT CONCAT(attempts, ' ', HEX(secret_hash)) FROM `{META}`.aip_auth_challenges WHERE subject = '{who}' AND purpose = '{purpose}'")
    };
    let count_challenges = format!("SELECT COUNT(*) FROM `{META}`.aip_auth_challenges WHERE subject = '{mailer_id}'");
    let last_code = |mailer: &Mailer| mailer.sent.lock().unwrap().last().unwrap()["code"].as_str().unwrap().to_owned();

    for path in ["/auth/email/verification", "/auth/email/verify", "/auth/password/verification", "/auth/password"] {
        let nobody = mail_post(path, None, json!({})).await;
        assert_eq!((nobody.status, nobody.body["reason"].clone()), (401, json!("token_invalid")), "{path} without a token");
        let forged = mail_post(path, Some(format!("{mailer_token}x")), json!({})).await;
        assert_eq!(forged.status, 401, "{path} with a tampered token");
    }
    assert_eq!(
        mail_post("/auth/email/verification", Some(mailer_token.clone()), json!({"email": "someone@else.com"})).await.status,
        422,
        "the address is the account's, not a request value"
    );
    assert!(mailer.sent.lock().unwrap().is_empty(), "nothing was mailed for the refused calls");

    // A code goes to the account's own address, and only its digest is kept.
    let sent = mail_post("/auth/email/verification", Some(mailer_token.clone()), json!({})).await;
    assert_eq!((sent.status, sent.body["ok"].clone()), (200, json!(true)), "{}", sent.body);
    let mail = mailer.sent.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (mail["to"].as_str(), mail["template"].as_str(), mail["expires_in_secs"].clone()),
        (Some("mailer@example.com"), Some("email-verification"), json!(300))
    );
    let code = last_code(&mailer);
    assert!(code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()), "{code}");
    let stored = cell(&mut setup, &challenges("email_code", mailer_id.clone())).await.unwrap();
    assert!(stored.starts_with("0 ") && !stored.to_lowercase().contains(&code), "one pending code, no attempts, not stored in the clear: {stored}");
    assert_eq!(cell(&mut setup, &format!("SELECT is_verify_email_done FROM user WHERE user_id = '{mailer_id}'")).await.as_deref(), Some("0"));

    // A wrong code is refused and counted; a malformed one is not even a guess; someone else's session cannot spend it.
    let wrong_code = if code == "000000" { "000001" } else { "000000" };
    let wrong = mail_post("/auth/email/verify", Some(mailer_token.clone()), json!({"code": wrong_code})).await;
    assert_eq!((wrong.status, wrong.body["reason"].clone()), (400, json!("verification_invalid")), "{}", wrong.body);
    assert!(cell(&mut setup, &challenges("email_code", mailer_id.clone())).await.unwrap().starts_with("1 "), "the wrong answer was counted");
    for malformed in [json!({"code": "12345"}), json!({"code": "12345a"}), json!({"code": 123456}), json!({})] {
        assert_eq!(mail_post("/auth/email/verify", Some(mailer_token.clone()), malformed).await.status, 422);
    }
    let bystander = mail_post("/auth/email/verify", Some(other_token.clone()), json!({"code": code})).await;
    assert_eq!(bystander.status, 400, "a code belongs to the account it was sent to: {}", bystander.body);
    let right = mail_post("/auth/email/verify", Some(mailer_token.clone()), json!({"code": code})).await;
    assert_eq!((right.status, right.body["ok"].clone()), (200, json!(true)), "{}", right.body);
    assert_eq!(cell(&mut setup, &format!("SELECT is_verify_email_done FROM user WHERE user_id = '{mailer_id}'")).await.as_deref(), Some("1"));
    assert_eq!(cell(&mut setup, &count_challenges).await.as_deref(), Some("0"), "a spent code is gone");
    assert_eq!(mail_post("/auth/email/verify", Some(mailer_token.clone()), json!({"code": code})).await.status, 400, "and cannot be used twice");

    // Guessing is bounded: five wrong answers lock the code, even against the right one.
    mail_post("/auth/email/verification", Some(mailer_token.clone()), json!({})).await;
    let code = last_code(&mailer);
    let wrong_code = if code == "000000" { "000001" } else { "000000" };
    for _ in 0..5 {
        assert_eq!(mail_post("/auth/email/verify", Some(mailer_token.clone()), json!({"code": wrong_code})).await.status, 400);
    }
    let locked = mail_post("/auth/email/verify", Some(mailer_token.clone()), json!({"code": code})).await;
    assert_eq!((locked.status, locked.body["reason"].clone()), (429, json!("too_many_attempts")), "{}", locked.body);
    assert_eq!(
        mail_post("/auth/password/verification", Some(mailer_token.clone()), json!({"code": code})).await.status,
        429,
        "the lock holds for the other route too"
    );
    // A new code replaces the locked one and starts counting again.
    mail_post("/auth/email/verification", Some(mailer_token.clone()), json!({})).await;
    assert!(cell(&mut setup, &challenges("email_code", mailer_id.clone())).await.unwrap().starts_with("0 "));

    // An expired code is the same as none.
    setup
        .exec_drop(
            format!("UPDATE `{META}`.aip_auth_challenges SET expires_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) WHERE subject = ?"),
            (&mailer_id,),
        )
        .await
        .unwrap();
    let expired = mail_post("/auth/email/verify", Some(mailer_token.clone()), json!({"code": last_code(&mailer)})).await;
    assert_eq!((expired.status, expired.body["reason"].clone()), (400, json!("verification_invalid")), "{}", expired.body);
    assert_eq!(cell(&mut setup, &count_challenges).await.as_deref(), Some("0"), "and the expired row is cleaned up");

    // A mail the provider refuses leaves no redeemable code behind.
    mailer.fail.store(true, Ordering::SeqCst);
    let failed = mail_post("/auth/email/verification", Some(mailer_token.clone()), json!({})).await;
    assert_eq!((failed.status, failed.body["reason"].clone()), (503, json!("mail_failed")), "{}", failed.body);
    assert_eq!(cell(&mut setup, &count_challenges).await.as_deref(), Some("0"));
    mailer.fail.store(false, Ordering::SeqCst);

    // Password change: code -> reset token -> new password. The token belongs to the account and is spent by its use.
    let other_session = auth(&address, "/auth/login", &json!({"email": "mailer@example.com", "password": "longenough1"}), None).await;
    let other_session_access = access(&other_session);
    mail_post("/auth/email/verification", Some(mailer_token.clone()), json!({})).await;
    let code = last_code(&mailer);
    assert_eq!(
        mail_post("/auth/password", Some(mailer_token.clone()), json!({"reset_token": "no-such-token", "new_password": "brand-new-pass1"}))
            .await
            .status,
        400
    );
    let exchanged = mail_post("/auth/password/verification", Some(mailer_token.clone()), json!({"code": code})).await;
    assert_eq!(exchanged.status, 200, "{}", exchanged.body);
    let reset = exchanged.body["reset_token"].as_str().unwrap().to_owned();
    assert!(reset.len() == 64 && reset.bytes().all(|b| b.is_ascii_hexdigit()), "{reset}");
    assert!(
        !cell(&mut setup, &challenges("reset_token", mailer_id.clone())).await.unwrap().to_lowercase().contains(&reset),
        "the token is stored as a digest"
    );
    assert_eq!(cell(&mut setup, &challenges("email_code", mailer_id.clone())).await, None, "the code was spent by the exchange");
    let too_short = mail_post("/auth/password", Some(mailer_token.clone()), json!({"reset_token": reset, "new_password": "short"})).await;
    assert_eq!((too_short.status, too_short.body["reason"].clone()), (422, json!("password_length")));
    let stolen = mail_post("/auth/password", Some(other_token.clone()), json!({"reset_token": reset, "new_password": "brand-new-pass1"})).await;
    assert_eq!(stolen.status, 400, "another account cannot use the token: {}", stolen.body);
    assert_eq!(
        auth(&address, "/auth/login", &json!({"email": "mailer@example.com", "password": "longenough1"}), None).await.status,
        200,
        "the password is unchanged so far"
    );
    let changed = mail_post("/auth/password", Some(mailer_token.clone()), json!({"reset_token": reset, "new_password": "brand-new-pass1"})).await;
    assert_eq!((changed.status, changed.body["ok"].clone()), (200, json!(true)), "{}", changed.body);
    let new_hash = cell(&mut setup, "SELECT password FROM user WHERE email = 'mailer@example.com'").await.unwrap();
    assert!(new_hash.starts_with("scrypt:32768:8:1$") && !new_hash.contains("brand-new-pass1"), "{new_hash}");
    assert_eq!(
        auth(&address, "/auth/login", &json!({"email": "mailer@example.com", "password": "longenough1"}), None).await.status,
        401,
        "the old password is dead"
    );
    assert_eq!(auth(&address, "/auth/login", &json!({"email": "mailer@example.com", "password": "brand-new-pass1"}), None).await.status, 200);
    assert_eq!(post(&address, "/session", Some(&mailer_token), "", &json!({})).await["ok"], true, "the session that changed it stays");
    assert_eq!(
        post(&address, "/session", Some(&other_session_access), "", &json!({})).await["code"],
        "WIRE.UNAUTHENTICATED",
        "every other login ended"
    );
    assert_eq!(
        mail_post("/auth/password", Some(mailer_token.clone()), json!({"reset_token": reset, "new_password": "brand-new-pass2"})).await.status,
        400,
        "a token is used once"
    );
    assert_eq!(cell(&mut setup, &count_challenges).await.as_deref(), Some("0"));
    assert_eq!(mailer.sent.lock().unwrap().len(), 5, "one mail per code request, the refused one included, and none for the refused calls");
}
