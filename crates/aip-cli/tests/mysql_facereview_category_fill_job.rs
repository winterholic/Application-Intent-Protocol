#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.7: the original's Celery beat task `fill_youtube_category_videos` (YoutubeCategoryFillJob) as an AIP durable job, through the
//! real `aip service` binary, the MySQL 8.4 schema of the original, a real MongoDB behind `workers.mongo` (with write collections), the
//! Python worker sandbox and a stand-in YouTube Data API behind the query-string integrations `youtube_search` and `youtube_videos`.
//!
//! What it proves (fixture: `product/tests/fixtures/facereview-mysql`, definition `fillCategoryVideos`):
//!  * only an administrator starts it; the plan is the original's: the four emptiest genres in its order, three queries each, twelve
//!    searches at most, a quota of 100 per search and 1 per details call, and the new videos saved under the searched genre with
//!    their two initial MongoDB documents (the oracle `reference/slice14b_oracle.py` runs the original class on the same data and gets
//!    the same 36 rows, 72 documents and quota 1212);
//!  * the same key is one run and asks the provider nothing more;
//!  * a 403 spends the rest of the quota, so the search that got it counts and no genre after it is searched (the original checks the quota
//!    only between genres, so the rest of that genre's queries still run);
//!  * a provider that does not answer asks for the run to be tried again, and the answers that did arrive are replayed.
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

fn fnv(text: &str) -> String {
    let mut value: u32 = 2_166_136_261;
    for byte in text.bytes() {
        value = (value ^ u32::from(byte)).wrapping_mul(16_777_619);
    }
    format!("{value:08x}")
}

fn video(id: &str) -> Value {
    json!({"id": id, "snippet": {"title": format!("Title {id}"), "channelTitle": "Channel", "categoryId": "22", "tags": [], "description": ""},
        "contentDetails": {"duration": "PT5M"}, "statistics": {"viewCount": "1"}})
}

/// A search answers two ids the database already has (one active, one soft-deleted) and three new ones per query, and a channel that
/// is not a video; the details of any id are a plain video.
fn answer(path: &str, query: &std::collections::BTreeMap<String, String>) -> Value {
    if path.ends_with("/search") {
        let digest = fnv(&query["q"]);
        let mut ids = vec!["yt000000001".to_string(), "yt000000006".to_string()];
        ids.extend((0..3).map(|n| format!("fv{digest}{n}")));
        let mut items = vec![json!({"id": {"kind": "youtube#channel", "channelId": "chan"}})];
        items.extend(ids.iter().map(|id| json!({"id": {"kind": "youtube#video", "videoId": id}})));
        return json!({"items": items});
    }
    json!({"items": query["id"].split(',').map(video).collect::<Vec<_>>()})
}

