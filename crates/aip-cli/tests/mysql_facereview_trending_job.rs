#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.7: the original's Celery beat task `fetch_youtube_trending_videos` (YoutubeTrendingJob) as an AIP durable job, through the
//! real `aip service` binary, the MySQL 8.4 schema of the original, a real MongoDB behind `workers.mongo` (with write collections), the
//! Python worker sandbox and a stand-in YouTube Data API behind the query-string integration `youtube_videos`.
//!
//! What it proves (fixture: `product/tests/fixtures/facereview-mysql`, definition `fetchTrendingVideos`):
//!  * only an administrator starts it; the provider sees three pages with the server's own API key and only the allowed parameters;
//!  * new videos become Video rows (original category rules, durations, NULL for an empty channel name, views 0, active) and get the
//!    two initial MongoDB documents; an existing video, a soft-deleted one (unique youtube_url), a repeat and an untitled one are not saved;
//!  * the same key is one run and asks the provider nothing more; a second run saves nothing twice;
//!  * a 403 from the provider stops the paging and the pages before it are processed; a provider that does not answer asks for the run
//!    to be tried again.
//!
//! The category and duration rules are compared with the original class in `reference/slice14b_oracle.py`.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up` with `AIP_TEST_MONGO_URL` pointing at a run-owned MongoDB.
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::{Child, Command},
};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
const MAPPING: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/mapping.json");
const SCHEMA: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/schema.sql");
const DEFAULTS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/db-defaults.sql");
const SEED: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/seed.sql");
const SEED_13D: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice13d-seed.sql");
const MYPAGE_SESSIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice14e-sessions.json");
const MYPAGE_GOLDEN: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice14e-golden.json");
const SUMMARY_SESSIONS: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice14f-sessions.json");
const SUMMARY_GOLDEN: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/slice14f-golden.json");
const EXTENSIONS: [(&str, &str); 11] = [
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
];
const META: &str = "aip_t_yt_meta";
const SOURCE: &str = "aip_t_yt_src";
const MONGO_DB: &str = "aip_t_yt_fr";
const API_KEY: &str = "api-key-only-the-server-knows";
const ISSUER: &str = "https://issuer.example";
const SUBJECTS: [(&str, &str); 5] = [
    ("alice", "11111111-1111-4111-8111-111111111111"),
    ("bob", "22222222-2222-4222-8222-222222222222"),
    ("carol", "33333333-3333-4333-8333-333333333333"),
    ("dave", "44444444-4444-4444-8444-444444444444"),
    ("erin", "55555555-5555-4555-8555-555555555555"),
];

/// The five totals, the checkpoint, the session id, lock_version, "checkpoint is recent" and "updated_at was touched" of a summary row.
type SummaryRow = (String, String, String, String, String, Option<String>, Option<String>, i64, i64, i64);

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
        .env("AIP_T_YT_ADOPT_URL", &env.adopt_url)
        .env("AIP_T_YT_RUNTIME_URL", &env.runtime_url)
        .env("AIP_T_YT_MONGO_URL", &env.mongo_url)
        .env("AIP_T_YT_REDIS_URL", &env.redis_url)
        .env("AIP_T_YT_API_KEY", API_KEY);
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

struct Server {
    _child: Child,
    address: String,
    fingerprint: String,
}

impl Server {
    async fn post(&self, path: &str, token: Option<&str>, body: &Value) -> Value {
        post(&self.address, path, token, &self.fingerprint, body).await
    }

