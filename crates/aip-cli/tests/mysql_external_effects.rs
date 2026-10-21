#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.5: a write extension that writes to MongoDB and Redis next to the MySQL transaction, through the real `aip service` binary,
//! the Python worker sandbox, MySQL 8.4, MongoDB and Redis. It tests the saga of the original (`common/saga`): effects first, a way back
//! for each, the relational commit last, and what is undone when the commit does not happen.
//!
//! What it proves (fixture: `product/tests/fixtures/external-effects`):
//!  * success: the relational change, the Mongo upsert, the Redis dedupe key and a `completed` saga log;
//!  * a replayed request (same key) and a replayed frame (same log id, new key) change nothing a second time;
//!  * an extension that fails after its effects, and a relational check that fails, undo the Mongo and Redis effects (`compensated`);
//!  * a Mongo failure after the Redis effect undoes the Redis effect; with Mongo unreachable the saga log cannot start, so no effect happens;
//!  * Redis unreachable fails before any other effect and recovers by itself;
//!  * a commit whose answer is lost keeps the effects and the log says `unknown`.
//!
//! Run with `-- --ignored` against run-owned containers: `AIP_TEST_MYSQL_URL` (MySQL 8.4 admin), `AIP_TEST_MONGO_URL`, `AIP_TEST_REDIS_URL`.
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
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
};

const DEFINITION: &str = include_str!("../../../product/tests/fixtures/external-effects/effects.aip");
const MAPPING: &str = include_str!("../../../product/tests/fixtures/external-effects/mapping.json");
const SCHEMA: &str = include_str!("../../../product/tests/fixtures/external-effects/schema.sql");
const SEED: &str = include_str!("../../../product/tests/fixtures/external-effects/seed.sql");
const EXTENSION: &str = include_str!("../../../product/tests/fixtures/external-effects/extensions/effects.py");
const META: &str = "aip_t_eff_meta";
const SOURCE: &str = "aip_t_eff_src";
const MONGO_DB: &str = "aip_t_eff_fr";
const ISSUER: &str = "https://issuer.example";
const ALICE: &str = "11111111-1111-4111-8111-111111111111";
const BOB: &str = "22222222-2222-4222-8222-222222222222";
const ONE: &str = "bbbbbbbb-0000-4000-8000-000000000001";
const TWO: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const FULL: &str = "bbbbbbbb-0000-4000-8000-000000000003";

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
        .env("AIP_T_EFF_ADOPT_URL", &env.adopt_url)
        .env("AIP_T_EFF_RUNTIME_URL", &env.runtime_url)
        .env("AIP_T_EFF_MONGO_URL", &env.mongo_url)
        .env("AIP_T_EFF_REDIS_URL", &env.redis_url);
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
    let mut socket = TcpStream::connect(address).await.expect("connect");
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

/// A TCP forwarder that can be switched off: while `down`, new connections are closed and open ones are dropped on their next bytes.
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

/// A MySQL forwarder that can drop the answer to the next COMMIT after the server has committed (the client then cannot know).
struct AckProxy {
    address: SocketAddr,
    armed: Arc<Mutex<bool>>,
}

