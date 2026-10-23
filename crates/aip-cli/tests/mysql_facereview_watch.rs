#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.5: the FaceReview watch detail (`GET watch/video`: `Video.watchDetail` and its `Video.view` increment) through the real
//! `aip service` binary, the MySQL 8.4 schema of the original (pinned new-facereview@7948b7e7), a real MongoDB behind `workers.mongo`
//! (the server-side all-viewer timeline aggregate) and the Python worker sandbox.
//!
//! The expected answers are `slice13f-golden.json`: what the original's own `WatchService.get_video_detail` returned for the same
//! data (`reference/slice13f_oracle.py` runs the original and the real server side by side and writes the file only after every case
//! matched). This test replays those requests on a fresh deployment, so it needs neither the original checkout nor Flask.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up` with `AIP_TEST_MONGO_URL` pointing at a run-owned MongoDB.
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sha2::Digest as _;
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
const MAPPING: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/mapping.json");
const SCHEMA: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/schema.sql");
const DEFAULTS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/db-defaults.sql");
const SEED: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/seed.sql");
const SEED_13D: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-seed.sql");
const SEED_13F: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13f-seed.sql");
const TIMELINE_SESSIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13f-youtube_watching_data.json");
const DISTRIBUTIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-video_distribution.json");
const SESSIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-youtube_watching_data.json");
const GOLDEN: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13f-golden.json");
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
const META: &str = "aip_t_wch_meta";
const SOURCE: &str = "aip_t_wch_src";
const MONGO_DB: &str = "aip_t_wch_fr";
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
    redis_url: String,
}

fn cli(env: &Env, args: &[&str], config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aip"));
    command.arg("service");
    if args.first() == Some(&"principal") {
        command.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        command.args(args).arg("--config").arg(config);
    }
    command
        .env("AIP_T_WCH_ADOPT_URL", &env.adopt_url)
        .env("AIP_T_WCH_RUNTIME_URL", &env.runtime_url)
        .env("AIP_T_WCH_MONGO_URL", &env.mongo_url)
        .env("AIP_T_WCH_REDIS_URL", &env.redis_url);
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

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn socket_connect(address: &str, token: &str) -> Socket {
    let mut request = format!("ws://{address}/socket.io/?EIO=4&transport=websocket").into_client_request().unwrap();
    request.headers_mut().insert("origin", "http://localhost".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.expect("Socket.IO WebSocket handshake");
    let open = match socket.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Text(text) => text.to_string(),
        other => panic!("Engine.IO open packet expected, got {other:?}"),
    };
    assert!(open.starts_with('0'), "{open}");
    socket.send(tokio_tungstenite::tungstenite::Message::Text(format!("40{}", json!({"token":token})).into())).await.unwrap();
    let connected = match socket.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Text(text) => text.to_string(),
        other => panic!("Socket.IO CONNECT acknowledgement expected, got {other:?}"),
    };
    assert!(connected.starts_with("40{"), "{connected}");
    socket
}

async fn socket_watch(socket: &mut Socket, ack: u64, frame: &Value) -> Value {
    let event = format!("42{ack}{}", json!(["watch_frame", frame]));
    socket.send(tokio_tungstenite::tungstenite::Message::Text(event.into())).await.unwrap();
    let reply = match socket.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Text(text) => text.to_string(),
        other => panic!("Socket.IO ACK expected, got {other:?}"),
    };
    let payload = reply.strip_prefix(&format!("43{ack}")).expect("ACK id matches");
    serde_json::from_str::<Value>(payload).unwrap()[0].clone()
}

fn watch_frame(view_log_id: &str) -> Value {
    json!({"video_view_log_id":view_log_id,"video_id":"eeeeeeee-0000-4000-8000-000000000001",
        "youtube_running_time":0.5,"frame_data":"data:image/jpeg;base64,AAAA","duration":120,"user_id":"client-claims-bob"})
}

fn token(private: &RsaPrivateKey, subject: &str) -> String {
    token_with_ttl(private, subject, 300)
}

