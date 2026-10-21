#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.9: the administrator lists of FaceReview (members, video requests, videos, comments) through the real `aip service`
//! binary and the MySQL 8.4 schema of the original (pinned new-facereview@7948b7e7).
//!
//! The expected answers are `slice13e-golden.json`: what the original's own `AdminService.get_users / get_video_requests /
//! get_videos / get_comments` returned for the same data (`reference/slice13e_oracle.py` runs the original and the real server side by
//! side and writes the file only after every case matched). The server answers `/read` rows plus a `/read` total; `compose_*` below
//! builds the original's JSON shape from them, which is the work an SDK or UI layer does. No original checkout or Flask is needed.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up`.
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

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
const MAPPING: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/mapping.json");
const SCHEMA: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/schema.sql");
const DEFAULTS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/db-defaults.sql");
const SEED: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/seed.sql");
const SEED_13D: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-seed.sql");
const SEED_13E: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13e-seed.sql");
const GOLDEN: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13e-golden.json");
const META: &str = "aip_t_adm_meta";
const SOURCE: &str = "aip_t_adm_src";
const ISSUER: &str = "https://issuer.example";
const SUBJECTS: [(&str, &str); 5] = [
    ("alice", "11111111-1111-4111-8111-111111111111"),
    ("bob", "22222222-2222-4222-8222-222222222222"),
    ("carol", "33333333-3333-4333-8333-333333333333"),
    ("dave", "44444444-4444-4444-8444-444444444444"),
    ("erin", "55555555-5555-4555-8555-555555555555"),
];

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
    command.env("AIP_T_ADM_ADOPT_URL", &env.adopt_url).env("AIP_T_ADM_RUNTIME_URL", &env.runtime_url);
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
    tokio::time::timeout(Duration::from_secs(60), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
    let (_, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    serde_json::from_str(body).unwrap_or(Value::Null)
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
    for chunk in script.split(";\n") {
        let statement: String = chunk.lines().filter(|line| !line.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
        if statement.trim().is_empty() {
            continue;
        }
        conn.query_drop(&statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
}

/// The original answers a naive `isoformat()`; the server answers the same UTC instant as RFC 3339.
fn stamp(value: &Value) -> Value {
    json!(value.as_str().unwrap().trim_end_matches("+00:00").trim_end_matches('Z'))
}

/// Python's `bool(flag)` of a nullable flag: NULL and 0 are false.
fn truthy(value: &Value) -> bool {
    value == &json!(true) || value.as_i64().is_some_and(|number| number != 0)
}

fn newest() -> Value {
    json!([{"field": "created_at", "dir": "desc"}])
}

fn either(left: &str, right: &str, keyword: &str) -> Value {
    json!({"or": [{"field": left, "op": "like", "value": keyword}, {"field": right, "op": "like", "value": keyword}]})
}

fn eq(field: &str, value: &Value) -> Value {
    json!({"field": field, "op": "eq", "value": value})
}

/// The envelope of every list: rows of the page, the total of the same filter and the paging fields.
fn envelope(key: &str, rows: Vec<Value>, total: &Value, page: u64, size: u64) -> Value {
    let mut out = serde_json::Map::new();
    out.insert(key.to_string(), Value::Array(rows));
    out.insert("total".into(), total.clone());
    out.insert("page".into(), json!(page));
    out.insert("size".into(), json!(size));
    out.insert("has_next".into(), json!((page - 1) * size + size < total.as_u64().unwrap()));
    Value::Object(out)
}

struct Server<'a> {
    address: &'a str,
    fingerprint: &'a str,
}

impl Server<'_> {
    async fn read(&self, token: &str, query: Value) -> Value {
        let reply = post(self.address, "/read", Some(token), self.fingerprint, &json!({"query": query})).await;
        assert_eq!(reply["ok"], true, "{reply}");
        reply
    }

    async fn total(&self, token: &str, aggregate: &str, filter: &[Value]) -> Value {
        let reply = self.read(token, json!({"aggregate": aggregate, "input": {}, "filter": filter})).await;
        reply["rows"][0].clone()
    }
}

async fn compose(server: &Server<'_>, token: &str, case: &Value) -> Value {
    let params = &case["params"];
    let (page, size) = (params["page"].as_u64().unwrap(), params["size"].as_u64().unwrap());
    let window = json!({"sort": newest(), "limit": size, "offset": (page - 1) * size});
    let mut filters: Vec<Value> = vec![];
    let with = |query: Value, filters: &[Value]| {
        let mut query = query;
        query["filter"] = json!(filters);
        query["sort"] = window["sort"].clone();
        query["limit"] = window["limit"].clone();
        query["offset"] = window["offset"].clone();
        query
    };
    match params["kind"].as_str().unwrap() {
        "users" => {
            if let Some(keyword) = params["keyword"].as_str() {
                filters.push(either("name", "email", keyword));
            }
            if !params["is_deleted"].is_null() {
                filters.push(eq("is_deleted", &params["is_deleted"]));
            }
            let total = server.total(token, "User.totalCount", &filters).await;
            let select = [
                "id",
                "email",
                "name",
                "role",
                "profile_image_id",
                "is_tutorial_done",
                "is_verify_email_done",
                "is_deleted",
                "created_at",
                "watchCount",
                "commentCount",
            ];
            let reply = server.read(token, with(json!({"read": "User", "select": select}), &filters)).await;
            let rows = reply["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    json!({"user_id": row["id"], "email": row["email"], "name": row["name"], "role": row["role"], "profile_image_id": row["profile_image_id"],
                           "is_tutorial_done": truthy(&row["is_tutorial_done"]), "is_verify_email_done": truthy(&row["is_verify_email_done"]),
                           "is_deleted": truthy(&row["is_deleted"]), "created_at": stamp(&row["created_at"]),
                           "total_watch_count": row["watchCount"], "total_comment_count": row["commentCount"]})
                })
                .collect();
            envelope("users", rows, &total, page, size)
        }
        "requests" => {
            if let Some(status) = params["status"].as_str() {
                filters.push(eq("status", &json!(status)));
            }
            let total = server.total(token, "VideoRequest.totalCount", &filters).await;
            let select = json!(["id", {"user": {"select": ["id", "name"]}}, "youtube_url", "youtube_full_url", "status", "admin_comment", "created_at", "updated_at"]);
            let reply = server.read(token, with(json!({"read": "VideoRequest", "select": select}), &filters)).await;
            let rows = reply["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    json!({"video_request_id": row["id"], "user_id": row["user"]["id"], "user_name": row["user"]["name"], "youtube_url": row["youtube_url"],
                           "youtube_full_url": row["youtube_full_url"], "status": row["status"], "admin_comment": row["admin_comment"],
                           "created_at": stamp(&row["created_at"]), "updated_at": stamp(&row["updated_at"])})
                })
                .collect();
            envelope("requests", rows, &total, page, size)
        }
        "videos" => {
            if let Some(keyword) = params["keyword"].as_str() {
                filters.push(either("title", "channel_name", keyword));
            }
            if let Some(category) = params["category"].as_str() {
                filters.push(eq("category", &json!(category)));
            }
            let total = server.total(token, "Video.totalCount", &filters).await;
            let select =
                ["id", "youtube_url", "title", "channel_name", "category", "duration", "view_count", "likeCount", "commentCount", "created_at"];
            let reply = server.read(token, with(json!({"read": "Video", "select": select}), &filters)).await;
            let rows = reply["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    json!({"video_id": row["id"], "youtube_url": row["youtube_url"], "title": row["title"], "channel_name": row["channel_name"].as_str().unwrap_or(""),
                           "category": row["category"], "duration": row["duration"], "view_count": row["view_count"], "like_count": row["likeCount"],
                           "comment_count": row["commentCount"], "created_at": stamp(&row["created_at"]), "is_deleted": false})
                })
                .collect();
            envelope("videos", rows, &total, page, size)
        }
        "comments" => {
            if let Some(video) = params["video"].as_str() {
                filters.push(eq("video", &json!(video)));
            }
            if let Some(keyword) = params["keyword"].as_str() {
                filters.push(json!({"field": "content", "op": "like", "value": keyword}));
            }
            if !params["is_deleted"].is_null() {
                filters.push(eq("is_deleted", &params["is_deleted"]));
            }
            let total = server.total(token, "Comment.totalCount", &filters).await;
            let select = json!(["id", {"video": {"select": ["id", "title"]}}, {"user": {"select": ["id", "name"]}}, "content", "is_modified", "is_deleted", "created_at"]);
            let reply = server.read(token, with(json!({"read": "Comment", "select": select}), &filters)).await;
            // A soft-deleted video is hidden from everyone by the Video row policy, so its traversal is null; the id stays readable.
            let refs = server.read(token, with(json!({"read": "Comment", "select": ["id", "video"]}), &filters)).await;
            let rows = reply["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    let video = refs["rows"].as_array().unwrap().iter().find(|candidate| candidate["id"] == row["id"]).unwrap()["video"].clone();
                    json!({"comment_id": row["id"], "video_id": video, "video_title": row["video"].get("title").cloned().unwrap_or(Value::Null), "user_id": row["user"]["id"],
                           "user_name": row["user"]["name"], "content": row["content"], "is_modified": truthy(&row["is_modified"]),
                           "is_deleted": truthy(&row["is_deleted"]), "created_at": stamp(&row["created_at"])})
                })
                .collect();
            envelope("comments", rows, &total, page, size)
        }
        other => panic!("unknown list {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn facereview_administrator_lists_match_the_original_admin_service_through_the_real_server() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("facereview.aip");
    std::fs::write(&file, DEFINITION).unwrap();

    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}` DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci")).await.unwrap();
    setup.query_drop(format!("USE `{SOURCE}`")).await.unwrap();
    setup.query_drop("SET FOREIGN_KEY_CHECKS=0").await.unwrap();
    for script in [SCHEMA, SEED, SEED_13D, SEED_13E] {
        run_script(&mut setup, script).await;
    }
    setup.query_drop("SET FOREIGN_KEY_CHECKS=1").await.unwrap();
    run_script(&mut setup, DEFAULTS).await;
    setup.query_drop("DROP USER IF EXISTS 'aip_t_adm_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_adm_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_adm_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env { adopt_url: admin_url.clone(), runtime_url: format!("mysql://aip_t_adm_rt:runtime_test_only@{host}/{SOURCE}") };

    let mut mapping: Value = serde_json::from_str(MAPPING).unwrap();
    mapping["schema"] = json!(SOURCE);
    std::fs::write(directory.path().join("mapping.json"), mapping.to_string()).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(
        directory.path().join("jwks.json"),
        json!({"keys": [{"kty": "RSA", "kid": "current", "alg": "RS256", "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()), "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]})
        .to_string(),
    )
    .unwrap();
    let config = directory.path().join("service.json");
    std::fs::write(
        &config,
        json!({
            "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
            "database_url_env": "AIP_T_ADM_RUNTIME_URL", "migrate_database_url_env": "AIP_T_ADM_ADOPT_URL",
            "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
            "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
        })
        .to_string(),
    )
    .unwrap();
    let file_arg = file.to_str().unwrap();
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    for (subject, actor) in SUBJECTS {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &config).await.expect("bind");
    }

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
    let server = Server { address: &address, fingerprint: &fingerprint };
    let tokens = |name: &str| token(&private, name);

    let golden: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    assert!(golden.len() >= 50, "the golden file lost cases");
    for case in &golden {
        let name = case["name"].as_str().unwrap();
        let subject = tokens(case["subject"].as_str().unwrap());
        assert_eq!(compose(&server, &subject, case).await, case["expected"], "{name}");
    }

    // A member is not an administrator: no total (the original answers 403) and no other member's row.
    let alice = tokens("alice");
    for aggregate in ["User.totalCount", "VideoRequest.totalCount", "Comment.totalCount"] {
        let reply = post(&address, "/read", Some(&alice), &fingerprint, &json!({"query": {"aggregate": aggregate, "input": {}, "filter": []}})).await;
        assert_eq!(reply["code"], "WIRE.ACCESS_DENIED", "{aggregate}: {reply}");
    }
    let others = post(
        &address,
        "/read",
        Some(&alice),
        &fingerprint,
        &json!({"query": {"read": "User", "select": ["id"], "filter": [eq("role", &json!("ADMIN"))]}}),
    )
    .await;
    assert_eq!(others["rows"], json!([]), "a member reads only the own member row: {others}");
}
