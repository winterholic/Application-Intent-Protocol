#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.3 and the first write slice of 7.2.2/7.2.4: the real FaceReview MySQL schema (11 tables as the original SQLAlchemy models emit them, pinned revision
//! new-facereview@7948b7e7) adopted without business DDL and read through the real `aip service` binary. The original schema has no
//! DB defaults, so adoption must refuse `default database` fields until the DBA step in `db-defaults.sql` is applied. The expected
//! rows were first compared with the original app's own SQLAlchemy queries (product/tests/fixtures/facereview-mysql/README.md).
//!
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up`; the adoption account is the container admin.
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
const META: &str = "aip_t_fr_meta";
const SOURCE: &str = "aip_t_fr_src";
const ISSUER: &str = "https://issuer.example";
const ALICE: &str = "11111111-1111-4111-8111-111111111111";
const BOB: &str = "22222222-2222-4222-8222-222222222222";
const CAT_VIDEO: &str = "aaaaaaaa-0000-4000-8000-000000000001";

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
    command.env("AIP_T_FR_ADOPT_URL", &env.adopt_url).env("AIP_T_FR_RUNTIME_URL", &env.runtime_url);
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
    let count = tokio::time::timeout(Duration::from_secs(15), reader.read_line(&mut line)).await.expect("startup deadline").expect("read startup");
    if count == 0 {
        let output = child.wait_with_output().await.unwrap();
        return Err(serde_json::from_slice(&output.stderr).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&output.stderr)})));
    }
    tokio::spawn(async move { while reader.read_line(&mut String::new()).await.is_ok_and(|count| count > 0) {} });
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
    tokio::time::timeout(Duration::from_secs(10), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
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

fn ids(response: &Value) -> Vec<String> {
    assert_eq!(response["ok"], true, "{response}");
    response["rows"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap().to_string()).collect()
}

async fn run_script(conn: &mut aip_migrate::mysql_driver::mysql_async::Conn, script: &str) {
    for chunk in script.split(";\n") {
        // Comment lines at the top of a chunk belong to the statement that follows them.
        let statement: String = chunk.lines().filter(|line| !line.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
        if statement.trim().is_empty() {
            continue;
        }
        conn.query_drop(&statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn facereview_schema_is_adopted_without_business_ddl_and_read_with_original_semantics() {
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
    // The dump lists comment before video; the original DDL order comes from the ORM, so loading is done with checks off.
    setup.query_drop("SET FOREIGN_KEY_CHECKS=0").await.unwrap();
    run_script(&mut setup, SCHEMA).await;
    run_script(&mut setup, SEED).await;
    setup.query_drop("DROP USER IF EXISTS 'aip_t_fr_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_fr_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    for schema in [SOURCE, META] {
        setup
            .query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_fr_rt'@'%'"))
            .await
            .unwrap();
    }
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env { adopt_url: admin_url.clone(), runtime_url: format!("mysql://aip_t_fr_rt:runtime_test_only@{host}/{SOURCE}") };

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
            "database_url_env": "AIP_T_FR_RUNTIME_URL", "migrate_database_url_env": "AIP_T_FR_ADOPT_URL",
            "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
            "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
        })
        .to_string(),
    )
    .unwrap();
    let file_arg = file.to_str().unwrap();

    // Without the DBA defaults the `default database` fields have nothing to stand on: adoption refuses, creating nothing.
    let refused = run(&env, &["adopt-external", file_arg], &config).await.unwrap_err();
    assert_eq!(refused["code"], "EXTERNAL_SCHEMA", "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("default database"), "{refused}");

    run_script(&mut setup, DEFAULTS).await;
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan after the DBA defaults");
    assert_eq!(plan["businessDDL"], json!([]), "adoption changes no business table: {plan}");
    let digest = plan["digest"].as_str().unwrap().to_string();
    run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    for (subject, actor) in [("alice", ALICE), ("bob", BOB)] {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &config).await.expect("bind");
    }
    let unknown =
        run(&env, &["principal", "bind", "--subject", "ghost", "--actor", "99999999-0000-4000-8000-000000000000"], &config).await.unwrap_err();
    assert_eq!(unknown["code"], "BAD_PRINCIPAL", "{unknown}");

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts on the adopted deployment");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
    let (alice, bob) = (token(&private, "alice"), token(&private, "bob"));
    let read = |token: Option<&str>, query: Value| {
        let (address, fingerprint, token) = (address.clone(), fingerprint.clone(), token.map(str::to_string));
        async move { post(&address, "/read", token.as_deref(), &fingerprint, &json!({"query": query})).await }
    };
    let newest = json!([{"field": "created_at", "dir": "desc"}]);

    // Same questions the original HomeService asks, newest first, soft-deleted and NULL-flag videos excluded.
    let all = read(None, json!({"read": "Video", "select": ["id"], "sort": newest})).await;
    assert_eq!(ids(&all), vec!["aaaaaaaa-0000-4000-8000-000000000002", CAT_VIDEO], "{all}");
    let by_title =
        read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "title", "op": "contains", "value": "고양이"}], "sort": newest}))
            .await;
    assert_eq!(ids(&by_title), vec!["aaaaaaaa-0000-4000-8000-000000000002", CAT_VIDEO], "{by_title}");
    let by_category = read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "category", "op": "eq", "value": "cook"}]})).await;
    assert_eq!(ids(&by_category), vec!["aaaaaaaa-0000-4000-8000-000000000002"], "{by_category}");
    let paged = read(None, json!({"read": "Video", "select": ["id"], "sort": newest, "limit": 1, "offset": 1})).await;
    assert_eq!(ids(&paged), vec![CAT_VIDEO], "{paged}");
    let detail = read(
        None,
        json!({"read": "Video", "select": ["id", "title", "channel_name", "category", "duration", "view_count", "created_at"],
        "filter": [{"field": "id", "op": "eq", "value": CAT_VIDEO}]}),
    )
    .await;
    let row = &detail["rows"][0];
    assert_eq!(
        (row["category"].clone(), row["channel_name"].clone(), row["duration"].clone()),
        (json!("animal"), json!("냥채널"), json!(212)),
        "{detail}"
    );
    assert_eq!(row["created_at"], "2026-10-03T10:00:00+00:00", "TIMESTAMP is read as UTC, as the original stored it: {detail}");

    // Comments of a visible video: the soft-deleted comment and the comment of the deleted video are not rows.
    let comments = read(
        None,
        json!({"read": "Comment", "select": ["id", "content"], "sort": [{"field": "created_at", "dir": "asc"}],
        "filter": [{"field": "video", "op": "eq", "value": CAT_VIDEO}]}),
    )
    .await;
    assert_eq!(ids(&comments), vec!["cccccccc-0000-4000-8000-000000000001", "cccccccc-0000-4000-8000-000000000002"], "{comments}");

    // Owner-only tables: the caller's own rows, nothing for anonymous callers or for another user's rows.
    let likes = |token: Option<String>| {
        let read = &read;
        async move { read(token.as_deref(), json!({"read": "VideoLike", "select": ["id"]})).await }
    };
    assert_eq!(ids(&likes(Some(alice.clone())).await), vec!["dddddddd-0000-4000-8000-000000000001"]);
    assert_eq!(ids(&likes(Some(bob.clone())).await), vec!["dddddddd-0000-4000-8000-000000000002"]);
    assert_eq!(ids(&likes(None).await), Vec::<String>::new());
    let points =
        read(Some(&alice), json!({"read": "UserPointHistory", "select": ["id", "amount"], "sort": [{"field": "created_at", "dir": "asc"}]})).await;
    assert_eq!(points["rows"][1]["amount"], json!(-5), "negative point amounts survive: {points}");
    let me = read(Some(&alice), json!({"read": "User", "select": ["id", "email", "role", "profile_image_id", "is_tutorial_done"]})).await;
    assert_eq!(ids(&me), vec![ALICE], "{me}");
    assert_eq!((me["rows"][0]["role"].clone(), me["rows"][0]["is_tutorial_done"].clone()), (json!("GENERAL"), json!(true)), "{me}");

    // The password hash is not a declared field, so it cannot be selected.
    let leak = read(Some(&alice), json!({"read": "User", "select": ["id", "password"]})).await;
    assert_ne!(leak["ok"], true, "{leak}");
    // An offset past the declared bound is refused rather than clamped.
    let deep = read(None, json!({"read": "Video", "select": ["id"], "limit": 1, "offset": 10_000_000})).await;
    assert_eq!(deep["code"], "WIRE.OFFSET_EXCEEDED", "{deep}");

    // Writes the original app performs in Python: the service fills the id, the DB defaults fill flags and UTC timestamps.
    let apply = |token: Option<String>, key: &'static str, request: Value| {
        let (address, fingerprint) = (address.clone(), fingerprint.clone());
        async move { post(&address, "/apply", token.as_deref(), &fingerprint, &json!({"key": key, "request": request})).await }
    };
    let create = |token: Option<String>, key: &'static str, resource: &'static str, values: Value| {
        apply(token, key, json!({"atomic": [{"create": resource, "values": values}]}))
    };
    let made = create(Some(alice.clone()), "fr-c1", "Comment", json!({"video": CAT_VIDEO, "user": ALICE, "content": "새 댓글"})).await;
    assert_eq!(made["ok"], true, "{made}");
    let comment_id = made["created"][0]["id"].as_str().unwrap().to_string();
    assert_eq!((comment_id.len(), comment_id.as_bytes()[14]), (36, b'4'), "a version 4 UUID, as the original's uuid4: {comment_id}");
    let stored: Option<(i8, i8, String)> =
        setup.query_first(format!("SELECT is_modified, is_deleted, user_id FROM comment WHERE comment_id = '{comment_id}'")).await.unwrap();
    assert_eq!(stored, Some((0, 0, ALICE.to_string())), "DB defaults filled the flags");
    let drift: Option<i64> = setup
        .query_first(format!("SELECT ABS(TIMESTAMPDIFF(SECOND, created_at, UTC_TIMESTAMP())) FROM comment WHERE comment_id = '{comment_id}'"))
        .await
        .unwrap();
    assert!(drift.is_some_and(|seconds| seconds < 120), "created_at is the UTC now the original stored: {drift:?}");
    // Not the caller's identity, a deleted video, or no identity at all: refused, and no row.
    for (key, token, user, video) in [
        ("fr-c2", Some(alice.clone()), BOB, CAT_VIDEO),
        ("fr-c3", Some(alice.clone()), ALICE, "aaaaaaaa-0000-4000-8000-000000000003"),
        ("fr-c4", None, ALICE, CAT_VIDEO),
    ] {
        let refused = create(token, key, "Comment", json!({"video": video, "user": user, "content": "거절"})).await;
        assert_eq!(refused["code"], "WIRE.MISSING_TARGET", "{key}: {refused}");
    }
    let refused_rows: Option<i64> = setup.query_first("SELECT COUNT(*) FROM comment WHERE content = '거절'").await.unwrap();
    assert_eq!(refused_rows, Some(0));
    let theirs = apply(
        Some(bob.clone()),
        "fr-e1",
        json!({"apply": "Comment.edit", "target": {"ids": [comment_id.clone()]}, "values": {"content": "남의 수정"}}),
    )
    .await;
    assert_ne!(theirs["ok"], true, "{theirs}");
    let edited = apply(
        Some(alice.clone()),
        "fr-e2",
        json!({"apply": "Comment.edit", "target": {"ids": [comment_id.clone()]}, "values": {"content": "수정됨"}}),
    )
    .await;
    assert_eq!(edited["ok"], true, "{edited}");
    let after: Option<(i8, String)> =
        setup.query_first(format!("SELECT is_modified, content FROM comment WHERE comment_id = '{comment_id}'")).await.unwrap();
    assert_eq!(after, Some((1, "수정됨".to_string())));
    let deleted = apply(Some(alice.clone()), "fr-d1", json!({"apply": "Comment.delete", "target": {"ids": [comment_id.clone()]}})).await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    let kept: Option<i8> = setup.query_first(format!("SELECT is_deleted FROM comment WHERE comment_id = '{comment_id}'")).await.unwrap();
    assert_eq!(kept, Some(1), "the original soft-deletes: the row stays, flagged");
    let again = apply(Some(alice.clone()), "fr-d2", json!({"apply": "Comment.delete", "target": {"ids": [comment_id.clone()]}})).await;
    assert_ne!(again["ok"], true, "{again}");
    // The unique (user, video) index is a definite answer, never a server error; cancelling a like (a physical delete) is not expressible.
    let like = create(Some(alice.clone()), "fr-l1", "VideoLike", json!({"video": CAT_VIDEO, "user": ALICE})).await;
    assert_eq!(like["code"], "WIRE.ALREADY_EXISTS", "alice already liked this video in the seed: {like}");
    let request = create(
        Some(alice.clone()),
        "fr-r1",
        "VideoRequest",
        json!({"user": ALICE, "youtube_url": "newreq00001", "youtube_full_url": "https://www.youtube.com/watch?v=newreq00001"}),
    )
    .await;
    assert_eq!(request["ok"], true, "{request}");
    let status: Option<String> = setup.query_first("SELECT status FROM video_request WHERE youtube_url = 'newreq00001'").await.unwrap();
    assert_eq!(status.as_deref(), Some("PENDING"), "DB default");

    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop("DROP USER IF EXISTS 'aip_t_fr_rt'@'%'").await.unwrap();
}