fn token_with_ttl(private: &RsaPrivateKey, subject: &str, ttl: u64) -> String {
    let key = private.to_pkcs1_der().unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let issued = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    encode(
        &header,
        &json!({"iss": ISSUER, "aud": "aip-api", "sub": subject, "iat": issued, "nbf": issued, "exp": issued + ttl}),
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

async fn view_log_count(conn: &mut aip_migrate::mysql_driver::mysql_async::Conn, id: &str) -> i64 {
    conn.query_first::<i64, _>(format!("SELECT COUNT(*) FROM `{SOURCE}`.`video_view_log` WHERE `video_view_log_id` = '{id}'")).await.unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs dedicated MySQL 8.4, MongoDB, Redis (AIP_TEST_REDIS_URL) and a worker sandbox; run with -- --ignored"]
async fn facereview_watch_detail_matches_the_original_service_through_the_real_server() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let mongo_url = std::env::var("AIP_TEST_MONGO_URL").expect("set AIP_TEST_MONGO_URL to a run-owned MongoDB");
    let parsed = url::Url::parse(&mongo_url).expect("Mongo URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "the MongoDB fixture must bind loopback only");
    assert!(parsed.port().is_some_and(|port| port > 1024 && port != 27017), "the MongoDB fixture must use a dedicated high port");
    let redis_url = std::env::var("AIP_TEST_REDIS_URL").expect("set AIP_TEST_REDIS_URL to a run-owned Redis");
    let parsed = url::Url::parse(&redis_url).expect("Redis URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "the Redis fixture must bind loopback only");
    assert!(parsed.port().is_some_and(|port| port > 1024 && port != 6379), "the Redis fixture must use a dedicated high port");

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
    run_script(&mut setup, SEED_13F).await;
    setup.query_drop("SET FOREIGN_KEY_CHECKS=1").await.unwrap();
    run_script(&mut setup, DEFAULTS).await;
    setup.query_drop("DROP USER IF EXISTS 'aip_t_wch_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_wch_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_wch_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_wch_rt:runtime_test_only@{host}/{SOURCE}"),
        mongo_url: mongo_url.clone(),
        redis_url: redis_url.clone(),
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
    for (collection, documents) in
        [("video_distribution", DISTRIBUTIONS), ("youtube_watching_data", SESSIONS), ("youtube_watching_data", TIMELINE_SESSIONS)]
    {
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
    // The original app creates the unique key of the session documents (`video_view_log_id`) itself. The seeded documents of the
    // original's golden cases carry no such field, so the key is partial: only documents that have one are held to it.
    mongo
        .database(MONGO_DB)
        .collection::<mongodb::bson::Document>("youtube_watching_data")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(mongodb::bson::doc! {"video_view_log_id": 1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(mongodb::bson::doc! {"video_view_log_id": {"$type": "string"}})
                        .build(),
                )
                .build(),
        )
        .await
        .expect("session key index");
    let config = directory.path().join("service.json");
    let write_config = |mongo: Value| {
        let mut workers = json!({"directory": "extensions", "language": "python", "enable_writes": true, "integrations": [{"name": "youtube_videos", "url": "http://127.0.0.1:9/youtube/v3/videos", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}, {"name": "youtube_search", "url": "http://127.0.0.1:9/youtube/v3/search", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}], "redis": {"uri_env": "AIP_T_WCH_REDIS_URL", "prefixes": ["facereview:watch:active:", "facereview:watch:frame:"], "max_in_flight": 4, "timeout_ms": 3000}});
        if !mongo.is_null() {
            workers["mongo"] = mongo;
        }
        std::fs::write(
            &config,
            json!({
                "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
                "database_url_env": "AIP_T_WCH_RUNTIME_URL", "migrate_database_url_env": "AIP_T_WCH_ADOPT_URL",
                "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null, "allowed_origins": ["http://localhost"],
                "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
                "socketio": {},
                "workers": workers,
            })
            .to_string(),
        )
        .unwrap();
    };
    write_config(json!({"uri_env": "AIP_T_WCH_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000,
        "write_collections": ["youtube_watching_data"]}));
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

    // Every golden case: the original answered, so must the server. The timeline is stored once per video in the golden file.
    let golden: Value = serde_json::from_str(GOLDEN).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 36, "the golden file lost cases");
    for case in cases {
        let (name, subject) = (case["name"].as_str().unwrap(), case["subject"].as_str());
        let reply = ask("Video.watchDetail", subject, &json!({"video_id": case["video"]})).await;
        if case["expect_error"] == true {
            assert_eq!(reply["ok"], false, "{name}: the original refuses, the server must not answer: {reply}");
            continue;
        }
        assert_eq!(reply["ok"], true, "{name}: {reply}");
        let mut output = reply["output"].clone();
        let timeline: Value = serde_json::from_str(output["timeline_data"].as_str().unwrap()).unwrap();
        output.as_object_mut().unwrap().remove("timeline_data");
        assert_eq!(output, case["expected"], "{name}");
        assert_eq!(timeline, golden["timelines"][case["video"].as_str().unwrap()], "{name}: timeline");
    }

    // The increment is its own transition: the detail reads above did not count a view (the seeded video still shows 10).
    let video = "eeeeeeee-0000-4000-8000-000000000001";
    let view = |subject: &'static str, target: &'static str, key: &'static str| {
        let bearer = tokens.iter().find(|(candidate, _)| *candidate == subject).unwrap().1.clone();
        let (address, fingerprint) = (address.clone(), fingerprint.clone());
        let body = json!({"key": key, "request": {"apply": "Video.view", "target": {"ids": [target]}}});
        async move { post(&address, "/apply", Some(&bearer), &fingerprint, &body).await }
    };
    let count = |reply: &Value| reply["output"]["view_count"].as_i64().unwrap();
    assert_eq!(count(&ask("Video.watchDetail", Some("alice"), &json!({"video_id": video})).await), 10);
    assert_eq!(view("alice", video, "wch-1").await["ok"], true);
    assert_eq!(view("bob", video, "wch-2").await["ok"], true);
    assert_eq!(count(&ask("Video.watchDetail", Some("alice"), &json!({"video_id": video})).await), 12, "two views counted");
    for (key, target) in [("wch-3", "bbbbbbbb-0000-4000-8000-000000000006"), ("wch-4", "cccccccc-0000-4000-8000-0000000000ff")] {
        let refused = view("alice", target, key).await;
        assert_eq!(refused["ok"], false, "{target} cannot be viewed (soft-deleted or unknown): {refused}");
    }

    // A guest has no session: the extension route needs a signed caller (the original's login_optional guests are not served).
    let guest = ask("Video.watchDetail", None, &json!({"video_id": video})).await;
    assert_eq!(guest["code"], "WIRE.UNAUTHENTICATED", "{guest}");

    // WBS 7.2.6 slice 2: the first authenticated frame enters the declared MySQL write extension and Redis session capability.
    // The browser's claimed user_id is deliberately wrong; the row and cache must use Alice from the verified token.
    let view_log_id = "66666666-6666-4666-8666-666666666666";
    let digest_hex: String = sha2::Sha256::digest(view_log_id.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    let redis_key = format!("facereview:watch:active:{digest_hex}");
    let redis = redis::Client::open(redis_url.as_str()).expect("Redis fixture");
    let mut redis_connection = redis.get_multiplexed_async_connection().await.expect("Redis connection");
    let _: i64 = redis::cmd("DEL").arg(&redis_key).query_async(&mut redis_connection).await.unwrap();

    let alice_token = tokens.iter().find(|(candidate, _)| *candidate == "alice").unwrap().1.clone();
    let mut alice_socket = socket_connect(&address, &alice_token).await;
    let first = socket_watch(&mut alice_socket, 1, &watch_frame(view_log_id)).await;
    assert_eq!(first["code"], "WIRE.NOT_AVAILABLE", "session is written, but ML is not connected: {first}");
    assert!(first.get("response").is_none(), "no fixed/mock emotion is returned: {first}");
    let duplicate = socket_watch(&mut alice_socket, 2, &watch_frame(view_log_id)).await;
    assert_eq!(duplicate["code"], "WIRE.NOT_AVAILABLE", "duplicate first frame is idempotent: {duplicate}");

    assert_eq!(view_log_count(&mut setup, view_log_id).await, 1, "duplicate frame created one view log");
    let owner: Option<String> =
        setup.query_first(format!("SELECT `user_id` FROM `{SOURCE}`.`video_view_log` WHERE `video_view_log_id` = '{view_log_id}'")).await.unwrap();
    assert_eq!(owner.as_deref(), Some(SUBJECTS[0].1), "server principal wins over payload user_id");
    let cached: Option<String> = redis::cmd("GET").arg(&redis_key).query_async(&mut redis_connection).await.unwrap();
    let cached: Value = serde_json::from_str(cached.as_deref().expect("Redis session cache")).unwrap();
    assert_eq!(cached["user_id"], SUBJECTS[0].1);
    assert_eq!(cached["video_id"], video);
    assert_eq!(cached["duration"], 120);

    // Reconnect has a new Socket.IO sid/idempotency key, reads the same owner-scoped row and rebuilds the Redis session.
    alice_socket.send(tokio_tungstenite::tungstenite::Message::Text("41".into())).await.unwrap();
    let _ = alice_socket.next().await;
    let mut reconnect = socket_connect(&address, &alice_token).await;
    assert_eq!(socket_watch(&mut reconnect, 3, &watch_frame(view_log_id)).await["code"], "WIRE.NOT_AVAILABLE");
    assert_eq!(view_log_count(&mut setup, view_log_id).await, 1, "reconnect reuses the existing view log");

    // A different authenticated actor cannot claim Alice's session id or learn her stored row.
    let bob_token = tokens.iter().find(|(candidate, _)| *candidate == "bob").unwrap().1.clone();
    let mut bob_socket = socket_connect(&address, &bob_token).await;
    let separated = socket_watch(&mut bob_socket, 4, &watch_frame(view_log_id)).await;
    assert_eq!(separated["code"], "WIRE.ALREADY_EXISTS", "another owner is refused opaquely: {separated}");
    assert_eq!(view_log_count(&mut setup, view_log_id).await, 1);

    // An operator's revoke reaches an open socket on its next frame: the token is verified again before any write, the frame is
    // refused and the connection ends, and no view log or Redis session is made.
    let carol_token = tokens.iter().find(|(candidate, _)| *candidate == "carol").unwrap().1.clone();
    let revoked_log = "77777777-7777-4777-8777-777777777777";
    let mut carol_socket = socket_connect(&address, &carol_token).await;
    run(&env, &["principal", "revoke", "--subject", "carol"], &config).await.expect("revoke carol");
    let refused = socket_watch(&mut carol_socket, 5, &watch_frame(revoked_log)).await;
    assert_eq!(refused["status"], "error", "{refused}");
    assert!(
        ["WIRE.UNAUTHENTICATED", "WIRE.TOKEN_EXPIRED"].contains(&refused["code"].as_str().unwrap_or("")),
        "a revoked principal's frame is refused: {refused}"
    );
    assert_eq!(view_log_count(&mut setup, revoked_log).await, 0, "a revoked principal wrote nothing");
    let revoked_close = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match carol_socket.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame,
                Some(Ok(_)) => continue,
                other => panic!("expected close after the refused frame, got {other:?}"),
            }
        }
    })
    .await
    .expect("the refused frame ends the connection");
    assert!(revoked_close.is_some());

    // The Socket.IO caller initializes one owner-scoped Mongo timeline; without a connected model both emotion maps remain empty.
    let sessions = mongo.database(MONGO_DB).collection::<mongodb::bson::Document>("youtube_watching_data");
    let empty_timeline =
        sessions.find_one(mongodb::bson::doc! {"video_view_log_id": view_log_id}).await.unwrap().expect("first frame initializes the Mongo timeline");
    assert_eq!(empty_timeline.get_document("emotion_score_timeline").unwrap().len(), 0);
    assert_eq!(empty_timeline.get_document("most_emotion_timeline").unwrap().len(), 0);
    assert_eq!(empty_timeline.get_str("video_id").unwrap(), video);
    assert_eq!(empty_timeline.get_str("user_id").unwrap(), SUBJECTS[0].1);
    assert_eq!(sessions.count_documents(mongodb::bson::doc! {"video_view_log_id": view_log_id}).await.unwrap(), 1);
    let stage = |centi: u64| format!("facereview:watch:frame:{digest_hex}:{centi}");
    let record = |subject: &'static str, key: String, input: Value| {
        let bearer = tokens.iter().find(|(candidate, _)| *candidate == subject).unwrap().1.clone();
        let (address, fingerprint) = (address.clone(), fingerprint.clone());
        async move {
            post(&address, "/apply", Some(&bearer), &fingerprint, &json!({"key": key, "request": {"extension": "User.recordFrame", "input": input}}))
                .await
        }
    };
    let stage_connection = redis_connection.clone();
    let stage_scores = |centi: u64, scores: Value| {
        let (mut connection, key) = (stage_connection.clone(), stage(centi));
        async move {
            let _: () = redis::cmd("SET").arg(key).arg(scores.to_string()).arg("EX").arg(60).query_async(&mut connection).await.unwrap();
        }
    };
    stage_scores(150, json!({"neutral": 10.5, "happy": 60.25, "surprise": 5.0, "sad": 4.25, "angry": 20.0})).await;
    let blocked = record("alice", "frame-public-probe".into(), json!({"id": view_log_id, "centi": 150})).await;
    assert_eq!(blocked["code"], "WIRE.NOT_EXPOSED", "public HTTP cannot invoke recordFrame: {blocked}");
    let timeline_after_refused_public_call = sessions.find_one(mongodb::bson::doc! {"video_view_log_id": view_log_id}).await.unwrap().unwrap();
    assert_eq!(timeline_after_refused_public_call.get_document("emotion_score_timeline").unwrap().len(), 0);
    let staged: Option<String> = redis::cmd("GET").arg(stage(150)).query_async(&mut redis_connection).await.unwrap();
    assert!(staged.is_some(), "a refused public request does not consume server-staged scores");

    // The same token is checked again on each frame, and the connection also closes at its expiry deadline.
    let short_token = token_with_ttl(&private, "alice", 2);
    let mut expiring = socket_connect(&address, &short_token).await;
    tokio::time::sleep(Duration::from_millis(2400)).await;
    let close = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match expiring.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame,
                Some(Ok(_)) => continue,
                other => panic!("expected token expiry close, got {other:?}"),
            }
        }
    })
    .await
    .expect("expiry closes the session");
    assert_eq!(close.unwrap().reason.as_str(), "WIRE.TOKEN_EXPIRED");
    assert_eq!(view_log_count(&mut setup, view_log_id).await, 1, "expired token did not write another view log");
    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
}
