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
use std::{collections::HashMap, path::Path, process::Stdio, time::Duration};
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
// The definition declares the home and watch list extensions, and `serve` refuses a worker directory that lacks any of them.
const LIST_EXTENSIONS: [(&str, &str); 4] = [
    ("slice13b_lists.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice13b_lists.py")),
    ("slice13d_recommend.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/slice13d_recommend.py")),
    ("recommendation_math.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/recommendation_math.py")),
    ("recommendation_pool_builder.py", include_str!("../../../product/tests/fixtures/facereview-mysql/extensions/recommendation_pool_builder.py")),
];
const META: &str = "aip_t_auth_meta";
const SOURCE: &str = "aip_t_auth_src";
const ISSUER: &str = "https://login.facereview.example";
const AUDIENCE: &str = "aip-api";
const ORIGIN: &str = "http://localhost:5173";
const CAROL: &str = "33333333-3333-4333-8333-333333333333";
const LEGACY: &str = "77777777-7777-4777-8777-777777777777";

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
    command.env("AIP_T_AUTH_ADOPT_URL", &env.adopt_url).env("AIP_T_AUTH_RUNTIME_URL", &env.runtime_url);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh) and a worker sandbox; run with -- --ignored"]
async fn facereview_accounts_log_in_refresh_and_log_out_without_the_originals_replay_defect() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("facereview.aip");
    std::fs::write(&file, format!("{DEFINITION}\n{OPERATIONS}")).unwrap();
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
    // An account the original wrote: a real Werkzeug 3.0.2 scrypt hash of "password123" (the seed's hashes are placeholders).
    let vectors: Value = serde_json::from_str(WERKZEUG).unwrap();
    let (legacy_password, legacy_hash) = (vectors["cases"][0]["password"].as_str().unwrap(), vectors["cases"][0]["hash"].as_str().unwrap());
    assert_eq!(legacy_password, "password123");
    setup
        .exec_drop(
            "INSERT INTO user (user_id, email, password, name, role, is_tutorial_done, is_verify_email_done, is_deleted, created_at, updated_at) \
             VALUES (?, 'legacy@example.com', ?, '레거시', 'GENERAL', 1, 1, 0, NOW(), NOW())",
            (LEGACY, legacy_hash),
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
    let env = Env { adopt_url: admin_url.clone(), runtime_url: format!("mysql://aip_t_auth_rt:runtime_test_only@{host}/{SOURCE}") };

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
    let config = directory.path().join("service.json");
    let write_config = |credentials: Value| {
        std::fs::write(
            &config,
            json!({
                "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
                "database_url_env": "AIP_T_AUTH_RUNTIME_URL", "migrate_database_url_env": "AIP_T_AUTH_ADOPT_URL",
                "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null, "allowed_origins": [ORIGIN],
                "auth": {"issuer": ISSUER, "audience": AUDIENCE, "jwks": {"kind": "file", "path": "jwks.json"}},
                "workers": {"directory": "extensions", "language": "python"},
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
    });
    write_config(credentials.clone());
    let file_arg = file.to_str().unwrap();

    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    let session_tables =
        cell(&mut setup, &format!("SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA='{META}' AND TABLE_NAME LIKE 'aip_auth_%'"))
            .await;
    assert_eq!(session_tables.as_deref(), Some("2"), "adoption creates the session tables");

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
}
