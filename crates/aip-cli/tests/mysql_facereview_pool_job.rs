#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.7: the original's Celery beat task `rebuild_recommendation_pool` as an AIP durable job, through the real `aip service` binary,
//! the MySQL 8.4 schema of the original (pinned new-facereview@7948b7e7), a real MongoDB and Redis behind `workers.mongo` and
//! `workers.redis`, and the Python worker sandbox.
//!
//! What it proves (fixture: `product/tests/fixtures/facereview-mysql`, definition `rebuildRecommendationPool`):
//!  * with a cold cache the lists build themselves and equal the golden answers of the original (`slice13d-golden.json`);
//!  * the job (an administrator only) writes the original's keys (`facereview:reco:pool`, `facereview:reco:category`) with their 7200
//!    second expiry, journals every write, and converges on one run for one key;
//!  * with the cache filled the lists still equal the golden answers, a change in the data does not show until the job runs again, and
//!    an expired cache is a cold cache again;
//!  * Redis unreachable: the lists fall back to building themselves, and a job started meanwhile is tried again and converges;
//!  * a catalog far beyond 64 ctx calls needs `limits.job_calls`; the default limit fails the run loudly and leaves the cache as it was;
//!  * the configured schedule runs the job as the service principal under its slot key.
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up` with `AIP_TEST_MONGO_URL` and `AIP_TEST_REDIS_URL` pointing at
//! run-owned containers.
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
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
const EXTENSIONS: [(&str, &str); 13] = [
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
];
const META: &str = "aip_t_pool_meta";
const SOURCE: &str = "aip_t_pool_src";
const MONGO_DB: &str = "aip_t_pool_fr";
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
        .env("AIP_T_POOL_ADOPT_URL", &env.adopt_url)
        .env("AIP_T_POOL_RUNTIME_URL", &env.runtime_url)
        .env("AIP_T_POOL_MONGO_URL", &env.mongo_url)
        .env("AIP_T_POOL_REDIS_URL", &env.redis_url);
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

/// A TCP forwarder in front of Redis that can be switched off: while it is down every new connection is refused and every open one is cut.
struct SwitchProxy {
    address: SocketAddr,
    down: Arc<AtomicBool>,
}

impl SwitchProxy {
    async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let down: Arc<AtomicBool> = Arc::default();
        let switch = down.clone();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else { return };
                if switch.load(Ordering::SeqCst) {
                    continue;
                }
                let Ok(server) = TcpStream::connect(target).await else { continue };
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                let (up, back) = (switch.clone(), switch.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    while let Ok(n) = client_read.read(&mut buf).await {
                        if n == 0 || up.load(Ordering::SeqCst) || server_write.write_all(&buf[..n]).await.is_err() {
                            return;
                        }
                    }
                });
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    while let Ok(n) = server_read.read(&mut buf).await {
                        if n == 0 || back.load(Ordering::SeqCst) || client_write.write_all(&buf[..n]).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { address, down }
    }

    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

fn socket_of(url: &str) -> SocketAddr {
    let parsed = url::Url::parse(url).expect("URL");
    format!("{}:{}", parsed.host_str().unwrap(), parsed.port().unwrap()).parse().expect("socket address")
}

fn via(url: &str, proxy: SocketAddr) -> String {
    let mut parsed = url::Url::parse(url).expect("URL");
    parsed.set_port(Some(proxy.port())).unwrap();
    parsed.to_string()
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

    async fn enqueue(&self, token: &str, key: &str) -> Value {
        self.post("/jobs/enqueue", Some(token), &json!({"operation": "rebuildRecommendationPool", "input": {}, "key": key, "delayMs": 0})).await
    }
}

async fn start_pool_server(env: &Env, file: &Path, config: &Path) -> Server {
    let (child, listening) = start_server(env, file, config).await.expect("serve starts with MongoDB and Redis adapters");
    assert_eq!(listening["readAdapters"]["mongo"]["status"], "ready", "{listening}");
    assert_eq!(listening["readAdapters"]["redis"]["status"], "ready", "{listening}");
    Server {
        _child: child,
        address: listening["address"].as_str().unwrap().to_string(),
        fingerprint: listening["fingerprint"].as_str().unwrap().to_string(),
    }
}

fn finished(run: &Value) -> bool {
    ["SUCCEEDED", "FAILED", "CANCELLED"].contains(&run["state"].as_str().unwrap())
}

async fn json_keys(redis: &mut redis::aio::MultiplexedConnection) -> Vec<String> {
    let mut keys: Vec<String> = redis::cmd("KEYS").arg("facereview:reco:*").query_async(redis).await.unwrap();
    keys.sort();
    keys
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 (scripts/mysql-live-fixture.sh), MongoDB (AIP_TEST_MONGO_URL), Redis (AIP_TEST_REDIS_URL) and a worker sandbox; run with -- --ignored"]
async fn the_scheduled_pool_job_fills_the_cache_the_recommendation_lists_read() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let mongo_url = std::env::var("AIP_TEST_MONGO_URL").expect("set AIP_TEST_MONGO_URL to a run-owned MongoDB");
    let redis_url = std::env::var("AIP_TEST_REDIS_URL").expect("set AIP_TEST_REDIS_URL to a run-owned Redis");
    for (name, value) in [("MongoDB", &mongo_url), ("Redis", &redis_url)] {
        let parsed = url::Url::parse(value).expect("fixture URL");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"), "the {name} fixture must bind loopback only");
        assert!(parsed.port().is_some_and(|port| port > 1024 && port != 27017 && port != 6379), "the {name} fixture must use a dedicated high port");
    }

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
    setup.query_drop("DROP USER IF EXISTS 'aip_t_pool_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_pool_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_pool_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let redis_proxy = SwitchProxy::start(socket_of(&redis_url)).await;
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_pool_rt:runtime_test_only@{host}/{SOURCE}"),
        mongo_url: mongo_url.clone(),
        redis_url: via(&redis_url, redis_proxy.address),
    };

    // MongoDB: the run-owned database is rebuilt from the same documents the original saw.
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
    let redis_client = redis::Client::open(redis_url.as_str()).expect("Redis fixture");
    let mut redis = redis_client.get_multiplexed_async_connection().await.expect("Redis connection");
    let stale: Vec<String> = json_keys(&mut redis).await;
    if !stale.is_empty() {
        let _: i64 = redis::cmd("DEL").arg(&stale).query_async(&mut redis).await.unwrap();
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
    let write_config = |job_calls: Option<u64>, schedule: bool| {
        let mut document = json!({
            "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
            "database_url_env": "AIP_T_POOL_RUNTIME_URL", "migrate_database_url_env": "AIP_T_POOL_ADOPT_URL",
            "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
            "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
            "workers": {"directory": "extensions", "language": "python",
                "integrations": [{"name": "youtube_videos", "url": "http://127.0.0.1:9/youtube/v3/videos", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}, {"name": "youtube_search", "url": "http://127.0.0.1:9/youtube/v3/search", "method": "GET", "timeout_ms": 1000, "max_request_bytes": 1024, "max_response_bytes": 1024, "allow_loopback": true, "query_params": ["part"]}],
                "mongo": {"uri_env": "AIP_T_POOL_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000},
                "redis": {"uri_env": "AIP_T_POOL_REDIS_URL", "prefixes": ["facereview:reco:"], "max_in_flight": 4, "timeout_ms": 3000}},
        });
        if let Some(calls) = job_calls {
            document["limits"] = json!({"job_calls": calls});
        }
        if schedule {
            document["schedules"] = json!([{"name": "pool", "operation": "rebuildRecommendationPool", "cron": "* * * * *", "principal": "svc"}]);
        }
        std::fs::write(&config, document.to_string()).unwrap();
    };
    write_config(Some(400), false);
    let file_arg = file.to_str().unwrap();
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    for (subject, actor) in SUBJECTS {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &config).await.expect("bind");
    }
    // The server schedule acts as this principal: Bob's account, an administrator.
    run(&env, &["principal", "bind", "--subject", "svc", "--actor", SUBJECTS[1].1], &config).await.expect("bind the service principal");

    let tokens: Vec<(&str, String)> = SUBJECTS.iter().map(|(subject, _)| (*subject, token(&private, subject))).collect();
    let bearer = |name: &str| tokens.iter().find(|(candidate, _)| *candidate == name).unwrap().1.clone();
    let (alice, bob) = (bearer("alice"), bearer("bob"));
    let ask = |server: &Server, extension: &str, subject: &str, input: &Value| {
        let (token, body) = (bearer(subject), json!({"extension": extension, "input": input}));
        let (address, fingerprint) = (server.address.clone(), server.fingerprint.clone());
        async move { post(&address, "/extension", Some(&token), &fingerprint, &body).await }
    };
    let golden: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    assert!(golden.len() >= 40, "the golden file lost cases");
    // Every golden case: the original answered, so must the server, with or without the cache behind it.
    let replay = |server: &Server, label: &'static str| {
        let golden = golden.clone();
        let asks: Vec<_> = golden
            .iter()
            .map(|case| {
                let (extension, subject) = (case["extension"].as_str().unwrap().to_string(), case["subject"].as_str().map(str::to_string));
                let input = case["input"].clone();
                (case.clone(), ask(server, &extension, subject.as_deref().unwrap(), &input))
            })
            .collect();
        async move {
            for (case, reply) in asks {
                let reply = reply.await;
                let (name, extension) = (case["name"].as_str().unwrap(), case["extension"].as_str().unwrap());
                if case["expect_error"] == true {
                    assert_eq!(reply["ok"], false, "{label}/{name}: the original refuses: {reply}");
                    continue;
                }
                assert_eq!(reply["ok"], true, "{label}/{name}: {reply}");
                assert_eq!(answer(extension, &reply["output"]), case["expected"], "{label}/{name}");
            }
        }
    };
    let category_ids = |reply: &Value| -> Vec<String> {
        answer("Video.homeCategory", &reply["output"])
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|category| category["videos"].as_array().unwrap().iter().map(|video| video["video_id"].as_str().unwrap().to_string()))
            .collect()
    };

    // ---- Phase 1: the cache is empty, then the job fills it. ----
    let server = start_pool_server(&env, &file, &config).await;
    replay(&server, "cold cache").await;
    assert!(json_keys(&mut redis).await.is_empty(), "a read never writes the cache");

    // Only an administrator may start the job: the original's task had no caller, here the allow policy decides.
    let refused = server.enqueue(&alice, "pool-alice").await;
    assert_eq!(refused["ok"], false, "a member may not rebuild the pool: {refused}");

    let accepted = server.enqueue(&bob, "pool-1").await;
    assert_eq!(accepted["ok"], true, "{accepted}");
    let id = accepted["run"]["id"].as_str().unwrap().to_string();
    let duplicate = server.enqueue(&bob, "pool-1").await;
    assert_eq!(duplicate["run"]["id"], id.as_str(), "the same key converges on one durable run");
    let run1 = server.run_until(&bob, &id, finished).await;
    assert_eq!(run1["state"], "SUCCEEDED", "{run1}");
    let result = server.post("/jobs/result", Some(&bob), &json!({"id": id})).await;
    let (pool_len, category_count) = (result["output"]["pool"].as_u64().unwrap(), result["output"]["categories"].as_u64().unwrap());
    assert!(pool_len > 0 && category_count > 0, "{result}");
    assert_eq!(
        json_keys(&mut redis).await,
        ["facereview:reco:category", "facereview:reco:category_order", "facereview:reco:pool"],
        "the job writes the original's keys and one more for the category order"
    );
    let raw_pool: String = redis::cmd("GET").arg("facereview:reco:pool").query_async(&mut redis).await.unwrap();
    let cached_pool: Vec<Value> = serde_json::from_str(&raw_pool).unwrap();
    assert_eq!(cached_pool.len() as u64, pool_len);
    assert!(cached_pool.windows(2).all(|pair| pair[0]["base_score"].as_f64() >= pair[1]["base_score"].as_f64()), "the pool is ranked by base_score");
    let fields: std::collections::BTreeMap<String, String> =
        redis::cmd("HGETALL").arg("facereview:reco:category").query_async(&mut redis).await.unwrap();
    assert_eq!(fields.len() as u64, category_count);
    let order: Vec<String> =
        serde_json::from_str(&redis::cmd("GET").arg("facereview:reco:category_order").query_async::<String>(&mut redis).await.unwrap()).unwrap();
    assert_eq!(order.iter().collect::<std::collections::BTreeSet<_>>(), fields.keys().collect::<std::collections::BTreeSet<_>>());
    for key in ["facereview:reco:pool", "facereview:reco:category", "facereview:reco:category_order"] {
        let ttl: i64 = redis::cmd("TTL").arg(key).query_async(&mut redis).await.unwrap();
        assert!((7000..=7200).contains(&ttl), "{key} lives at most the original's 7200 seconds: {ttl}");
    }
    assert_eq!(
        setup
            .query_first::<i64, _>(format!("SELECT COUNT(*) FROM `{META}`.aip_effects WHERE run_id='{id}' AND state='SUCCEEDED'"))
            .await
            .unwrap()
            .unwrap(),
        3,
        "every write of the run is journaled"
    );

    // The lists now come from the cache: the answers are the same, and a change in the data does not show until the job runs again.
    replay(&server, "warm cache").await;
    let before = category_ids(&ask(&server, "Video.homeCategory", "alice", &json!({})).await);
    let hidden = "bbbbbbbb-0000-4000-8000-000000000001";
    assert!(before.iter().any(|video| video == hidden), "the fixture lists {hidden} for Alice");
    setup.query_drop(format!("UPDATE `{SOURCE}`.video SET is_deleted=1 WHERE video_id='{hidden}'")).await.unwrap();
    let stale = category_ids(&ask(&server, "Video.homeCategory", "alice", &json!({})).await);
    assert!(stale.iter().any(|video| video == hidden), "a cached list stays as the job wrote it until the job or the expiry replaces it");
    let second = server.enqueue(&bob, "pool-2").await;
    let run2 = server.run_until(&bob, second["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(run2["state"], "SUCCEEDED", "{run2}");
    let fresh = category_ids(&ask(&server, "Video.homeCategory", "alice", &json!({})).await);
    assert!(!fresh.iter().any(|video| video == hidden), "the next job run drops the deleted video");
    // An expired cache (the original's TTL) is a cold cache again: the lists build themselves and still answer.
    let _: i64 = redis::cmd("DEL").arg(json_keys(&mut redis).await).query_async(&mut redis).await.unwrap();
    let expired = category_ids(&ask(&server, "Video.homeCategory", "alice", &json!({})).await);
    assert_eq!(expired.iter().collect::<std::collections::BTreeSet<_>>(), fresh.iter().collect::<std::collections::BTreeSet<_>>());
    setup.query_drop(format!("UPDATE `{SOURCE}`.video SET is_deleted=0 WHERE video_id='{hidden}'")).await.unwrap();

    // Redis unreachable: reads fall back to building the lists, as the original does when its cache does not answer.
    redis_proxy.set_down(true);
    replay(&server, "redis down").await;
    // ... and a job started now is tried again until Redis is back, then converges: nothing was written twice.
    let retried = server.enqueue(&bob, "pool-3").await;
    let retried_id = retried["run"]["id"].as_str().unwrap().to_string();
    server
        .run_until(&bob, &retried_id, |run| run["attempt"].as_i64().unwrap_or(0) >= 1 && run["state"] != "RUNNING" && run["state"] != "SUCCEEDED")
        .await;
    redis_proxy.set_down(false);
    let run3 = server.run_until(&bob, &retried_id, finished).await;
    assert_eq!(run3["state"], "SUCCEEDED", "{run3}");
    assert!(run3["attempt"].as_i64().unwrap() >= 2, "the run was tried again after Redis failed: {run3}");
    assert_eq!(json_keys(&mut redis).await.len(), 3);

    // ---- Phase 2: a catalog far beyond 64 ctx calls. ----
    setup.query_drop("SET SESSION cte_max_recursion_depth=10000").await.unwrap();
    setup
        .query_drop(format!(
            "INSERT INTO `{SOURCE}`.video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) \
             SELECT UUID(), CONCAT('big', n), CONCAT('big video ', n), 'big channel', 'etc', 100, n, 0, '2026-10-01 00:00:00', '2026-10-01 00:00:00' \
             FROM (WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 3500) SELECT n FROM s) t"
        ))
        .await
        .unwrap();
    let big_ids: Vec<(String, i64)> =
        setup.query(format!("SELECT video_id, view_count FROM `{SOURCE}`.video WHERE youtube_url LIKE 'big%' ORDER BY view_count")).await.unwrap();
    assert_eq!(big_ids.len(), 3500);
    let documents = database.collection::<mongodb::bson::Document>("video_distribution");
    let batch: Vec<mongodb::bson::Document> = big_ids
        .iter()
        .map(|(video_id, n)| {
            mongodb::bson::serialize_to_document(&json!({
                "video_id": video_id, "category": "etc", "duration": 100, "dominant_emotion": "happy",
                "emotion_averages": {"neutral": 0.1, "happy": 0.5 + (*n % 40) as f64 / 100.0, "surprise": 0.1, "sad": 0.1, "angry": 0.1},
                "emotion_counts": {"neutral": 1, "happy": 5, "surprise": 1, "sad": 1, "angry": 1}, "total_frames": 9,
                "average_completion_rate": 0.5, "recommendation_scores": {"happy": 0.5},
                "created_at": "2026-10-01T00:00:00", "updated_at": "2026-10-01T00:00:00",
            }))
            .unwrap()
        })
        .collect();
    documents.insert_many(batch).await.expect("seed the big catalog");
    let big = server.enqueue(&bob, "pool-big").await;
    let big_run = server.run_until(&bob, big["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(big_run["state"], "SUCCEEDED", "400 ctx calls cover 3500 videos: {big_run}");
    let big_result = server.post("/jobs/result", Some(&bob), &json!({"id": big["run"]["id"]})).await;
    assert_eq!(big_result["output"]["pool"], 1000, "the pool keeps the original's top 1000: {big_result}");
    let raw_pool: String = redis::cmd("GET").arg("facereview:reco:pool").query_async(&mut redis).await.unwrap();
    let big_pool: Vec<Value> = serde_json::from_str(&raw_pool).unwrap();
    assert_eq!(big_pool.len(), 1000);
    // With the cache warm the lists answer for any catalog the job could rank. Cold, a request has 64 ctx calls (50 videos and 100 ids
    // per call), so it fails loudly instead of ranking a part of the catalog.
    let warm = ask(&server, "Video.homeCategory", "alice", &json!({})).await;
    assert_eq!(warm["ok"], true, "{warm}");
    let _: i64 = redis::cmd("DEL").arg(json_keys(&mut redis).await).query_async(&mut redis).await.unwrap();
    let cold = ask(&server, "Video.homeCategory", "alice", &json!({})).await;
    assert_eq!(cold["ok"], false, "a cold cache cannot be rebuilt inside one request for 3500 videos: {cold}");
    let refill = server.enqueue(&bob, "pool-big-refill").await;
    let refill_run = server.run_until(&bob, refill["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(refill_run["state"], "SUCCEEDED", "{refill_run}");
    let raw_pool: String = redis::cmd("GET").arg("facereview:reco:pool").query_async(&mut redis).await.unwrap();
    drop(server);

    // ---- Phase 3: the default limit of 64 ctx calls refuses the same catalog loudly, and the old cache stays. ----
    write_config(None, false);
    let server = start_pool_server(&env, &file, &config).await;
    let limited = server.enqueue(&bob, "pool-limited").await;
    let limited_run = server.run_until(&bob, limited["run"]["id"].as_str().unwrap(), finished).await;
    assert_eq!(limited_run["state"], "FAILED", "{limited_run}");
    let still: String = redis::cmd("GET").arg("facereview:reco:pool").query_async(&mut redis).await.unwrap();
    assert_eq!(still, raw_pool, "a failed run leaves the previous cache untouched");
    drop(server);

    // ---- Phase 4: the schedule runs the job as the service principal. ----
    setup.query_drop(format!("DELETE FROM `{SOURCE}`.video WHERE youtube_url LIKE 'big%'")).await.unwrap();
    documents
        .delete_many(mongodb::bson::doc! {"video_id": {"$in": big_ids.iter().map(|(video_id, _)| video_id.clone()).collect::<Vec<_>>()}})
        .await
        .unwrap();
    let _: i64 = redis::cmd("DEL").arg(json_keys(&mut redis).await).query_async(&mut redis).await.unwrap();
    write_config(None, true);
    let server = start_pool_server(&env, &file, &config).await;
    let mut written = false;
    for _ in 0..300 {
        if json_keys(&mut redis).await.len() == 3 {
            written = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(written, "the schedule slot ran the job and filled the cache");
    // The cache write commits before the run settles, so the run row can still be RUNNING here.
    let mut slot_runs = 0;
    for _ in 0..100 {
        slot_runs = setup
            .query_first::<i64, _>(format!("SELECT COUNT(*) FROM `{META}`.aip_jobs WHERE operation='rebuildRecommendationPool' AND idempotency_key LIKE 'schedule:pool:%' AND state='SUCCEEDED'"))
            .await
            .unwrap()
            .unwrap();
        if slot_runs >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(slot_runs >= 1, "the run is the schedule's, under its slot key");
    replay(&server, "scheduled cache").await;
    drop(server);

    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
    let _: i64 = redis::cmd("DEL").arg(json_keys(&mut redis).await).query_async(&mut redis).await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
    }
    setup.query_drop("DROP USER 'aip_t_pool_rt'@'%'").await.unwrap();
}
