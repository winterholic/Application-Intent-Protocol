#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.3: the FaceReview recommendation APIs (`home/category`, `home/personalized`, `watch/recommended`) through the real
//! `aip service` binary, the MySQL 8.4 schema of the original (pinned new-facereview@7948b7e7), a real MongoDB behind `workers.mongo`
//! and the Python worker sandbox.
//!
//! The expected answers are `slice13d-golden.json`: what the original's own `HomeService` and `WatchService` returned for the same
//! data (`reference/slice13d_oracle.py` runs the original and the real server side by side and writes the file only after every case
//! matched). This test replays those requests on a fresh deployment, so it needs neither the original checkout nor Flask.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up` with `AIP_TEST_MONGO_URL` pointing at a run-owned MongoDB.
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
const DISTRIBUTIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-video_distribution.json");
const SESSIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-youtube_watching_data.json");
const GOLDEN: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-golden.json");
const EXTENSIONS: [(&str, &str); 14] = [
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
];
const META: &str = "aip_t_rec_meta";
const SOURCE: &str = "aip_t_rec_src";
const MONGO_DB: &str = "aip_t_rec_fr";
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
    command.env("AIP_T_REC_ADOPT_URL", &env.adopt_url).env("AIP_T_REC_RUNTIME_URL", &env.runtime_url).env("AIP_T_REC_MONGO_URL", &env.mongo_url);
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
    tokio::time::timeout(Duration::from_secs(120), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
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

/// The answer shape of each extension, parsed out of its JSON text fields.
fn answer(extension: &str, output: &Value) -> Value {
    let parse = |field: &str| serde_json::from_str::<Value>(output[field].as_str().unwrap_or_else(|| panic!("{field} is text: {output}"))).unwrap();
    match extension {
        "Video.homeCategory" => parse("categories"),
        "Video.homePersonalized" => parse("videos"),
        "Video.watchRecommended" => {
            json!({"videos": parse("videos"), "total": output["total"], "page": output["page"], "size": output["size"], "has_next": output["has_next"]})
        }
        other => panic!("unexpected extension {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 (scripts/mysql-live-fixture.sh), MongoDB (AIP_TEST_MONGO_URL) and a worker sandbox; run with -- --ignored"]
async fn facereview_recommendations_match_the_original_services_through_the_real_server() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let mongo_url = std::env::var("AIP_TEST_MONGO_URL").expect("set AIP_TEST_MONGO_URL to a run-owned MongoDB");
    let parsed = url::Url::parse(&mongo_url).expect("Mongo URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "the MongoDB fixture must bind loopback only");
    assert!(parsed.port().is_some_and(|port| port > 1024 && port != 27017), "the MongoDB fixture must use a dedicated high port");

    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("facereview.aip");
    std::fs::write(&file, DEFINITION).unwrap();
    let extensions = directory.path().join("extensions");
    std::fs::create_dir(&extensions).unwrap();
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
    run_script(&mut setup, SEED_13D).await;
    setup.query_drop("SET FOREIGN_KEY_CHECKS=1").await.unwrap();
    run_script(&mut setup, DEFAULTS).await;
    setup.query_drop("DROP USER IF EXISTS 'aip_t_rec_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_rec_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_rec_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_rec_rt:runtime_test_only@{host}/{SOURCE}"),
        mongo_url: mongo_url.clone(),
    };

    // MongoDB: the run-owned database is rebuilt from the same documents the original saw. Insertion order is the natural order.
    let mongo = mongodb::Client::with_uri_str(&mongo_url).await.expect("MongoDB fixture");
    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
    let database = mongo.database(MONGO_DB);
    database
        .collection::<mongodb::bson::Document>("video_distribution")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(mongodb::bson::doc! {"video_id": 1})
                .options(mongodb::options::IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await
        .expect("unique video_id index");
    for (collection, documents) in [("video_distribution", DISTRIBUTIONS), ("youtube_watching_data", SESSIONS)] {
        for document in serde_json::from_str::<Vec<Value>>(documents).unwrap() {
            let document = mongodb::bson::serialize_to_document(&document).unwrap();
            database.collection::<mongodb::bson::Document>(collection).insert_one(document).await.expect("seed MongoDB");
        }
    }

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
    let write_config = |mongo: Value| {
        let mut workers = json!({"directory": "extensions", "language": "python", "integrations": [{"name": "youtube_videos", "url": "http://127.0.0.1:9/youtube/v3/videos", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}, {"name": "youtube_search", "url": "http://127.0.0.1:9/youtube/v3/search", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}]});
        if !mongo.is_null() {
            workers["mongo"] = mongo;
        }
        std::fs::write(
            &config,
            json!({
                "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
                "database_url_env": "AIP_T_REC_RUNTIME_URL", "migrate_database_url_env": "AIP_T_REC_ADOPT_URL",
                "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
                "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
                "workers": workers,
            })
            .to_string(),
        )
        .unwrap();
    };
    write_config(json!({"uri_env": "AIP_T_REC_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000}));
    let file_arg = file.to_str().unwrap();
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    for (subject, actor) in SUBJECTS {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &config).await.expect("bind");
    }

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts with a MongoDB read adapter");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(listening["readAdapters"]["mongo"]["status"], "ready", "{listening}");
    let tokens: Vec<(&str, String)> = SUBJECTS.iter().map(|(subject, _)| (*subject, token(&private, subject))).collect();
    let ask = |extension: &str, subject: Option<&str>, input: &Value| {
        let bearer = subject.map(|name| tokens.iter().find(|(candidate, _)| *candidate == name).unwrap().1.clone());
        let (address, fingerprint, body) = (address.clone(), fingerprint.clone(), json!({"extension": extension, "input": input}));
        async move { post(&address, "/extension", bearer.as_deref(), &fingerprint, &body).await }
    };

    // Every golden case: the original answered, so must the server; where the original refuses (unknown, deleted or NULL-flag video)
    // the server answers an error and no list.
    let golden: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    assert!(golden.len() >= 40, "the golden file lost cases");
    for case in &golden {
        let (name, extension, subject) = (case["name"].as_str().unwrap(), case["extension"].as_str().unwrap(), case["subject"].as_str());
        let reply = ask(extension, subject, &case["input"]).await;
        if case["expect_error"] == true {
            assert_eq!(reply["ok"], false, "{name}: the original refuses, the server must not answer: {reply}");
            continue;
        }
        assert_eq!(reply["ok"], true, "{name}: {reply}");
        assert_eq!(answer(extension, &reply["output"]), case["expected"], "{name}");
    }

    // A guest has no session: the extension route needs a signed caller (the original's login_optional guests are not served).
    let guest = ask("Video.homeCategory", None, &json!({})).await;
    assert_eq!(guest["code"], "WIRE.UNAUTHENTICATED", "{guest}");
    // The private watch sessions of other members never reach a caller: Bob's single angry session (a golden input) does not move
    // Alice's profile, and a caller-supplied user filter does not exist on the extension surface.
    let unknown_input = ask(
        "Video.watchRecommended",
        Some("alice"),
        &json!({"video_id": "bbbbbbbb-0000-4000-8000-000000000002", "page": 1, "size": 10, "user": "x"}),
    )
    .await;
    assert_eq!(unknown_input["ok"], false, "{unknown_input}");
    let beyond =
        ask("Video.watchRecommended", Some("alice"), &json!({"video_id": "bbbbbbbb-0000-4000-8000-000000000002", "page": 0, "size": 10})).await;
    assert_eq!(beyond["ok"], false, "page 0 is refused like the original's schema: {beyond}");
    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
}