    /// The final state of a durable run, polled until it ends or `states` is reached.
    async fn run_until(&self, token: &str, id: &str, done: impl Fn(&Value) -> bool) -> Value {
        let mut last = Value::Null;
        for _ in 0..600 {
            last = self.post("/jobs/status", Some(token), &json!({"id": id})).await;
            assert_eq!(last["ok"], true, "{last}");
            if done(&last["run"]) {
                return last["run"].clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the run did not reach the expected state: {last}");
    }
}

async fn start_job_server(env: &Env, file: &Path, config: &Path) -> Server {
    let (child, listening) = start_server(env, file, config).await.expect("serve starts with a MongoDB adapter");
    assert_eq!(listening["readAdapters"]["mongo"]["status"], "ready", "{listening}");
    Server {
        _child: child,
        address: listening["address"].as_str().unwrap().to_string(),
        fingerprint: listening["fingerprint"].as_str().unwrap().to_string(),
    }
}

fn finished(run: &Value) -> bool {
    ["SUCCEEDED", "FAILED", "CANCELLED"].contains(&run["state"].as_str().unwrap())
}

fn video(id: &str, title: &str, channel: &str, category: &str, duration: &str, tags: &[&str], description: &str) -> Value {
    json!({"id": id, "snippet": {"title": title, "channelTitle": channel, "categoryId": category, "tags": tags, "description": description},
        "contentDetails": {"duration": duration}, "statistics": {"viewCount": "12345"}})
}

/// The pages the stand-in YouTube answers, by page token (none, "page1", "page2").
fn pages() -> Vec<Vec<Value>> {
    vec![
        vec![
            video("ytnew00001", "Official MV", "Music Channel", "10", "PT3M25S", &[], ""),
            video("ytnew00002", "Weekend table", "Home Chef", "26", "PT1H2M3S", &["table"], "맛있는 집밥 레시피 모음"),
            // Already in the database (an active video of the fixture).
            video("yt000000001", "Already here", "Old", "10", "PT1M", &[], ""),
        ],
        vec![
            video("ytnew00003", "맛집 먹방 mukbang", "Eater", "22", "PT10M", &["night"], ""),
            video("ytnew00004", "롤 게임 gaming", "Gamer", "22", "PT25M30S", &[], ""),
            // Soft-deleted in the database: the unique youtube_url refuses it again.
            video("yt000000006", "Deleted before", "Old", "10", "PT2M", &[], ""),
        ],
        vec![
            video("ytnew00005", "Match highlights", "Sports", "17", "P0D", &[], ""),
            video("ytnew00006", "Variety clip", "", "24", "PT45S", &[], ""),
            // Listed on two pages and saved once; a video without a title is skipped.
            video("ytnew00001", "Official MV", "Music Channel", "10", "PT3M25S", &[], ""),
            video("ytnotitle1", "", "No title", "10", "PT1M", &[], ""),
        ],
    ]
}

#[derive(Default)]
struct YoutubeMode {
    /// Answer 403 (quota) for this page index and stop there.
    quota_at: Option<usize>,
    /// Hang up on this many next requests without answering.
    hang_up: usize,
}

struct Youtube {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    mode: Arc<Mutex<YoutubeMode>>,
}

impl Youtube {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (requests, mode): (Arc<Mutex<Vec<String>>>, Arc<Mutex<YoutubeMode>>) = Default::default();
        let (log, switch) = (requests.clone(), mode.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let (log, switch) = (log.clone(), switch.clone());
                tokio::spawn(async move {
                    let mut received = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !received.windows(4).any(|window| window == b"\r\n\r\n") {
                        let Ok(read) = socket.read(&mut chunk).await else { return };
                        if read == 0 {
                            return;
                        }
                        received.extend_from_slice(&chunk[..read]);
                    }
                    let text = String::from_utf8_lossy(&received).to_string();
                    let target = text.lines().next().unwrap().split(' ').nth(1).unwrap().to_string();
                    log.lock().unwrap().push(target.clone());
                    {
                        let mut mode = switch.lock().unwrap();
                        if mode.hang_up > 0 {
                            mode.hang_up -= 1;
                            return;
                        }
                    }
                    let parsed = url::Url::parse(&format!("http://peer{target}")).unwrap();
                    let token = parsed.query_pairs().find(|(name, _)| name == "pageToken").map(|(_, value)| value.to_string());
                    let page: usize = token.as_deref().map_or(0, |token| token.trim_start_matches("page").parse().unwrap());
                    let quota = switch.lock().unwrap().quota_at == Some(page);
                    let all = pages();
                    let (status, body) = if quota {
                        ("403 Forbidden", json!({"error": {"code": 403, "message": "quotaExceeded"}}))
                    } else {
                        let mut body = json!({"items": all[page]});
                        if page + 1 < all.len() {
                            body["nextPageToken"] = json!(format!("page{}", page + 1));
                        }
                        ("200 OK", body)
                    };
                    let body = body.to_string();
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        Self { address, requests, mode }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn query_of(target: &str) -> std::collections::BTreeMap<String, String> {
    url::Url::parse(&format!("http://peer{target}")).unwrap().query_pairs().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 (scripts/mysql-live-fixture.sh), MongoDB (AIP_TEST_MONGO_URL) and a worker sandbox; run with -- --ignored"]
async fn the_trending_job_saves_new_videos_with_their_initial_documents() {
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
    setup.query_drop("DROP USER IF EXISTS 'aip_t_yt_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_yt_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_yt_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let youtube = Youtube::start().await;
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_yt_rt:runtime_test_only@{host}/{SOURCE}"),
        mongo_url: mongo_url.clone(),
        redis_url: std::env::var("AIP_TEST_REDIS_URL").unwrap_or_default(),
    };

    let mongo = mongodb::Client::with_uri_str(&mongo_url).await.expect("MongoDB fixture");
    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
    let database = mongo.database(MONGO_DB);
    let distributions = database.collection::<mongodb::bson::Document>("video_distribution");
    let timelines = database.collection::<mongodb::bson::Document>("video_timeline_emotion_count");

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
            "database_url_env": "AIP_T_YT_RUNTIME_URL", "migrate_database_url_env": "AIP_T_YT_ADOPT_URL",
            "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
            "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
            "workers": {"directory": "extensions", "language": "python", "enable_writes": true,
                "mongo": {"uri_env": "AIP_T_YT_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000,
                    "write_collections": ["video_distribution", "video_timeline_emotion_count"], "ensure_indexes": true},
                "integrations": [{"name": "youtube_videos", "url": format!("http://{}/youtube/v3/videos", youtube.address), "method": "GET",
                    "timeout_ms": 3000, "max_request_bytes": 4096, "max_response_bytes": 1048576, "allow_loopback": true,
                    "query_params": ["part", "chart", "regionCode", "maxResults", "pageToken", "id"],
                    "secret_query_env": {"name": "key", "env": "AIP_T_YT_API_KEY"}},
                    // The definition also declares the category fill job, which needs its own endpoint; this test never starts it.
                    {"name": "youtube_search", "url": format!("http://{}/youtube/v3/search", youtube.address), "method": "GET",
                    "timeout_ms": 3000, "max_request_bytes": 4096, "max_response_bytes": 1048576, "allow_loopback": true,
                    "query_params": ["part", "q", "type", "regionCode", "relevanceLanguage", "maxResults", "order"],
                    "secret_query_env": {"name": "key", "env": "AIP_T_YT_API_KEY"}}]},
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
    let server = start_job_server(&env, &file, &config).await;
    let tokens: Vec<(&str, String)> = SUBJECTS.iter().map(|(subject, _)| (*subject, token(&private, subject))).collect();
    let bearer = |name: &str| tokens.iter().find(|(candidate, _)| *candidate == name).unwrap().1.clone();
    let (alice, bob) = (bearer("alice"), bearer("bob"));
    let enqueue = |token: String, key: &'static str| {
        let (address, fingerprint) = (server.address.clone(), server.fingerprint.clone());
        async move {
            post(
                &address,
                "/jobs/enqueue",
                Some(&token),
                &fingerprint,
                &json!({"operation": "fetchTrendingVideos", "input": {}, "key": key, "delayMs": 0}),
            )
            .await
        }
    };
    let rows = |sql: String| {
        let driver = admin.clone();
        async move {
            let mut conn = driver.pool().get_conn().await.unwrap();
            conn.query::<(String, String, String, Option<String>, Option<i64>, Option<i64>, Option<i64>), _>(sql).await.unwrap()
        }
    };
    let saved_sql = format!(
        "SELECT video_id, youtube_url, title, channel_name, duration, view_count, is_deleted FROM `{SOURCE}`.video WHERE youtube_url LIKE 'ytnew%' ORDER BY youtube_url"
    );

    // A member may not start the collection; the administrator may.
    let refused = enqueue(alice.clone(), "trend-alice").await;
    assert_eq!(refused["ok"], false, "{refused}");
    let accepted = enqueue(bob.clone(), "trend-1").await;
    assert_eq!(accepted["ok"], true, "{accepted}");
    let id = accepted["run"]["id"].as_str().unwrap().to_string();
    let run1 = server.run_until(&bob, &id, finished).await;
    assert_eq!(run1["state"], "SUCCEEDED", "{run1}");
    let result = server.post("/jobs/result", Some(&bob), &json!({"id": id})).await;
    // 3 + 3 + 4 listed, one repeated: 9 distinct; the active fixture video is not new; the soft-deleted one is refused by the unique
    // url; the untitled one is skipped; six are saved.
    assert_eq!(result["output"], json!({"fetched": 9, "fresh": 8, "saved": 6}), "{result}");

    // The provider saw three pages with the server's own key and the caller's parameters, and nothing else.
    let seen = youtube.requests();
    assert_eq!(seen.len(), 3, "{seen:?}");
    for (index, target) in seen.iter().enumerate() {
        let query = query_of(target);
        assert_eq!(query["key"], API_KEY, "the server adds its key: {target}");
        assert_eq!((query["chart"].as_str(), query["regionCode"].as_str(), query["maxResults"].as_str()), ("mostPopular", "KR", "50"));
        assert_eq!(query.get("pageToken").map(String::as_str), (index > 0).then(|| ["", "page1", "page2"][index]), "{target}");
    }

    // The rows are what the original's _save_video writes, with the original's category rules.
    let saved = rows(saved_sql.clone()).await;
    let summary: Vec<(&str, &str, Option<&str>, Option<i64>)> =
        saved.iter().map(|(_, url, title, channel, duration, _, _)| (url.as_str(), title.as_str(), channel.as_deref(), *duration)).collect();
    assert_eq!(
        summary,
        [
            ("ytnew00001", "Official MV", Some("Music Channel"), Some(205)),
            ("ytnew00002", "Weekend table", Some("Home Chef"), Some(3723)),
            ("ytnew00003", "맛집 먹방 mukbang", Some("Eater"), Some(600)),
            ("ytnew00004", "롤 게임 gaming", Some("Gamer"), Some(1530)),
            ("ytnew00005", "Match highlights", Some("Sports"), Some(0)),
            ("ytnew00006", "Variety clip", None, Some(45)),
        ]
    );
    assert!(saved.iter().all(|row| row.5 == Some(0) && row.6 == Some(0)), "views start at 0 and the video is active: {saved:?}");
    let categories: Vec<(String, String)> = setup
        .query(format!("SELECT youtube_url, category FROM `{SOURCE}`.video WHERE youtube_url LIKE 'ytnew%' ORDER BY youtube_url"))
        .await
        .unwrap();
    assert_eq!(categories.iter().map(|(_, category)| category.as_str()).collect::<Vec<_>>(), ["music", "cook", "eating", "game", "sports", "show"]);
    // Both initial documents exist for every new video, with the original's empty shape.
    for (video_id, ..) in &saved {
        let distribution = distributions.find_one(mongodb::bson::doc! {"video_id": video_id}).await.unwrap().expect("distribution document");
        let distribution: Value = mongodb::bson::deserialize_from_document(distribution).unwrap();
        assert_eq!(distribution["average_completion_rate"], 0.0);
        assert_eq!(distribution["emotion_averages"], json!({"neutral": 0.0, "happy": 0.0, "surprise": 0.0, "sad": 0.0, "angry": 0.0}));
        assert_eq!(distribution["dominant_emotion"], Value::Null);
        assert!(distribution.get("created_at").is_some() && distribution.get("updated_at").is_some());
        let timeline = timelines.find_one(mongodb::bson::doc! {"video_id": video_id}).await.unwrap().expect("timeline document");
        let timeline: Value = mongodb::bson::deserialize_from_document(timeline).unwrap();
        assert_eq!(timeline["emotion_labels"], json!(["neutral", "happy", "surprise", "sad", "angry"]));
        assert_eq!(timeline["counts"], json!({}));
    }
    assert_eq!(distributions.count_documents(mongodb::bson::doc! {}).await.unwrap(), 6, "documents only for the saved videos");

    // The same key is one run and asks YouTube nothing more; a new key finds nothing new, whatever the page says.
    let again = enqueue(bob.clone(), "trend-1").await;
    assert_eq!(again["run"]["id"], id.as_str());
    assert_eq!(youtube.requests().len(), 3);
    let second = enqueue(bob.clone(), "trend-2").await;
    let run2 = server.run_until(&bob, second["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(run2["state"], "SUCCEEDED", "{run2}");
    let second_result = server.post("/jobs/result", Some(&bob), &json!({"id": second["run"]["id"]})).await;
    assert_eq!(
        second_result["output"],
        json!({"fetched": 9, "fresh": 2, "saved": 0}),
        "only the soft-deleted and the untitled video are still unseen: {second_result}"
    );
    assert_eq!(rows(saved_sql.clone()).await.len(), 6, "nothing was saved twice");
    assert_eq!(youtube.requests().len(), 6);

    // A provider that answers 403 (quota) stops the paging, and what came before it is processed (the original's behaviour).
    setup.query_drop(format!("DELETE FROM `{SOURCE}`.video WHERE youtube_url LIKE 'ytnew%'")).await.unwrap();
    distributions.delete_many(mongodb::bson::doc! {}).await.unwrap();
    timelines.delete_many(mongodb::bson::doc! {}).await.unwrap();
    youtube.mode.lock().unwrap().quota_at = Some(1);
    let quota = enqueue(bob.clone(), "trend-quota").await;
    let run3 = server.run_until(&bob, quota["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(run3["state"], "SUCCEEDED", "{run3}");
    let third = server.post("/jobs/result", Some(&bob), &json!({"id": quota["run"]["id"]})).await;
    assert_eq!(third["output"], json!({"fetched": 3, "fresh": 2, "saved": 2}), "page 0 only: {third}");
    assert_eq!(youtube.requests().len(), 8);
    youtube.mode.lock().unwrap().quota_at = None;

    // A provider that does not answer asks for the run to be tried again; the page that did arrive is not asked for twice.
    setup.query_drop(format!("DELETE FROM `{SOURCE}`.video WHERE youtube_url LIKE 'ytnew%'")).await.unwrap();
    distributions.delete_many(mongodb::bson::doc! {}).await.unwrap();
    timelines.delete_many(mongodb::bson::doc! {}).await.unwrap();
    let before = youtube.requests().len();
    youtube.mode.lock().unwrap().hang_up = 1;
    let flaky = enqueue(bob.clone(), "trend-flaky").await;
    let flaky_id = flaky["run"]["id"].as_str().unwrap().to_string();
    let run4 = server.run_until(&bob, &flaky_id, finished).await;
    assert_eq!(run4["state"], "SUCCEEDED", "{run4}");
    assert_eq!(run4["attempt"], 2, "the first attempt did not get an answer: {run4}");
    // attempt 1: page 0 hung up; attempt 2: pages 0, 1 and 2 (page 0 was not journaled, as it never answered).
    assert_eq!(youtube.requests().len() - before, 4);
    assert_eq!(rows(saved_sql).await.len(), 6);

    // ---- WBS 7.2.9: the approval of a video request registers the video, its two initial documents and the acceptance. ----
    let approve = |token: String, key: &'static str, request: &'static str, category: &'static str| {
        let (address, fingerprint) = (server.address.clone(), server.fingerprint.clone());
        async move {
            post(
                &address,
                "/jobs/enqueue",
                Some(&token),
                &fingerprint,
                &json!({"operation": "approveVideoRequest", "key": key, "delayMs": 0, "input": {
                    "request": request, "youtube_title": "승인된 영상", "channel_name": "승인 채널", "duration": 321, "category": category}}),
            )
            .await
        }
    };
    const PENDING_REQUEST: &str = "ffffffff-0000-4000-8000-000000000001";
    let status_of = |request: &'static str| {
        let driver = admin.clone();
        async move {
            let mut conn = driver.pool().get_conn().await.unwrap();
            conn.query_first::<String, _>(format!("SELECT status FROM `{SOURCE}`.video_request WHERE video_request_id = '{request}'")).await.unwrap()
        }
    };
    let videos_of = |url: &'static str| {
        let driver = admin.clone();
        async move {
            let mut conn = driver.pool().get_conn().await.unwrap();
            conn.query::<(String, String, Option<String>, String, Option<i64>, Option<i64>, Option<i64>), _>(format!(
                "SELECT video_id, title, channel_name, category, duration, view_count, is_deleted FROM `{SOURCE}`.video WHERE youtube_url = '{url}'"
            ))
            .await
            .unwrap()
        }
    };
    assert_eq!(approve(alice.clone(), "approve-alice", PENDING_REQUEST, "vlog").await["ok"], false, "a member cannot approve");
    let first = approve(bob.clone(), "approve-1", PENDING_REQUEST, "vlog").await;
    assert_eq!(first["ok"], true, "{first}");
    let first_id = first["run"]["id"].as_str().unwrap().to_string();
    let approved = server.run_until(&bob, &first_id, finished).await;
    assert_eq!(approved["state"], "SUCCEEDED", "{approved}");
    let outcome = server.post("/jobs/result", Some(&bob), &json!({"id": first_id})).await;
    let new_video = outcome["output"]["video_id"].as_str().expect("the new video id is the job result").to_string();
    assert_eq!(
        videos_of("reqvideo001").await,
        [(new_video.clone(), "승인된 영상".to_string(), Some("승인 채널".to_string()), "vlog".to_string(), Some(321), Some(0), Some(0))],
        "the video the administrator described, active, with no views"
    );
    assert_eq!(status_of(PENDING_REQUEST).await.as_deref(), Some("ACCEPTED"));
    for collection in [&distributions, &timelines] {
        assert_eq!(collection.count_documents(mongodb::bson::doc! {"video_id": &new_video}).await.unwrap(), 1, "one initial document each");
    }
    let again = approve(bob.clone(), "approve-1", PENDING_REQUEST, "vlog").await;
    assert_eq!(again["run"]["id"], first_id.as_str(), "the same key is the same run");

    // Nothing is approved twice; a request for a video the service already has, and an unknown request, fail without a trace.
    setup
        .query_drop(format!(
            "INSERT INTO `{SOURCE}`.video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, created_at, updated_at) VALUES \
             ('ffffffff-0000-4000-8000-000000000002', '11111111-1111-4111-8111-111111111111', 'yt000000001', 'https://www.youtube.com/watch?v=yt000000001', 'PENDING', NOW(), NOW()), \
             ('ffffffff-0000-4000-8000-000000000003', '11111111-1111-4111-8111-111111111111', 'nvideo00001', 'https://www.youtube.com/watch?v=nvideo00001', 'PENDING', NOW(), NOW())"
        ))
        .await
        .unwrap();
    let before_documents = distributions.count_documents(mongodb::bson::doc! {}).await.unwrap();
    for (key, request, label) in [
        ("approve-2", PENDING_REQUEST, "already processed"),
        ("approve-dup", "ffffffff-0000-4000-8000-000000000002", "the video exists"),
        ("approve-none", "ffffffff-0000-4000-8000-0000000000ff", "no such request"),
    ] {
        let accepted = approve(bob.clone(), key, request, "vlog").await;
        let run = server.run_until(&bob, accepted["run"]["id"].as_str().unwrap(), finished).await;
        assert_eq!(run["state"], "FAILED", "{label}: {run}");
    }
    assert_eq!(status_of("ffffffff-0000-4000-8000-000000000002").await.as_deref(), Some("PENDING"), "a refused approval leaves the request");
    assert_eq!(videos_of("reqvideo001").await.len(), 1);
    assert_eq!(distributions.count_documents(mongodb::bson::doc! {}).await.unwrap(), before_documents, "and writes no document");

    // A client cannot accept a request by itself while no video has been registered for it.
    let direct = post(
        &server.address,
        "/apply",
        Some(&bob),
        &server.fingerprint,
        &json!({"key": "accept-direct", "request": {"apply": "VideoRequest.accept", "target": {"ids": ["ffffffff-0000-4000-8000-000000000003"]}}}),
    )
    .await;
    assert_ne!(direct["ok"], true, "{direct}");
    assert_eq!(status_of("ffffffff-0000-4000-8000-000000000003").await.as_deref(), Some("PENDING"));

    // ---- WBS 7.2.8: the my page screens equal the original MypageService's answers on the same sessions (recorded by
    // reference/slice14e_oracle.py, which ran the original on this data). The fixture's cached DNA row is removed: the port
    // recomputes on every call, the original answers from a fresh cache.
    let sessions: Vec<Value> = serde_json::from_str(MYPAGE_SESSIONS).unwrap();
    let watching = database.collection::<mongodb::bson::Document>("youtube_watching_data");
    for session in &sessions {
        let document = serde_json::from_value::<mongodb::bson::Document>(session.clone()).unwrap();
        watching.insert_one(document).await.unwrap();
    }
    setup.query_drop(format!("DELETE FROM `{SOURCE}`.user_emotion_dna")).await.unwrap();
    let golden: Vec<Value> = serde_json::from_str(MYPAGE_GOLDEN).unwrap();
    assert!(golden.len() >= 60, "the recorded cases");
    for case in &golden {
        let subject = case["subject"].as_str().unwrap();
        let answer = server.post("/extension", Some(&bearer(subject)), &json!({"extension": case["extension"], "input": case["input"]})).await;
        assert_eq!(answer["ok"], true, "{}: {answer}", case["name"]);
        let mut got: Value = serde_json::from_str(answer["output"]["body"].as_str().unwrap()).unwrap();
        if let Some(object) = got.as_object_mut() {
            object.remove("generated_at");
        }
        assert_eq!(got, case["expected"], "{}", case["name"]);
    }
    // A guest has no my page, and a member's screens never show another member's sessions (bob's answers above hold only bob's).
    let guest = server.post("/extension", None, &json!({"extension": "User.myHighlight", "input": {}})).await;
    assert_ne!(guest["ok"], true, "{guest}");

    // ---- WBS 7.2.8: GET mypage/emotion/summary (the write extension User.myEmotionSummary) equals the original's get_emotion_summary
    // on the recorded cases (reference/slice14f_oracle.py ran the original on this data and these starting rows): the answer and the
    // row afterwards (totals, checkpoint, session id, lock_version).
    watching.delete_many(mongodb::bson::doc! {}).await.unwrap();
    let summary_sessions: Vec<Value> = serde_json::from_str(SUMMARY_SESSIONS).unwrap();
    for session in &summary_sessions {
        watching.insert_one(serde_json::from_value::<mongodb::bson::Document>(session.clone()).unwrap()).await.unwrap();
    }
    let summary_golden: Vec<Value> = serde_json::from_str(SUMMARY_GOLDEN).unwrap();
    assert!(summary_golden.len() >= 15, "the recorded cases");
    let mysql_time = |iso: &str| -> String {
        let text = iso.replace('T', " ");
        match text.split_once('.') {
            Some((head, fraction)) => format!("{head}.{:0<3}", &fraction[..fraction.len().min(3)]),
            None => format!("{text}.000"),
        }
    };
    let user_of = |name: &str| SUBJECTS.iter().find(|(candidate, _)| *candidate == name).unwrap().1;
    for (index, case) in summary_golden.iter().enumerate() {
        let (name, subject) = (case["name"].as_str().unwrap(), case["subject"].as_str().unwrap());
        let user = user_of(subject);
        setup.query_drop(format!("DELETE FROM `{SOURCE}`.user_emotion_summary WHERE user_id = '{user}'")).await.unwrap();
        if let Some(state) = case["state"].as_object() {
            let totals: Vec<String> = state["seconds"].as_array().unwrap().iter().map(|n| n.to_string()).collect();
            let at = state["at"].as_str().map_or("NULL".to_string(), |at| format!("'{}'", mysql_time(at)));
            let session = state.get("session").and_then(Value::as_str).map_or("NULL".to_string(), |id| format!("'{id}'"));
            setup
                .query_drop(format!(
                    "INSERT INTO `{SOURCE}`.user_emotion_summary (user_id, neutral_seconds, happy_seconds, surprise_seconds, sad_seconds, angry_seconds, \
                     last_finalized_at, last_session_id, lock_version, created_at, updated_at) VALUES ('{user}', {}, {at}, {session}, {}, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
                    totals.join(", "),
                    state["version"]
                ))
                .await
                .unwrap();
        }
        let answer = server
            .post(
                "/apply",
                Some(&bearer(subject)),
                &json!({"key": format!("summary-{index}"), "request": {"extension": "User.myEmotionSummary", "input": {}}}),
            )
            .await;
        assert_eq!(answer["ok"], true, "{name}: {answer}");
        let got: Value = serde_json::from_str(answer["output"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(got, case["expected"], "{name}");
        let row: SummaryRow = setup
            .query_first(format!(
                "SELECT neutral_seconds, happy_seconds, surprise_seconds, sad_seconds, angry_seconds, CAST(last_finalized_at AS CHAR), last_session_id, \
                 lock_version, ABS(TIMESTAMPDIFF(SECOND, last_finalized_at, UTC_TIMESTAMP())) < 120, updated_at <> '2026-01-01 00:00:00' \
                 FROM `{SOURCE}`.user_emotion_summary WHERE user_id = '{user}'"
            ))
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{name}: the row exists"));
        let want = &case["row"];
        let seconds: Vec<i64> = [&row.0, &row.1, &row.2, &row.3, &row.4].iter().map(|n| n.parse().unwrap()).collect();
        assert_eq!(json!(seconds), want["seconds"], "{name}: totals");
        assert_eq!(row.6.as_deref().unwrap_or(""), want["session"].as_str().unwrap_or(""), "{name}: session id");
        assert_eq!(json!(row.7), want["version"], "{name}: lock_version");
        match want["at"].as_str() {
            Some("recent") => assert_eq!(row.8, 1, "{name}: the checkpoint is recent"),
            Some(at) => assert_eq!(row.5.as_deref(), Some(mysql_time(at).as_str()), "{name}: checkpoint"),
            None => assert!(row.5.is_none(), "{name}: no checkpoint"),
        }
        assert_eq!(row.9 == 1, want["updated"] == "touched", "{name}: updated_at");
    }
    // Four requests at once from the same member with sessions waiting: the totals are added once. A request that lost the lock_version
    // is refused with WIRE.CONFLICT (or WIRE.WORKER_BUSY beyond the local worker limit) and may be sent again; one that came after
    // another finds nothing new.
    let bob = user_of("bob");
    // Enough sessions that the two calls the principal may run at once both read the row before either one writes.
    let heavy: Vec<mongodb::bson::Document> = (0..3000_i64)
        .map(|n| {
            mongodb::bson::doc! {"user_id": bob, "video_id": "bbbbbbbb-0000-4000-8000-000000000002", "video_view_log_id": format!("heavy-{n:05}"),
                "created_at": mongodb::bson::DateTime::from_millis(1_772_323_200_000), "finalized_at": mongodb::bson::DateTime::from_millis(1_772_323_200_000 + n * 1000),
                "emotion_seconds": {"neutral": 1, "happy": 0, "surprise": 0, "sad": 0, "angry": 0}}
        })
        .collect();
    watching.insert_many(heavy).await.unwrap();
    for round in 0..8 {
        setup.query_drop(format!("DELETE FROM `{SOURCE}`.user_emotion_summary WHERE user_id = '{bob}'")).await.unwrap();
        setup
            .query_drop(format!(
                "INSERT INTO `{SOURCE}`.user_emotion_summary (user_id, neutral_seconds, happy_seconds, surprise_seconds, sad_seconds, angry_seconds, \
                 last_finalized_at, last_session_id, lock_version, created_at, updated_at) VALUES ('{bob}', 0, 0, 0, 0, 0, '2026-02-28 10:00:00.000', '', 0, \
                 '2026-01-01 00:00:00', '2026-01-01 00:00:00')"
            ))
            .await
            .unwrap();
        let request = |key: &str| json!({"key": key, "request": {"extension": "User.myEmotionSummary", "input": {}}});
        let (bob_token, keys) = (bearer("bob"), ["c1", "c2", "c3", "c4"].map(|key| request(&format!("{key}-{round}"))));
        let results = tokio::join!(
            server.post("/apply", Some(&bob_token), &keys[0]),
            server.post("/apply", Some(&bob_token), &keys[1]),
            server.post("/apply", Some(&bob_token), &keys[2]),
            server.post("/apply", Some(&bob_token), &keys[3])
        );
        let results = [results.0, results.1, results.2, results.3];
        assert!(results.iter().any(|r| r["ok"] == true), "{results:?}");
        for refused in results.iter().filter(|r| r["ok"] != true) {
            assert!(["WIRE.CONFLICT", "WIRE.WORKER_BUSY"].contains(&refused["code"].as_str().unwrap_or("")), "{refused}");
        }
        let (total, version): (String, i64) = setup
            .query_first(format!("SELECT neutral_seconds, lock_version FROM `{SOURCE}`.user_emotion_summary WHERE user_id = '{bob}'"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((total.as_str(), version), ("3100", 1), "the sessions are added once: {results:?}");
    }
    // The transition the extension calls states its own contract: a stale lock_version is refused and changes nothing, the current one
    // applies and moves the version, and another member cannot touch the row. (The transition is public: a member can also write
    // their own totals directly. Nothing else reads this table, so only their own summary screen is affected.)
    let alice = user_of("alice");
    setup.query_drop(format!("DELETE FROM `{SOURCE}`.user_emotion_summary WHERE user_id = '{alice}'")).await.unwrap();
    setup
        .query_drop(format!(
            "INSERT INTO `{SOURCE}`.user_emotion_summary (user_id, neutral_seconds, happy_seconds, surprise_seconds, sad_seconds, angry_seconds, \
             last_finalized_at, last_session_id, lock_version, created_at, updated_at) VALUES ('{alice}', 1, 2, 3, 4, 5, NULL, NULL, 5, '2026-01-01 00:00:00', '2026-01-01 00:00:00')"
        ))
        .await
        .unwrap();
    let direct = |key: &str, expect: i64| json!({"key": key, "request": {"apply": "UserEmotionSummary.applyDelta", "target": {"ids": [alice]}, "values": {"neutral_seconds": 50}, "expect": {"lock_version": expect}}});
    let state_query = format!("SELECT neutral_seconds, lock_version FROM `{SOURCE}`.user_emotion_summary WHERE user_id = '{alice}'");
    let alice_token = bearer("alice");
    let stale = server.post("/apply", Some(&alice_token), &direct("stale", 4)).await;
    assert_eq!(stale["code"], "WIRE.CONFLICT", "{stale}");
    assert_eq!(
        setup.query_first::<(String, i64), _>(state_query.as_str()).await.unwrap().unwrap(),
        ("1".to_string(), 5),
        "a stale lock_version changes nothing"
    );
    let current = server.post("/apply", Some(&alice_token), &direct("current", 5)).await;
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(
        setup.query_first::<(String, i64), _>(state_query.as_str()).await.unwrap().unwrap(),
        ("50".to_string(), 6),
        "the current lock_version applies and moves the version"
    );
    let foreign = server.post("/apply", Some(&bearer("bob")), &direct("foreign", 6)).await;
    assert_ne!(foreign["ok"], true, "{foreign}");
    assert_eq!(
        setup.query_first::<(String, i64), _>(state_query.as_str()).await.unwrap().unwrap(),
        ("50".to_string(), 6),
        "another member cannot change the row"
    );
    // A guest has no summary.
    let guest = server.post("/apply", None, &json!({"key": "guest", "request": {"extension": "User.myEmotionSummary", "input": {}}})).await;
    assert_ne!(guest["ok"], true, "{guest}");

    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
    }
    setup.query_drop("DROP USER 'aip_t_yt_rt'@'%'").await.unwrap();
}
