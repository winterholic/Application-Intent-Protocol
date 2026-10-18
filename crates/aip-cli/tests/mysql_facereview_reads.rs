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
const CAROL: &str = "33333333-3333-4333-8333-333333333333";
const DAVE: &str = "44444444-4444-4444-8444-444444444444";
const ERIN: &str = "55555555-5555-4555-8555-555555555555";
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
    for (subject, actor) in [("alice", ALICE), ("bob", BOB), ("carol", CAROL), ("dave", DAVE), ("erin", ERIN)] {
        run(&env, &["principal", "bind", "--subject", subject, "--actor", actor], &config).await.expect("bind");
    }
    let unknown =
        run(&env, &["principal", "bind", "--subject", "ghost", "--actor", "99999999-0000-4000-8000-000000000000"], &config).await.unwrap_err();
    assert_eq!(unknown["code"], "BAD_PRINCIPAL", "{unknown}");

    let (_child, listening) = start_server(&env, &file, &config).await.expect("serve starts on the adopted deployment");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
    let (alice, bob) = (token(&private, "alice"), token(&private, "bob"));
    let (carol, dave) = (token(&private, "carol"), token(&private, "dave"));
    let erin = token(&private, "erin");
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

    // WBS 7.2.3: Single logical OR search (title OR channel_name) as in original HomeService.get_search_videos(keyword_type='all')
    // 1) Title-only match in OR:
    let or_title_only = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "요리"},
            {"field": "channel_name", "op": "contains", "value": "없는채널"}
        ]}], "sort": newest}),
    )
    .await;
    assert_eq!(ids(&or_title_only), vec!["aaaaaaaa-0000-4000-8000-000000000002"], "{or_title_only}");

    // 2) Channel-only match in OR:
    let or_channel_only = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "없는제목"},
            {"field": "channel_name", "op": "contains", "value": "냥채널"}
        ]}], "sort": newest}),
    )
    .await;
    assert_eq!(ids(&or_channel_only), vec![CAT_VIDEO], "{or_channel_only}");

    // 3) Both match and overlap: global sort (created_at desc) and deduplication preserved without disjoint set artifact
    let or_both = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "고양이"},
            {"field": "channel_name", "op": "contains", "value": "쿡채널"}
        ]}], "sort": newest}),
    )
    .await;
    assert_eq!(ids(&or_both), vec!["aaaaaaaa-0000-4000-8000-000000000002", CAT_VIDEO], "{or_both}");

    // 4) Neither matches (empty result):
    let or_none = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "없는제목"},
            {"field": "channel_name", "op": "contains", "value": "없는채널"}
        ]}], "sort": newest}),
    )
    .await;
    assert_eq!(ids(&or_none), Vec::<String>::new(), "{or_none}");

    // 5) Pagination on OR search (page 1, page 2, empty trailing page):
    let or_p1 = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "고양이"},
            {"field": "channel_name", "op": "contains", "value": "쿡채널"}
        ]}], "sort": newest, "limit": 1, "offset": 0}),
    )
    .await;
    assert_eq!(ids(&or_p1), vec!["aaaaaaaa-0000-4000-8000-000000000002"], "{or_p1}");

    let or_p2 = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "고양이"},
            {"field": "channel_name", "op": "contains", "value": "쿡채널"}
        ]}], "sort": newest, "limit": 1, "offset": 1}),
    )
    .await;
    assert_eq!(ids(&or_p2), vec![CAT_VIDEO], "{or_p2}");

    let or_p3 = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "고양이"},
            {"field": "channel_name", "op": "contains", "value": "쿡채널"}
        ]}], "sort": newest, "limit": 1, "offset": 2}),
    )
    .await;
    assert_eq!(ids(&or_p3), Vec::<String>::new(), "{or_p3}");

    // 6) Combination of AND filter and OR filter:
    let and_or = read(
        None,
        json!({"read": "Video", "select": ["id"], "filter": [
            {"field": "category", "op": "eq", "value": "cook"},
            {"or": [
                {"field": "title", "op": "contains", "value": "고양이"},
                {"field": "channel_name", "op": "contains", "value": "쿡채널"}
            ]}
        ], "sort": newest}),
    )
    .await;
    assert_eq!(ids(&and_or), vec!["aaaaaaaa-0000-4000-8000-000000000002"], "{and_or}");

    // WBS 7.2.3: Video total count aggregate over the same policy and live MySQL rows
    let total_guest = read(None, json!({"aggregate": "Video.totalCount", "input": {}})).await;
    assert_eq!(total_guest["ok"], true, "{total_guest}");
    assert_eq!(total_guest["rows"][0], 2, "2 active videos in seed: {total_guest}");

    let total_member = read(Some(&alice), json!({"aggregate": "Video.totalCount", "input": {}})).await;
    assert_eq!(total_member["ok"], true, "{total_member}");
    assert_eq!(total_member["rows"][0], 2, "active member sees the same total: {total_member}");

    // Filtered Video.totalCount matching original HomeService.get_search_videos count contract:
    // 1) Title-only in OR:
    let tot_title = read(
        None,
        json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "요리"},
            {"field": "channel_name", "op": "contains", "value": "없는채널"}
        ]}]}),
    )
    .await;
    assert_eq!(tot_title["rows"][0], 1, "filtered total matches read row count: {tot_title}");

    // 2) Channel-only in OR:
    let tot_channel = read(
        None,
        json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "없는제목"},
            {"field": "channel_name", "op": "contains", "value": "냥채널"}
        ]}]}),
    )
    .await;
    assert_eq!(tot_channel["rows"][0], 1, "filtered total matches read row count: {tot_channel}");

    // 3) Both match in OR:
    let tot_both = read(
        None,
        json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "고양이"},
            {"field": "channel_name", "op": "contains", "value": "쿡채널"}
        ]}]}),
    )
    .await;
    assert_eq!(tot_both["rows"][0], 2, "filtered total matches read row count: {tot_both}");

    // 4) Neither matches in OR:
    let tot_none = read(
        None,
        json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "없는제목"},
            {"field": "channel_name", "op": "contains", "value": "없는채널"}
        ]}]}),
    )
    .await;
    assert_eq!(tot_none["rows"][0], 0, "filtered total is 0: {tot_none}");

    // 5) AND + OR combined in totalCount:
    let tot_and_or = read(
        None,
        json!({"aggregate": "Video.totalCount", "input": {}, "filter": [
            {"field": "category", "op": "eq", "value": "cook"},
            {"or": [
                {"field": "title", "op": "contains", "value": "고양이"},
                {"field": "channel_name", "op": "contains", "value": "쿡채널"}
            ]}
        ]}),
    )
    .await;
    assert_eq!(tot_and_or["rows"][0], 1, "AND+OR total matches read row count: {tot_and_or}");

    // 6) Pagination has_next check: page 1 (limit 1, offset 0) -> offset + limit < total (0 + 1 < 2 => true)
    let total_val = tot_both["rows"][0].as_i64().unwrap();
    let has_next_p1 = (0 + 1) < total_val;
    assert!(has_next_p1, "page 1 has next page");
    // page 2 (limit 1, offset 1) -> offset + limit < total (1 + 1 < 2 => false)
    let has_next_p2 = (1 + 1) < total_val;
    assert!(!has_next_p2, "page 2 is last page (no next page)");

    // 7) Wildcard and empty search string semantics (MySQL LIKE '%kw%'):
    // Empty search string ("") matches all active non-deleted videos:
    let read_empty = read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "title", "op": "contains", "value": ""}], "sort": newest})).await;
    assert_eq!(ids(&read_empty), vec!["aaaaaaaa-0000-4000-8000-000000000002", CAT_VIDEO], "{read_empty}");
    let tot_empty = read(None, json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"field": "title", "op": "contains", "value": ""}]})).await;
    assert_eq!(tot_empty["rows"][0], 2, "{tot_empty}");

    // Wildcard '%' matches all active non-deleted videos:
    let read_pct = read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "title", "op": "contains", "value": "%"}], "sort": newest})).await;
    assert_eq!(ids(&read_pct), vec!["aaaaaaaa-0000-4000-8000-000000000002", CAT_VIDEO], "{read_pct}");
    let tot_pct = read(None, json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"field": "title", "op": "contains", "value": "%"}]})).await;
    assert_eq!(tot_pct["rows"][0], 2, "{tot_pct}");

    // Wildcard '_' matches non-empty characters:
    let read_under = read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "title", "op": "contains", "value": "_"}], "sort": newest})).await;
    assert_eq!(ids(&read_under), vec!["aaaaaaaa-0000-4000-8000-000000000002", CAT_VIDEO], "{read_under}");
    let tot_under = read(None, json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"field": "title", "op": "contains", "value": "_"}]})).await;
    assert_eq!(tot_under["rows"][0], 2, "{tot_under}");

    // Deleted videos (is_deleted = 1, e.g. aaaaaaaa-0000-4000-8000-000000000003) and NULL flag videos (aaaaaaaa-0000-4000-8000-000000000004)
    // are NEVER returned or counted by %, _, or empty string:
    for res_ids in [&ids(&read_empty), &ids(&read_pct), &ids(&read_under)] {
        assert!(!res_ids.contains(&"aaaaaaaa-0000-4000-8000-000000000003".to_string()));
        assert!(!res_ids.contains(&"aaaaaaaa-0000-4000-8000-000000000004".to_string()));
    }

    // 8) Case-insensitivity verification (MySQL utf8mb4_0900_ai_ci collation):
    setup.query_drop("INSERT INTO video (video_id, youtube_url, title, channel_name, category, duration, view_count, is_deleted, created_at, updated_at) VALUES ('aaaaaaaa-0000-4000-8000-000000000099', 'engtestvid', 'English Cat Video', 'CatChannel', 'animal', 100, 1, 0, '2026-10-07 10:00:00', '2026-10-07 10:00:00')").await.unwrap();
    let read_upper = read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "title", "op": "contains", "value": "CAT"}]})).await;
    let read_lower = read(None, json!({"read": "Video", "select": ["id"], "filter": [{"field": "title", "op": "contains", "value": "cat"}]})).await;
    assert_eq!(ids(&read_upper), vec!["aaaaaaaa-0000-4000-8000-000000000099"], "{read_upper}");
    assert_eq!(ids(&read_lower), vec!["aaaaaaaa-0000-4000-8000-000000000099"], "{read_lower}");
    let tot_upper = read(None, json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"field": "title", "op": "contains", "value": "CAT"}]})).await;
    let tot_lower = read(None, json!({"aggregate": "Video.totalCount", "input": {}, "filter": [{"field": "title", "op": "contains", "value": "cat"}]})).await;
    assert_eq!(tot_upper["rows"][0], 1, "{tot_upper}");
    assert_eq!(tot_lower["rows"][0], 1, "{tot_lower}");
    setup.query_drop("DELETE FROM video WHERE video_id = 'aaaaaaaa-0000-4000-8000-000000000099'").await.unwrap();
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
    // The unique (user, video) index is a definite answer, never a server error.
    let like = create(Some(alice.clone()), "fr-l1", "VideoLike", json!({"video": CAT_VIDEO, "user": ALICE})).await;
    assert_eq!(like["code"], "WIRE.ALREADY_EXISTS", "alice already liked this video in the seed: {like}");
    // WBS 7.2.4: 좋아요 취소 (물리 삭제) 및 재등록 (토글 완성)
    let alice_like_id: Option<String> =
        setup.query_first(format!("SELECT video_like_id FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert!(alice_like_id.is_some(), "alice has a seeded like");
    let alice_like_id = alice_like_id.unwrap();
    let unlike = apply(Some(alice.clone()), "fr-ul1", json!({"apply": "VideoLike.remove", "target": {"ids": [alice_like_id.clone()]}})).await;
    assert_eq!(unlike["ok"], true, "unlike ok: {unlike}");
    assert_eq!(unlike["tags"], json!(["VideoLike"]), "delete transition exposes its cache invalidation tag");
    let unlike_replay = apply(Some(alice.clone()), "fr-ul1", json!({"apply": "VideoLike.remove", "target": {"ids": [alice_like_id.clone()]}})).await;
    assert_eq!(unlike_replay["replayed"], true, "same key replays the committed remove: {unlike_replay}");
    let unlike_audit: Option<(String, String, String, String, String, String)> = setup
        .query_first(format!(
            "SELECT via, request_key, resource, transition, target_id, JSON_TYPE(after_values) FROM `{META}`.aip_audit WHERE request_key = 'fr-ul1'"
        ))
        .await
        .unwrap();
    assert_eq!(
        unlike_audit,
        Some(("apply".into(), "fr-ul1".into(), "VideoLike".into(), "remove".into(), alice_like_id.clone(), "NULL".into())),
        "audit records the physical delete and null after snapshot"
    );
    let unlike_audit_count: Option<i64> =
        setup.query_first("SELECT COUNT(*) FROM aip_t_fr_meta.aip_audit WHERE request_key = 'fr-ul1'").await.unwrap();
    assert_eq!(unlike_audit_count, Some(1), "idempotent replay does not duplicate the audit record");
    let like_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(like_count, Some(0), "like is physically deleted");
    let alice_likes_after_remove = read(Some(&alice), json!({"read": "VideoLike", "select": ["id"]})).await;
    assert_eq!(ids(&alice_likes_after_remove), Vec::<String>::new(), "service read observes the committed removal");
    let relike = create(Some(alice.clone()), "fr-l2", "VideoLike", json!({"video": CAT_VIDEO, "user": ALICE})).await;
    assert_eq!(relike["ok"], true, "re-like ok after remove: {relike}");
    let relike_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(relike_count, Some(1), "re-like leaves exactly one row");
    let bob_like_remove =
        apply(Some(alice.clone()), "fr-ul-other", json!({"apply": "VideoLike.remove", "target": {"ids": ["dddddddd-0000-4000-8000-000000000002"]}}))
            .await;
    assert_ne!(bob_like_remove["ok"], true, "a caller cannot remove another user's like: {bob_like_remove}");
    let anonymous_like_remove =
        apply(None, "fr-ul-anon", json!({"apply": "VideoLike.remove", "target": {"ids": ["dddddddd-0000-4000-8000-000000000002"]}})).await;
    assert_ne!(anonymous_like_remove["ok"], true, "anonymous like remove is refused: {anonymous_like_remove}");
    let bob_like_count: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{BOB}' AND video_id = 'aaaaaaaa-0000-4000-8000-000000000002'"))
        .await
        .unwrap();
    assert_eq!(bob_like_count, Some(1), "refused removals preserve the other user's like");
    let racing_like_id: Option<String> =
        setup.query_first(format!("SELECT video_like_id FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    let racing_like_id = racing_like_id.expect("the re-created Alice like is present");
    let racing_request = json!({"apply": "VideoLike.remove", "target": {"ids": [racing_like_id]}});
    let (race_a, race_b) =
        tokio::join!(apply(Some(alice.clone()), "fr-ul-race", racing_request.clone()), apply(Some(alice.clone()), "fr-ul-race", racing_request),);
    assert_eq!(
        (race_a["ok"].clone(), race_b["ok"].clone()),
        (json!(true), json!(true)),
        "concurrent same-key removals both resolve: {race_a} {race_b}"
    );
    let replay_count = [race_a["replayed"] == true, race_b["replayed"] == true].into_iter().filter(|replayed| *replayed).count();
    assert_eq!(replay_count, 1, "one concurrent request executes; the other replays");
    let race_like_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    let race_audit_count: Option<i64> =
        setup.query_first("SELECT COUNT(*) FROM aip_t_fr_meta.aip_audit WHERE request_key = 'fr-ul-race'").await.unwrap();
    assert_eq!((race_like_count, race_audit_count), (Some(0), Some(1)), "concurrent deletion leaves no row and one audit");

    let distinct_key_like = create(Some(alice.clone()), "fr-ul-distinct-seed", "VideoLike", json!({"video": CAT_VIDEO, "user": ALICE})).await;
    assert_eq!(distinct_key_like["ok"], true, "seed distinct-key race target: {distinct_key_like}");
    let distinct_key_id: Option<String> =
        setup.query_first(format!("SELECT video_like_id FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    let distinct_key_id = distinct_key_id.expect("distinct-key race target exists");
    let distinct_key_request = json!({"apply": "VideoLike.remove", "target": {"ids": [distinct_key_id]}});
    let (distinct_a, distinct_b) = tokio::join!(
        apply(Some(alice.clone()), "fr-ul-distinct-a", distinct_key_request.clone()),
        apply(Some(alice.clone()), "fr-ul-distinct-b", distinct_key_request),
    );
    assert_eq!(
        [distinct_a["ok"] == true, distinct_b["ok"] == true].into_iter().filter(|ok| *ok).count(),
        1,
        "distinct-key concurrent removals execute exactly once: {distinct_a} {distinct_b}"
    );
    let distinct_race_like_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{ALICE}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    let distinct_race_audit_count: Option<i64> = setup
        .query_first("SELECT COUNT(*) FROM aip_t_fr_meta.aip_audit WHERE request_key IN ('fr-ul-distinct-a', 'fr-ul-distinct-b')")
        .await
        .unwrap();
    assert_eq!((distinct_race_like_count, distinct_race_audit_count), (Some(0), Some(1)), "distinct keys leave one physical delete and one audit");

    // Both toggle paths exercise create -> physical remove -> zero rows -> create -> one row.
    let carol_like = create(Some(carol.clone()), "fr-lc1", "VideoLike", json!({"video": CAT_VIDEO, "user": CAROL})).await;
    assert_eq!(carol_like["ok"], true, "create Carol's like: {carol_like}");
    let carol_like_id: Option<String> =
        setup.query_first(format!("SELECT video_like_id FROM video_like WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    let carol_like_id = carol_like_id.expect("Carol like was seeded by create");
    let carol_unlike = apply(Some(carol.clone()), "fr-lc2", json!({"apply": "VideoLike.remove", "target": {"ids": [carol_like_id]}})).await;
    assert_eq!(carol_unlike["ok"], true, "remove Carol's like: {carol_unlike}");
    let carol_like_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(carol_like_count, Some(0));
    let carol_likes_after_remove = read(Some(&carol), json!({"read": "VideoLike", "select": ["id"]})).await;
    assert_eq!(ids(&carol_likes_after_remove), Vec::<String>::new(), "service read observes the committed removal");
    let carol_relike = create(Some(carol.clone()), "fr-lc3", "VideoLike", json!({"video": CAT_VIDEO, "user": CAROL})).await;
    assert_eq!(carol_relike["ok"], true, "recreate Carol's like: {carol_relike}");
    let carol_like_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(carol_like_count, Some(1));
    setup.query_drop(format!("DELETE FROM video_like WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    setup.query_drop(format!("INSERT INTO video_like (video_like_id, video_id, user_id, created_at) VALUES ('eeeeeeee-0000-4000-8000-000000000012', 'aaaaaaaa-0000-4000-8000-000000000002', '{CAROL}', NOW())")).await.unwrap();
    let carol_withdraw_like_seed: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_withdraw_like_seed, Some(1), "withdraw starts with one explicitly seeded like");

    let carol_bookmark = create(Some(carol.clone()), "fr-bc1", "VideoBookmark", json!({"video": CAT_VIDEO, "user": CAROL})).await;
    assert_eq!(carol_bookmark["ok"], true, "create Carol's bookmark: {carol_bookmark}");
    let carol_bookmark_id: Option<String> = setup
        .query_first(format!("SELECT video_bookmark_id FROM video_bookmark WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'"))
        .await
        .unwrap();
    let carol_bookmark_id = carol_bookmark_id.expect("Carol bookmark was seeded by create");
    let carol_unbookmark =
        apply(Some(carol.clone()), "fr-bc2", json!({"apply": "VideoBookmark.remove", "target": {"ids": [carol_bookmark_id]}})).await;
    assert_eq!(carol_unbookmark["ok"], true, "remove Carol's bookmark: {carol_unbookmark}");
    let carol_bookmark_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_bookmark WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(carol_bookmark_count, Some(0));
    let carol_bookmarks_after_remove = read(Some(&carol), json!({"read": "VideoBookmark", "select": ["id"]})).await;
    assert_eq!(ids(&carol_bookmarks_after_remove), Vec::<String>::new(), "service read observes the committed removal");
    let carol_rebookmark = create(Some(carol.clone()), "fr-bc3", "VideoBookmark", json!({"video": CAT_VIDEO, "user": CAROL})).await;
    assert_eq!(carol_rebookmark["ok"], true, "recreate Carol's bookmark: {carol_rebookmark}");
    let carol_bookmark_count: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_bookmark WHERE user_id = '{CAROL}' AND video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(carol_bookmark_count, Some(1));
    let bob_bookmark =
        create(Some(bob.clone()), "fr-bb1", "VideoBookmark", json!({"video": "aaaaaaaa-0000-4000-8000-000000000002", "user": BOB})).await;
    assert_eq!(bob_bookmark["ok"], true, "seed another user's bookmark for authorization checks: {bob_bookmark}");
    let bob_bookmark_id: Option<String> = setup
        .query_first(format!(
            "SELECT video_bookmark_id FROM video_bookmark WHERE user_id = '{BOB}' AND video_id = 'aaaaaaaa-0000-4000-8000-000000000002'"
        ))
        .await
        .unwrap();
    let bob_bookmark_id = bob_bookmark_id.expect("Bob bookmark exists");
    let other_bookmark_remove =
        apply(Some(alice.clone()), "fr-bb2", json!({"apply": "VideoBookmark.remove", "target": {"ids": [bob_bookmark_id.clone()]}})).await;
    assert_ne!(other_bookmark_remove["ok"], true, "a caller cannot remove another user's bookmark: {other_bookmark_remove}");
    let anonymous_bookmark_remove = apply(None, "fr-bb3", json!({"apply": "VideoBookmark.remove", "target": {"ids": [bob_bookmark_id]}})).await;
    assert_ne!(anonymous_bookmark_remove["ok"], true, "anonymous bookmark remove is refused: {anonymous_bookmark_remove}");
    let bob_bookmark_count: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM video_bookmark WHERE user_id = '{BOB}' AND video_id = 'aaaaaaaa-0000-4000-8000-000000000002'"))
        .await
        .unwrap();
    assert_eq!(bob_bookmark_count, Some(1), "refused removals preserve the other user's bookmark");
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

    // Roles of the original admin_required / super_admin_required, judged from the caller's own row.
    let everyone = read(Some(&bob), json!({"read": "User", "select": ["id"]})).await;
    assert_eq!(ids(&everyone).len(), 5, "an ADMIN lists every user: {everyone}");
    assert_eq!(ids(&read(Some(&alice), json!({"read": "User", "select": ["id"]})).await), vec![ALICE], "a member lists herself only");
    let deactivate = |token: &str, key: &'static str, target: &'static str| {
        apply(Some(token.to_string()), key, json!({"apply": "User.deactivate", "target": {"ids": [target]}}))
    };
    assert_ne!(deactivate(&alice, "fr-a1", DAVE).await["ok"], true, "a member cannot deactivate");
    assert_ne!(deactivate(&bob, "fr-a2", ERIN).await["ok"], true, "an ADMIN cannot deactivate another ADMIN");
    let still: Option<i8> = setup.query_first(format!("SELECT is_deleted FROM user WHERE user_id = '{ERIN}'")).await.unwrap();
    assert_eq!(still, Some(0));

    // A failed child audit insert must roll back all earlier withdrawal effects in this MySQL transaction.
    setup.query_drop(format!("INSERT INTO video_like (video_like_id, video_id, user_id, created_at) VALUES ('eeeeeeee-0000-4000-8000-000000000013', '{CAT_VIDEO}', '{DAVE}', NOW())")).await.unwrap();
    setup.query_drop(format!("CREATE TRIGGER `{META}`.reject_child_delete_audit BEFORE INSERT ON `{META}`.aip_audit FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='injected child audit failure'")).await.unwrap();
    let audit_failure = apply(Some(dave.clone()), "fr-w-audit-failure", json!({"apply": "User.withdraw", "target": {"ids": [DAVE]}})).await;
    assert_ne!(audit_failure["ok"], true, "child audit insertion failure rejects withdrawal: {audit_failure}");
    let failure_snapshot: Option<(i8, i64, i64)> = setup.query_first(format!(
        "SELECT (SELECT is_deleted FROM user WHERE user_id='{DAVE}'), (SELECT COUNT(*) FROM video_like WHERE video_like_id='eeeeeeee-0000-4000-8000-000000000013'), (SELECT COUNT(*) FROM `{META}`.aip_audit WHERE request_key='fr-w-audit-failure')"
    )).await.unwrap();
    println!("MYSQL_SNAPSHOT child_audit_insert_failure: {failure_snapshot:?}");
    assert_eq!(failure_snapshot, Some((0, 1, 0)), "audit failure rolls back parent mutation, child delete, and all audit inserts");
    setup.query_drop(format!("DROP TRIGGER `{META}`.reject_child_delete_audit")).await.unwrap();
    setup.query_drop("DELETE FROM video_like WHERE video_like_id='eeeeeeee-0000-4000-8000-000000000013'").await.unwrap();

    // Erin has no delete-effect child rows: withdrawal records only its parent transition.
    let zero_child = apply(Some(erin.clone()), "fr-w-zero-child", json!({"apply": "User.withdraw", "target": {"ids": [ERIN]}})).await;
    assert_eq!(zero_child["ok"], true, "zero-child withdrawal succeeds: {zero_child}");
    let zero_child_audit: Option<(i64, i64)> = setup.query_first(format!(
        "SELECT (SELECT COUNT(*) FROM `{META}`.aip_audit WHERE request_key='fr-w-zero-child' AND resource='User'), (SELECT COUNT(*) FROM `{META}`.aip_audit WHERE request_key='fr-w-zero-child' AND transition='withdraw:delete')"
    )).await.unwrap();
    println!("MYSQL_SNAPSHOT zero_child_audit: {zero_child_audit:?}");
    assert_eq!(zero_child_audit, Some((1, 0)));

    // maxRows+1 must roll back the earlier Comment update and child-table deletes together with the User state.
    let overflow_comment = "eeeeeeee-0000-4000-8000-000000000001";
    setup.query_drop(format!("INSERT INTO comment (comment_id, video_id, user_id, content, is_modified, is_deleted, created_at, updated_at) VALUES ('{overflow_comment}', '{CAT_VIDEO}', '{DAVE}', 'wbs724 rollback probe', 0, 0, NOW(), NOW())")).await.unwrap();
    setup.query_drop(format!("INSERT INTO video_like (video_like_id, video_id, user_id, created_at) VALUES ('eeeeeeee-0000-4000-8000-000000000002', 'aaaaaaaa-0000-4000-8000-000000000002', '{DAVE}', NOW())")).await.unwrap();
    setup.query_drop(format!("INSERT INTO video_view_log (video_view_log_id, video_id, user_id, created_at, updated_at) VALUES ('eeeeeeee-0000-4000-8000-000000000003', '{CAT_VIDEO}', '{DAVE}', NOW(), NOW())")).await.unwrap();
    setup.query_drop(format!("INSERT INTO video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, created_at, updated_at) VALUES ('eeeeeeee-0000-4000-8000-000000000004', '{DAVE}', 'wbs724overflw', 'https://www.youtube.com/watch?v=wbs724overflw', 'PENDING', NOW(), NOW())")).await.unwrap();
    setup.query_drop(format!("INSERT INTO user_favorite_genre (user_favorite_genre_id, user_id, genre, created_at, updated_at) VALUES ('eeeeeeee-0000-4000-8000-000000000005', '{DAVE}', 'etc', NOW(), NOW())")).await.unwrap();
    setup.query_drop(format!("INSERT INTO user_point_history (user_point_history_id, user_id, video_id, amount, description, created_at, updated_at) SELECT UUID(), '{DAVE}', '{CAT_VIDEO}', 1, 'wbs724-overflow', NOW(), NOW() FROM information_schema.columns LIMIT 1001")).await.unwrap();
    let overflow_rows: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM user_point_history WHERE user_id = '{DAVE}' AND description = 'wbs724-overflow'"))
        .await
        .unwrap();
    assert_eq!(overflow_rows, Some(1001), "the test creates maxRows+1 child rows");
    let overflow = apply(Some(dave.clone()), "fr-w-overflow", json!({"apply": "User.withdraw", "target": {"ids": [DAVE]}})).await;
    assert_ne!(overflow["ok"], true, "withdraw over maxRows is refused: {overflow}");
    let overflow_audit: Option<i64> =
        setup.query_first("SELECT COUNT(*) FROM aip_t_fr_meta.aip_audit WHERE request_key = 'fr-w-overflow'").await.unwrap();
    let overflow_idempotency: Option<i64> =
        setup.query_first("SELECT COUNT(*) FROM aip_t_fr_meta.aip_idem WHERE idem_key = 'fr-w-overflow'").await.unwrap();
    assert_eq!((overflow_audit, overflow_idempotency), (Some(0), Some(0)), "failed atomic withdrawal leaves no audit or idempotency record");
    let dave_state: Option<i8> = setup.query_first(format!("SELECT is_deleted FROM user WHERE user_id = '{DAVE}'")).await.unwrap();
    assert_eq!(dave_state, Some(0), "parent update rolled back");
    let rollback_comment: Option<i8> =
        setup.query_first(format!("SELECT is_deleted FROM comment WHERE comment_id = '{overflow_comment}'")).await.unwrap();
    assert_eq!(rollback_comment, Some(0), "comment soft delete rolled back");
    let rollback_like: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE video_like_id = 'eeeeeeee-0000-4000-8000-000000000002'")).await.unwrap();
    let rollback_log: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM video_view_log WHERE video_view_log_id = 'eeeeeeee-0000-4000-8000-000000000003'"))
        .await
        .unwrap();
    let rollback_request: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM video_request WHERE video_request_id = 'eeeeeeee-0000-4000-8000-000000000004'"))
        .await
        .unwrap();
    let rollback_genre: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM user_favorite_genre WHERE user_favorite_genre_id = 'eeeeeeee-0000-4000-8000-000000000005'"))
        .await
        .unwrap();
    assert_eq!(
        (rollback_like, rollback_log, rollback_request, rollback_genre),
        (Some(1), Some(1), Some(1), Some(1)),
        "prior child deletes rolled back"
    );
    setup.query_drop(format!("DELETE FROM comment WHERE comment_id = '{overflow_comment}'")).await.unwrap();
    for id in [
        "eeeeeeee-0000-4000-8000-000000000002",
        "eeeeeeee-0000-4000-8000-000000000003",
        "eeeeeeee-0000-4000-8000-000000000004",
        "eeeeeeee-0000-4000-8000-000000000005",
    ] {
        let (table, field) = match id {
            "eeeeeeee-0000-4000-8000-000000000002" => ("video_like", "video_like_id"),
            "eeeeeeee-0000-4000-8000-000000000003" => ("video_view_log", "video_view_log_id"),
            "eeeeeeee-0000-4000-8000-000000000004" => ("video_request", "video_request_id"),
            _ => ("user_favorite_genre", "user_favorite_genre_id"),
        };
        setup.query_drop(format!("DELETE FROM `{table}` WHERE `{field}` = '{id}'")).await.unwrap();
    }
    setup.query_drop(format!("DELETE FROM user_point_history WHERE user_id = '{DAVE}' AND description = 'wbs724-overflow'")).await.unwrap();

    assert_eq!(deactivate(&bob, "fr-a3", DAVE).await["ok"], true, "an ADMIN deactivates a GENERAL member");
    let hidden: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM comment WHERE user_id = '{DAVE}' AND is_deleted = 0")).await.unwrap();
    assert_eq!(hidden, Some(0), "the member's comments are hidden in the same transaction");
    assert_eq!(deactivate(&carol, "fr-a4", ERIN).await["ok"], true, "a SUPER_ADMIN deactivates an ADMIN");
    let change = |token: &str, key: &'static str, target: &'static str, role: &'static str| {
        apply(Some(token.to_string()), key, json!({"apply": "User.changeRole", "target": {"ids": [target]}, "values": {"role": role}}))
    };
    assert_ne!(change(&bob, "fr-a5", ALICE, "ADMIN").await["ok"], true, "only a SUPER_ADMIN changes roles");
    assert_ne!(change(&carol, "fr-a6", DAVE, "ADMIN").await["ok"], true, "a deactivated member is not found");
    assert_ne!(change(&carol, "fr-a7", ALICE, "ROOT").await["ok"], true, "a role outside the enum");
    assert_eq!(change(&carol, "fr-a8", ALICE, "ADMIN").await["ok"], true);
    let role: Option<String> = setup.query_first(format!("SELECT role FROM user WHERE user_id = '{ALICE}'")).await.unwrap();
    assert_eq!(role.as_deref(), Some("ADMIN"));
    assert_eq!(change(&carol, "fr-a9", ALICE, "GENERAL").await["ok"], true);
    // Video requests: an ADMIN reads and rejects a pending one, once. (alice's own request was created above, so the seed one is used.)
    let pending = "ffffffff-0000-4000-8000-000000000001";
    let reject = |token: &str, key: &'static str| {
        apply(
            Some(token.to_string()),
            key,
            json!({"apply": "VideoRequest.reject", "target": {"ids": [pending]}, "values": {"admin_comment": "중복 요청"}}),
        )
    };
    assert_ne!(reject(&alice, "fr-q1").await["ok"], true, "a member cannot reject");
    assert_eq!(reject(&bob, "fr-q2").await["ok"], true);
    let stored: Option<(String, String)> =
        setup.query_first(format!("SELECT status, admin_comment FROM video_request WHERE video_request_id = '{pending}'")).await.unwrap();
    assert_eq!(stored, Some(("REJECTED".to_string(), "중복 요청".to_string())));
    assert_ne!(reject(&bob, "fr-q3").await["ok"], true, "a processed request cannot be rejected again");
    // Soft deletes by an ADMIN keep the rows; public reads stop showing them.
    let remove = |token: &str, key: &'static str, name: &'static str, target: &'static str| {
        apply(Some(token.to_string()), key, json!({"apply": name, "target": {"ids": [target]}}))
    };
    assert_ne!(remove(&alice, "fr-x1", "Video.remove", CAT_VIDEO).await["ok"], true, "a member cannot remove a video");
    assert_eq!(remove(&bob, "fr-x2", "Video.remove", CAT_VIDEO).await["ok"], true);
    let listed = read(None, json!({"read": "Video", "select": ["id"]})).await;
    assert!(!ids(&listed).contains(&CAT_VIDEO.to_string()), "{listed}");
    let kept: Option<i8> = setup.query_first(format!("SELECT is_deleted FROM video WHERE video_id = '{CAT_VIDEO}'")).await.unwrap();
    assert_eq!(kept, Some(1));
    // A deactivated member is no longer a valid caller (the original's login_required filters is_deleted = False).
    let gone = read(Some(&dave), json!({"read": "User", "select": ["id"]})).await;
    assert_eq!(gone["rows"], json!([]), "{gone}");
    let write = create(
        Some(dave.clone()),
        "fr-z1",
        "Comment",
        json!({"video": "aaaaaaaa-0000-4000-8000-000000000002", "user": DAVE, "content": "복귀 시도"}),
    )
    .await;
    assert_ne!(write["ok"], true, "{write}");
    let none: Option<i64> = setup.query_first("SELECT COUNT(*) FROM comment WHERE content = '복귀 시도'").await.unwrap();
    assert_eq!(none, Some(0));

    // WBS 7.2.4: 회원 본인 탈퇴 (User.withdraw) - 5개 테이블 물리 삭제 + 댓글/회원 soft delete + bookmark 남김
    setup.query_drop(format!(
        "INSERT INTO video_view_log (video_view_log_id, video_id, user_id, created_at, updated_at) VALUES (UUID(), 'aaaaaaaa-0000-4000-8000-000000000002', '{CAROL}', NOW(), NOW())"
    )).await.unwrap();
    setup.query_drop(format!(
        "INSERT INTO video_request (video_request_id, user_id, youtube_url, youtube_full_url, status, created_at, updated_at) VALUES (UUID(), '{CAROL}', 'carolwithdraw1', 'https://www.youtube.com/watch?v=carolwithdraw1', 'PENDING', NOW(), NOW())"
    )).await.unwrap();
    setup.query_drop(format!(
        "INSERT INTO comment (comment_id, video_id, user_id, content, is_modified, is_deleted, created_at, updated_at) VALUES ('eeeeeeee-0000-4000-8000-000000000011', 'aaaaaaaa-0000-4000-8000-000000000002', '{CAROL}', 'carol withdraw soft-delete probe', 0, 0, NOW(), NOW())"
    )).await.unwrap();
    setup.query_drop(format!(
        "INSERT INTO user_favorite_genre (user_favorite_genre_id, user_id, genre, created_at, updated_at) VALUES (UUID(), '{CAROL}', 'drama', NOW(), NOW())"
    )).await.unwrap();
    setup.query_drop(format!(
        "INSERT INTO user_point_history (user_point_history_id, user_id, video_id, amount, created_at, updated_at) VALUES (UUID(), '{CAROL}', 'aaaaaaaa-0000-4000-8000-000000000002', 100, NOW(), NOW())"
    )).await.unwrap();
    setup.query_drop(format!(
        "INSERT INTO video_bookmark (video_bookmark_id, video_id, user_id, created_at) VALUES (UUID(), 'aaaaaaaa-0000-4000-8000-000000000002', '{CAROL}', NOW())"
    )).await.unwrap();

    let other_withdraw = apply(Some(alice.clone()), "fr-w-other", json!({"apply": "User.withdraw", "target": {"ids": [CAROL]}})).await;
    assert_ne!(other_withdraw["ok"], true, "one member cannot withdraw another member: {other_withdraw}");
    let anonymous_withdraw = apply(None, "fr-w-anon", json!({"apply": "User.withdraw", "target": {"ids": [CAROL]}})).await;
    assert_ne!(anonymous_withdraw["ok"], true, "anonymous withdrawal is refused: {anonymous_withdraw}");
    let carol_still_active: Option<i8> = setup.query_first(format!("SELECT is_deleted FROM user WHERE user_id = '{CAROL}'")).await.unwrap();
    let carol_like_still_seeded: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!((carol_still_active, carol_like_still_seeded), (Some(0), Some(1)), "refused withdrawal preserves parent and child rows");

    let withdraw = apply(Some(carol.clone()), "fr-w1", json!({"apply": "User.withdraw", "target": {"ids": [CAROL]}})).await;
    assert_eq!(withdraw["ok"], true, "carol withdraw ok: {withdraw}");
    assert_eq!(
        withdraw["tags"],
        json!(["Comment", "User", "UserFavoriteGenre", "UserPointHistory", "VideoLike", "VideoRequest", "VideoViewLog"]),
        "withdraw advertises the parent and every affected resource tag"
    );
    let withdrawal_parent_audit: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM `{META}`.aip_audit WHERE request_key = 'fr-w1' AND resource = 'User' AND transition = 'withdraw' AND target_id = '{CAROL}'")).await.unwrap();
    assert_eq!(withdrawal_parent_audit, Some(1), "withdraw audit retains the parent User transition");
    let withdrawal_child_audit: Option<i64> = setup.query_first(format!(
        "SELECT COUNT(*) FROM `{META}`.aip_audit WHERE request_key = 'fr-w1' AND actor_id = (SELECT actor_id FROM `{META}`.aip_audit WHERE request_key='fr-w1' AND resource='User' AND transition='withdraw') AND via = 'apply' AND transition = 'withdraw:delete' AND after_values = CAST('null' AS JSON) AND resource IN ('VideoLike','VideoViewLog','VideoRequest','UserFavoriteGenre','UserPointHistory')"
    )).await.unwrap();
    assert_eq!(withdrawal_child_audit, Some(5), "each deleted child gets a same-request null-after audit row");
    let exact_child_audit_snapshots: Option<i64> = setup.query_first(format!(
        "SELECT COUNT(*) FROM `{META}`.aip_audit WHERE request_key='fr-w1' AND actor_id=(SELECT actor_id FROM `{META}`.aip_audit WHERE request_key='fr-w1' AND resource='User' AND transition='withdraw') AND via='apply' AND transition='withdraw:delete' AND after_values=CAST('null' AS JSON) AND JSON_UNQUOTE(JSON_EXTRACT(before_values,'$.id'))=target_id AND JSON_UNQUOTE(JSON_EXTRACT(before_values,'$.user'))='{CAROL}' AND JSON_LENGTH(before_values)=CASE resource WHEN 'VideoLike' THEN 4 WHEN 'VideoViewLog' THEN 6 WHEN 'VideoRequest' THEN 8 WHEN 'UserFavoriteGenre' THEN 5 WHEN 'UserPointHistory' THEN 8 END"
    )).await.unwrap();
    assert_eq!(exact_child_audit_snapshots, Some(5), "each before snapshot has its deleted row ID, owner, and every declared field");
    let child_audit_rows: Vec<(String, String, String, String)> = setup.query(format!(
        "SELECT resource,target_id,CAST(before_values AS CHAR),CAST(after_values AS CHAR) FROM `{META}`.aip_audit WHERE request_key='fr-w1' AND transition='withdraw:delete' ORDER BY resource,target_id"
    )).await.unwrap();
    println!("MYSQL_AUDIT child_effect_exact_snapshots={child_audit_rows:?}");
    let carol_deleted: Option<i8> = setup.query_first(format!("SELECT is_deleted FROM user WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_deleted, Some(1), "user is soft deleted");
    let carol_likes: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM video_like WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_likes, Some(0), "video_like physically deleted");
    let carol_logs: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM video_view_log WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_logs, Some(0), "video_view_log physically deleted");
    let guest_log_preserved: Option<i64> = setup.query_first(
        "SELECT COUNT(*) FROM video_view_log WHERE video_view_log_id='99999999-0000-4000-8000-000000000002' AND user_id IS NULL AND guest_token='guest-xyz'"
    ).await.unwrap();
    assert_eq!(guest_log_preserved, Some(1), "the unrelated guest row with a NULL user FK is preserved");
    let carol_genres: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM user_favorite_genre WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_genres, Some(0), "user_favorite_genre physically deleted");
    let carol_points: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM user_point_history WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_points, Some(0), "user_point_history physically deleted");
    let carol_requests: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM video_request WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_requests, Some(0), "video_request physically deleted");
    let carol_comment: Option<(i8, String)> =
        setup.query_first("SELECT is_deleted, content FROM comment WHERE comment_id = 'eeeeeeee-0000-4000-8000-000000000011'").await.unwrap();
    assert_eq!(carol_comment, Some((1, "carol withdraw soft-delete probe".to_string())), "active comment is soft-deleted and row remains");
    let carol_bookmarks: Option<i64> = setup.query_first(format!("SELECT COUNT(*) FROM video_bookmark WHERE user_id = '{CAROL}'")).await.unwrap();
    assert_eq!(carol_bookmarks, Some(2), "both the toggle bookmark and separately seeded bookmark remain (parity baseline)");
    let seeded_bookmark: Option<i64> = setup
        .query_first(format!("SELECT COUNT(*) FROM video_bookmark WHERE user_id = '{CAROL}' AND video_id = 'aaaaaaaa-0000-4000-8000-000000000002'"))
        .await
        .unwrap();
    assert_eq!(seeded_bookmark, Some(1), "explicitly seeded bookmark is preserved after withdrawal");

    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop("DROP USER IF EXISTS 'aip_t_fr_rt'@'%'").await.unwrap();
}