#[derive(Default)]
struct YoutubeMode {
    /// Answer 403 (quota) to the nth search (1-based).
    quota_at_search: Option<usize>,
    /// Hang up on this many next requests without answering.
    hang_up: usize,
    searches: usize,
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
                    let parsed = url::Url::parse(&format!("http://peer{target}")).unwrap();
                    let (status, body) = {
                        let mut mode = switch.lock().unwrap();
                        if mode.hang_up > 0 {
                            mode.hang_up -= 1;
                            return;
                        }
                        if parsed.path().ends_with("/search") {
                            mode.searches += 1;
                            if mode.quota_at_search == Some(mode.searches) {
                                ("403 Forbidden", json!({"error": {"code": 403, "message": "quotaExceeded"}}))
                            } else {
                                ("200 OK", answer(parsed.path(), &query_of(&target)))
                            }
                        } else {
                            ("200 OK", answer(parsed.path(), &query_of(&target)))
                        }
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

    fn searches(&self) -> usize {
        self.requests().iter().filter(|target| target.contains("/search?")).count()
    }
}

fn query_of(target: &str) -> std::collections::BTreeMap<String, String> {
    url::Url::parse(&format!("http://peer{target}")).unwrap().query_pairs().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 (scripts/mysql-live-fixture.sh), MongoDB (AIP_TEST_MONGO_URL) and a worker sandbox; run with -- --ignored"]
async fn the_category_fill_job_searches_the_thinnest_genres_within_the_original_budget() {
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
    setup.query_drop("DROP USER IF EXISTS 'aip_t_fill_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_fill_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_fill_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let youtube = Youtube::start().await;
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_fill_rt:runtime_test_only@{host}/{SOURCE}"),
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
    let integration = |name: &str, path: &str, params: Value| {
        json!({"name": name, "url": format!("http://{}/youtube/v3/{path}", youtube.address), "method": "GET", "timeout_ms": 3000,
            "max_request_bytes": 4096, "max_response_bytes": 1048576, "allow_loopback": true, "query_params": params,
            "secret_query_env": {"name": "key", "env": "AIP_T_YT_API_KEY"}})
    };
    std::fs::write(
        &config,
        json!({
            "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
            "database_url_env": "AIP_T_YT_RUNTIME_URL", "migrate_database_url_env": "AIP_T_YT_ADOPT_URL",
            "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
            // Twelve searches of three new videos make about 150 ctx calls; the default of 64 would end the run early.
            "limits": {"job_calls": 400},
            "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
            "workers": {"directory": "extensions", "language": "python",
                "mongo": {"uri_env": "AIP_T_YT_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000,
                    "write_collections": ["video_distribution", "video_timeline_emotion_count"], "ensure_indexes": true},
                "integrations": [
                    integration("youtube_videos", "videos", json!(["part", "chart", "regionCode", "maxResults", "pageToken", "id"])),
                    integration("youtube_search", "search", json!(["part", "q", "type", "regionCode", "relevanceLanguage", "maxResults", "order"])),
                ]},
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
                &json!({"operation": "fillCategoryVideos", "input": {}, "key": key, "delayMs": 0}),
            )
            .await
        }
    };
    let clear = || async {
        let mut conn = admin.pool().get_conn().await.unwrap();
        conn.query_drop(format!("DELETE FROM `{SOURCE}`.video WHERE youtube_url LIKE 'fv%'")).await.unwrap();
        distributions.delete_many(mongodb::bson::doc! {}).await.unwrap();
        timelines.delete_many(mongodb::bson::doc! {}).await.unwrap();
    };

    // A member may not start the collection; the administrator may.
    let refused = enqueue(alice.clone(), "fill-alice").await;
    assert_eq!(refused["ok"], false, "{refused}");
    let accepted = enqueue(bob.clone(), "fill-1").await;
    assert_eq!(accepted["ok"], true, "{accepted}");
    let id = accepted["run"]["id"].as_str().unwrap().to_string();
    let run1 = server.run_until(&bob, &id, finished).await;
    assert_eq!(run1["state"], "SUCCEEDED", "{run1}");
    let result = server.post("/jobs/result", Some(&bob), &json!({"id": id})).await;
    // Twelve searches (the original's MAX_SEARCH_CALLS) of three queries for each of the four emptiest genres, in the original's
    // order; every search costs 100 and every details call 1; a query saves its three new videos (the two the database has are not).
    assert_eq!(result["output"], json!({"saved": 36, "searches": 12, "quota": 1212}), "{result}");
    let seen = youtube.requests();
    assert_eq!((seen.len(), youtube.searches()), (24, 12), "{seen:?}");
    for target in &seen {
        let query = query_of(target);
        assert_eq!(query["key"], API_KEY, "the server adds its key: {target}");
        if target.contains("/search?") {
            assert_eq!((query["type"].as_str(), query["regionCode"].as_str(), query["relevanceLanguage"].as_str()), ("video", "KR", "ko"));
            assert_eq!((query["maxResults"].as_str(), query["order"].as_str()), ("20", "viewCount"));
        }
    }
    assert_eq!(query_of(&seen[0])["q"], "먹방 한국", "the emptiest genre, in the original's order, is searched first");
    let genres: Vec<(String, i64)> = setup
        .query(format!(
            "SELECT category, COUNT(*) FROM `{SOURCE}`.video WHERE youtube_url LIKE 'fv%' GROUP BY category ORDER BY CAST(category AS CHAR)"
        ))
        .await
        .unwrap();
    assert_eq!(
        genres,
        [("drama".to_string(), 9), ("eating".to_string(), 9), ("exercise".to_string(), 9), ("horror".to_string(), 9)],
        "nine new videos for each searched genre, saved under that genre"
    );
    assert_eq!(distributions.count_documents(mongodb::bson::doc! {}).await.unwrap(), 36);
    assert_eq!(timelines.count_documents(mongodb::bson::doc! {}).await.unwrap(), 36);

    // The same key is one run and asks the provider nothing more.
    let again = enqueue(bob.clone(), "fill-1").await;
    assert_eq!(again["run"]["id"], id.as_str());
    assert_eq!(youtube.requests().len(), 24);

    // A 403 spends the rest of the quota: the search that got it counts and no genre after it is searched, but the original checks the
    // quota only between genres, so the rest of that genre's queries still run (search 5 fails, search 6 is the genre's third query).
    clear().await;
    youtube.mode.lock().unwrap().quota_at_search = Some(youtube.searches() + 5);
    let quota = enqueue(bob.clone(), "fill-quota").await;
    let run2 = server.run_until(&bob, quota["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(run2["state"], "SUCCEEDED", "{run2}");
    let second = server.post("/jobs/result", Some(&bob), &json!({"id": quota["run"]["id"]})).await;
    assert_eq!(second["output"], json!({"saved": 15, "searches": 6, "quota": 1302}), "{second}");
    youtube.mode.lock().unwrap().quota_at_search = None;

    // A provider that does not answer asks for the run to be tried again; answers that did arrive are replayed, not asked for twice.
    clear().await;
    let before = youtube.requests().len();
    youtube.mode.lock().unwrap().hang_up = 1;
    let flaky = enqueue(bob.clone(), "fill-flaky").await;
    let run3 = server.run_until(&bob, flaky["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(run3["state"], "SUCCEEDED", "{run3}");
    assert_eq!(run3["attempt"], 2, "{run3}");
    assert_eq!(youtube.requests().len() - before, 25, "one request that hung up, then the 24 of a whole run");
    let third = server.post("/jobs/result", Some(&bob), &json!({"id": flaky["run"]["id"]})).await;
    assert_eq!(third["output"], json!({"saved": 36, "searches": 12, "quota": 1212}), "{third}");

    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
    }
    setup.query_drop("DROP USER 'aip_t_fill_rt'@'%'").await.unwrap();
}