impl AckProxy {
    async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let armed: Arc<Mutex<bool>> = Arc::default();
        let shared = armed.clone();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else { return };
                let shared = shared.clone();
                tokio::spawn(async move {
                    let Ok(server) = TcpStream::connect(target).await else { return };
                    let (mut client_read, mut client_write) = client.into_split();
                    let (mut server_read, mut server_write) = server.into_split();
                    let drop_ack = Arc::new(AtomicBool::new(false));
                    let flag = drop_ack.clone();
                    let forward = async move {
                        let mut buf = vec![0u8; 65536];
                        while let Ok(n) = client_read.read(&mut buf).await {
                            if n == 0 {
                                return;
                            }
                            // COM_QUERY (0x03) with the text COMMIT: after the server has it, its answer is swallowed.
                            let commit = n > 5 && buf[4] == 0x03 && String::from_utf8_lossy(&buf[5..n]).trim().eq_ignore_ascii_case("COMMIT");
                            if commit {
                                let mut armed = shared.lock().unwrap();
                                if *armed {
                                    *armed = false;
                                    flag.store(true, Ordering::SeqCst);
                                }
                            }
                            if server_write.write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                    };
                    let backward = async move {
                        let mut buf = vec![0u8; 65536];
                        while let Ok(n) = server_read.read(&mut buf).await {
                            if n == 0 || drop_ack.load(Ordering::SeqCst) || client_write.write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                    };
                    tokio::select! { _ = forward => {}, _ = backward => {} }
                });
            }
        });
        Self { address, armed }
    }

    fn arm(&self) {
        *self.armed.lock().unwrap() = true;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs MySQL 8.4 (AIP_TEST_MYSQL_URL), MongoDB (AIP_TEST_MONGO_URL), Redis (AIP_TEST_REDIS_URL) and a worker sandbox; run with -- --ignored"]
async fn write_extension_effects_commit_or_compensate_with_the_relational_transaction() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    let mongo_url = std::env::var("AIP_TEST_MONGO_URL").expect("set AIP_TEST_MONGO_URL to a run-owned MongoDB");
    let redis_url = std::env::var("AIP_TEST_REDIS_URL").expect("set AIP_TEST_REDIS_URL to a run-owned Redis");
    for (name, value) in [("MongoDB", &mongo_url), ("Redis", &redis_url)] {
        let parsed = url::Url::parse(value).expect("fixture URL");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"), "the {name} fixture must bind loopback only");
        assert!(parsed.port().is_some_and(|port| port > 1024 && port != 27017 && port != 6379), "the {name} fixture must use a dedicated high port");
    }

    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("effects.aip");
    std::fs::write(&file, DEFINITION).unwrap();
    let extensions = directory.path().join("extensions");
    std::fs::create_dir(&extensions).unwrap();
    std::fs::write(extensions.join("effects.py"), EXTENSION).unwrap();

    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}` DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci")).await.unwrap();
    setup.query_drop(format!("USE `{SOURCE}`")).await.unwrap();
    run_script(&mut setup, SCHEMA).await;
    run_script(&mut setup, SEED).await;
    setup.query_drop("DROP USER IF EXISTS 'aip_t_eff_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_eff_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_eff_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let mysql_proxy = AckProxy::start(host.parse().expect("MySQL socket address")).await;
    let mongo_proxy = SwitchProxy::start(socket_of(&mongo_url)).await;
    let redis_proxy = SwitchProxy::start(socket_of(&redis_url)).await;
    let env = Env {
        adopt_url: admin_url.clone(),
        runtime_url: format!("mysql://aip_t_eff_rt:runtime_test_only@{}/{SOURCE}", mysql_proxy.address),
        mongo_url: via(&mongo_url, mongo_proxy.address),
        redis_url: via(&redis_url, redis_proxy.address),
    };

    let mongo = mongodb::Client::with_uri_str(&mongo_url).await.expect("MongoDB fixture");
    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
    let sessions = mongo.database(MONGO_DB).collection::<mongodb::bson::Document>("youtube_watching_data");
    let saga = mongo.database(MONGO_DB).collection::<mongodb::bson::Document>("saga_transaction_log");
    let redis = redis::Client::open(redis_url.as_str()).expect("Redis fixture");
    let mut redis = redis.get_multiplexed_async_connection().await.expect("Redis connection");
    let keys: Vec<String> = redis::cmd("KEYS").arg("effects:*").query_async(&mut redis).await.unwrap();
    for key in keys {
        let _: () = redis::cmd("DEL").arg(key).query_async(&mut redis).await.unwrap();
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
    std::fs::write(
        &config,
        json!({
            "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
            "database_url_env": "AIP_T_EFF_RUNTIME_URL", "migrate_database_url_env": "AIP_T_EFF_ADOPT_URL",
            "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
            "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
            "workers": {
                "directory": "extensions", "language": "python", "enable_writes": true,
                "mongo": {"uri_env": "AIP_T_EFF_MONGO_URL", "database": MONGO_DB, "max_pool_size": 4, "timeout_ms": 3000,
                          "write_collections": ["youtube_watching_data"], "ensure_indexes": true},
                "redis": {"uri_env": "AIP_T_EFF_REDIS_URL", "prefixes": ["effects:"], "max_in_flight": 8, "timeout_ms": 3000},
            },
        })
        .to_string(),
    )
    .unwrap();
    let file_arg = file.to_str().unwrap();
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    for (subject, actor) in [("alice", ALICE), ("bob", BOB)] {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &config).await.expect("bind");
    }

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts with Mongo and Redis write adapters");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
    let alice = token(&private, "alice");
    let track = |key: &str, subject_token: &str, video: &str, log: &str, scenario: &str| {
        let body = json!({"key": key, "request": {"extension": "Video.track", "input": {"video_id": video, "log_id": log, "scenario": scenario}}});
        let (address, fingerprint, bearer) = (address.clone(), fingerprint.clone(), subject_token.to_string());
        async move { post(&address, "/apply", Some(&bearer), &fingerprint, &body).await }
    };
    let views = |video: &'static str| {
        let admin = &admin;
        async move {
            let mut conn = admin.pool().get_conn().await.unwrap();
            conn.query_first::<i64, _>(format!("SELECT view_count FROM `{SOURCE}`.`video` WHERE video_id = '{video}'")).await.unwrap().unwrap()
        }
    };
    let frames = |log: &'static str| {
        let sessions = sessions.clone();
        async move { sessions.find_one(mongodb::bson::doc! {"video_view_log_id": log}).await.unwrap().map(|doc| doc.get_i32("frame_count").unwrap()) }
    };
    let redis_probe = redis.clone();
    let exists = |key: &str| {
        let mut connection = redis_probe.clone();
        let key = key.to_string();
        async move { redis::cmd("EXISTS").arg(key).query_async::<bool>(&mut connection).await.unwrap() }
    };
    let log_of = |status: &'static str| {
        let saga = saga.clone();
        async move { saga.find(mongodb::bson::doc! {"status": status}).await.unwrap() }
    };
    let counter = |mut connection: redis::aio::MultiplexedConnection| async move {
        redis::cmd("GET").arg("effects:counter:requests").query_async::<Option<i64>>(&mut connection).await.unwrap().unwrap_or(0)
    };

    // 1. Success: the relational change, the Mongo upsert, the dedupe key and a completed saga log.
    let done = track("eff-1", &alice, ONE, "log-1", "ok").await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["output"], json!({"counted": true, "frames": 1}));
    assert_eq!(views(ONE).await, 1);
    assert_eq!(frames("log-1").await, Some(1));
    assert!(exists("effects:dedupe:log-1").await);
    let completed = {
        use futures_util::TryStreamExt;
        log_of("completed").await.try_collect::<Vec<_>>().await.unwrap()
    };
    assert_eq!(completed.len(), 1, "one saga log of the call, completed");
    let steps = completed[0].get_array("steps").unwrap();
    assert_eq!(steps.len(), 3, "dedupe key, Mongo upsert and the counter");
    let step_names: Vec<&str> = steps.iter().map(|step| step.as_document().unwrap().get_str("name").unwrap()).collect();
    assert_eq!(
        step_names,
        ["redis.effects:dedupe:log-1.set_nx_ex", "mongo.youtube_watching_data.find_one_and_update", "redis.effects:counter:requests.incr_expire"]
    );
    assert!(steps.iter().all(|step| step.as_document().unwrap().get_str("status").unwrap() == "completed"));
    assert_eq!(counter(redis.clone()).await, 1);

    // 2. The same request again replays the stored result; the same frame under a new key is dropped by the dedupe key.
    let replay = track("eff-1", &alice, ONE, "log-1", "ok").await;
    assert_eq!(replay["ok"], true, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!((views(ONE).await, frames("log-1").await, counter(redis.clone()).await), (1, Some(1), 1), "a replay changes nothing");
    let duplicate = track("eff-2", &alice, ONE, "log-1", "ok").await;
    assert_eq!(duplicate["ok"], true, "{duplicate}");
    assert_eq!(duplicate["output"], json!({"counted": false, "frames": 0}));
    assert_eq!((views(ONE).await, frames("log-1").await, counter(redis.clone()).await), (1, Some(1), 1), "a duplicate frame counts once");

    // 3. The extension fails after its effects: the relational change is rolled back and so are the Mongo and Redis effects.
    let failed = track("eff-3", &alice, TWO, "log-3", "fail_after").await;
    assert_eq!(failed["ok"], false, "{failed}");
    assert_eq!(views(TWO).await, 0, "the relational change was rolled back");
    assert_eq!(frames("log-3").await, None, "the Mongo upsert was undone");
    assert!(!exists("effects:dedupe:log-3").await, "the dedupe key was undone, so the frame can be sent again");
    let compensated = {
        use futures_util::TryStreamExt;
        log_of("compensated").await.try_collect::<Vec<_>>().await.unwrap()
    };
    assert_eq!(compensated.len(), 1);
    let statuses: Vec<&str> =
        compensated[0].get_array("steps").unwrap().iter().map(|step| step.as_document().unwrap().get_str("status").unwrap()).collect();
    assert_eq!(statuses, ["compensated", "compensated", "completed"], "the counter has no way back and stays");
    assert_eq!(counter(redis.clone()).await, 2, "the counter was incremented by the failed call and is not undone");
    let retried = track("eff-3b", &alice, TWO, "log-3", "ok").await;
    assert_eq!(retried["output"], json!({"counted": true, "frames": 1}), "after the undo the same frame is counted: {retried}");
    assert_eq!(views(TWO).await, 1);

    // 4. A relational check that fails after the effects (the counter is at its limit) undoes them too.
    let blocked = track("eff-4", &alice, FULL, "log-4", "ok").await;
    assert_eq!(blocked["ok"], false, "{blocked}");
    assert_eq!(views(FULL).await, 1000);
    assert_eq!(frames("log-4").await, None);
    assert!(!exists("effects:dedupe:log-4").await);

    // 5. Mongo fails after the Redis effect (a session of another member holds the unique log id): the Redis key is undone.
    sessions
        .insert_one(mongodb::bson::doc! {"video_view_log_id": "log-5", "user_id": BOB, "video_id": ONE})
        .await
        .expect("a session of another member");
    let conflict = track("eff-5", &alice, ONE, "log-5", "ok").await;
    assert_eq!(conflict["ok"], false, "{conflict}");
    assert!(!exists("effects:dedupe:log-5").await, "the Redis effect before the failed Mongo write was undone");
    assert_eq!(views(ONE).await, 1);

    // 6. Mongo unreachable: the saga log cannot start, so no external effect happens and nothing is committed; after recovery the call works.
    mongo_proxy.set_down(true);
    let outage = track("eff-6", &alice, ONE, "log-6", "ok").await;
    assert_eq!(outage["ok"], false, "{outage}");
    assert!(!exists("effects:dedupe:log-6").await, "no effect without a saga log");
    assert_eq!(views(ONE).await, 1);
    mongo_proxy.set_down(false);
    let recovered = {
        let mut result = Value::Null;
        for attempt in 0..20 {
            result = track(&format!("eff-6-{attempt}"), &alice, ONE, "log-6", "ok").await;
            if result["ok"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        result
    };
    assert_eq!(recovered["ok"], true, "{recovered}");
    assert_eq!(recovered["output"], json!({"counted": true, "frames": 1}));
    assert_eq!(views(ONE).await, 2);

    // 6b. Redis unreachable: the first effect fails, nothing reached Mongo or MySQL; after recovery the same frame is counted once.
    redis_proxy.set_down(true);
    let redis_outage = track("eff-6b", &alice, ONE, "log-6b", "ok").await;
    assert_eq!(redis_outage["ok"], false, "{redis_outage}");
    assert_eq!((views(ONE).await, frames("log-6b").await), (2, None), "no partial effect");
    redis_proxy.set_down(false);
    let back = {
        let mut result = Value::Null;
        for attempt in 0..20 {
            result = track(&format!("eff-6b-{attempt}"), &alice, ONE, "log-6b", "ok").await;
            if result["ok"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        result
    };
    assert_eq!(back["output"], json!({"counted": true, "frames": 1}), "{back}");
    assert_eq!((views(ONE).await, frames("log-6b").await), (3, Some(1)));

    // 7. A commit whose answer is lost: MySQL committed, the caller cannot know, so the effects stay and the log says `unknown`.
    mysql_proxy.arm();
    let unknown = track("eff-7", &alice, TWO, "log-7", "ok").await;
    assert_eq!(unknown["ok"], false, "{unknown}");
    assert_eq!(unknown["code"], "WIRE.COMMIT_UNKNOWN", "{unknown}");
    assert_eq!(views(TWO).await, 2, "the transaction did commit");
    assert_eq!(frames("log-7").await, Some(1), "the effects of a possibly committed transaction stay");
    assert!(exists("effects:dedupe:log-7").await);
    let unknown_logs = {
        use futures_util::TryStreamExt;
        log_of("unknown").await.try_collect::<Vec<_>>().await.unwrap()
    };
    assert_eq!(unknown_logs.len(), 1);
    // The same key again settles it: the stored result is returned and nothing is counted twice.
    let settled = track("eff-7", &alice, TWO, "log-7", "ok").await;
    assert_eq!(settled["ok"], true, "{settled}");
    assert_eq!(settled["replayed"], true, "{settled}");
    assert_eq!((views(TWO).await, frames("log-7").await), (2, Some(1)));

    mongo.database(MONGO_DB).drop().await.expect("drop the run-owned database");
}
