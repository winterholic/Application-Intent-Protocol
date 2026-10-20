#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.1.1 through the real `aip service` binary against a real MySQL 8.4: plan and apply an external adoption, bind a
//! user, serve policy reads to an RS256 access token, then drift the business schema and watch the very next request close.
//!
//! The runtime account has DML on the business schema and the AIP schema; the adoption account is the admin of a run-owned container.
//! Run with `-- --ignored` after `scripts/mysql-live-fixture.sh up` (see that script for the environment).
use aip_migrate::mysql_driver::{MysqlDriver, mysql_async::prelude::Queryable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use spike_v2_read::sqlgen;
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const RECRUITMENT: &str = include_str!("../../../spikes/spike-v1-fixture/fixture/recruitment.aip");
const NOTE: &str = "
enum NoteKind { MEMO, TODO }
resource Note {
  fields { id: Id; title: Text(1..100); done: Bool; score: Decimal(10,2); due: Date?; created: Time; kind: NoteKind }
  rows read when true
  expose read {
    select id, title, done, score, due, created, kind
    filter done.eq, score.gte, title.contains
    sort created, score, id
    budget { rows 20; depth 1; deadline 2s; cost 1000 }
  }
  transition finish { from done = false; allow actor != null; to done = true; repeat unchanged }
  expose apply finish { target id; bulk maxRows 5 }
}
";
const META: &str = "aip_t_mysql_cli";
const SOURCE: &str = "aip_t_mysql_cli_src";
const ISSUER: &str = "https://issuer.example";

fn definition() -> String {
    // The adopted resources need a single-column key each: the fixture's two link resources get one.
    let src = RECRUITMENT
        .replacen("fields { club: Club; member: Member; role: ClubRole }", "fields { id: Id; club: Club; member: Member; role: ClubRole }", 1)
        .replacen("fields { recruitment: Recruitment; member: Member }", "fields { id: Id; recruitment: Recruitment; member: Member }", 1)
        // A partial unique index would be a business DDL change; the external adoption refuses it by design.
        .replacen("limit atMostOnePublished on Recruitment = atMost 1 where status = PUBLISHED\n", "", 1)
        .replacen("  invariant atMostOnePublished per club\n", "", 1);
    format!("{src}\n{NOTE}")
}

fn source_ddl(facts: &Value) -> Vec<String> {
    let mut out = vec![];
    for (resource, definition) in facts["resources"].as_object().unwrap() {
        let mut columns = vec![];
        let mut constraints = vec![];
        for (field, fd) in definition["fields"].as_object().unwrap() {
            let ty = fd["ty"].as_str().unwrap();
            let base = ty.trim_end_matches('?');
            let column = sqlgen::column(facts, resource, field).unwrap();
            let sql_ty = if field == "id" {
                "BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY".to_string()
            } else {
                let physical = if let Some(target) = base.strip_prefix("Ref<").and_then(|rest| rest.strip_suffix('>')) {
                    constraints.push(format!(
                        "CONSTRAINT `fk_{}_{column}` FOREIGN KEY (`{column}`) REFERENCES `{}` (`id`)",
                        sqlgen::snake(resource),
                        sqlgen::snake(target)
                    ));
                    "BIGINT"
                } else if base.starts_with("Decimal<") {
                    "DECIMAL(10,2)"
                } else if base.starts_with("Enum<") {
                    "VARCHAR(64)"
                } else {
                    match base {
                        "Text" | "Email" | "Url" => "VARCHAR(255)",
                        "Int" => "BIGINT",
                        "Bool" => "TINYINT(1)",
                        "Time" => "DATETIME(6)",
                        "Date" => "DATE",
                        other => panic!("test DDL has no type for {other}"),
                    }
                };
                format!("{physical}{}", if ty.ends_with('?') { " NULL" } else { " NOT NULL" })
            };
            columns.push(format!("`{column}` {sql_ty}"));
        }
        columns.extend(constraints);
        out.push(format!("CREATE TABLE `{SOURCE}`.`{}` ({}) ENGINE=InnoDB", sqlgen::snake(resource), columns.join(", ")));
    }
    out
}

/// Resources reference each other, so tables are created parents first.
fn ordered(mut statements: Vec<String>) -> Vec<String> {
    let mut done: Vec<String> = vec![];
    let mut out = vec![];
    while !statements.is_empty() {
        let before = statements.len();
        statements.retain(|statement| {
            let mine = statement.split('`').nth(3).unwrap().to_string();
            let parents: Vec<&str> = statement.split("REFERENCES `").skip(1).map(|rest| rest.split('`').next().unwrap()).collect();
            if parents.iter().all(|parent| *parent == mine || done.iter().any(|d| d == parent)) {
                done.push(mine);
                out.push(statement.clone());
                false
            } else {
                true
            }
        });
        assert!(statements.len() < before, "reference cycle in test DDL");
    }
    out
}

const SEED: &[&str] = &[
    "INSERT INTO school (id, name) VALUES (1, 'A대'), (2, 'B대')",
    "INSERT INTO member (id, school_id) VALUES (1, 1), (2, 1), (3, 2)",
    "INSERT INTO club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1), (11, 'Open', NULL, NULL), (12, 'B', NULL, 2)",
    "INSERT INTO club_member (id, club_id, member_id, role) VALUES (1, 10, 1, 'MANAGER'), (2, 10, 2, 'MEMBER')",
    "INSERT INTO recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES \
     (100, 'A 모집', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY), 'PUBLISHED', 0, 10, 'note'), \
     (101, 'Open 모집', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 31 DAY), 'PUBLISHED', 5, 11, NULL), \
     (102, 'B 모집', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 32 DAY), 'PUBLISHED', 2, 12, NULL)",
    "INSERT INTO note (id, title, done, score, due, created, kind) VALUES \
     (1, 'first', 1, 10.50, '2026-10-18', '2026-10-01 09:30:00.123400', 'MEMO'), \
     (2, 'second', 0, 2.25, NULL, '2026-10-02 00:00:00.000000', 'TODO')",
];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

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
    command.env("AIP_T_MYSQL_ADOPT_URL", &env.adopt_url).env("AIP_T_MYSQL_RUNTIME_URL", &env.runtime_url);
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
    // Keep draining stdout so the child never blocks on a full pipe.
    tokio::spawn(async move { while reader.read_line(&mut String::new()).await.is_ok_and(|count| count > 0) {} });
    Ok((child, serde_json::from_str(&line).expect("listening event")))
}

async fn http(address: &str, method: &str, path: &str, token: Option<&str>, fingerprint: &str, body: &Value) -> (u16, Value) {
    let text = if method == "POST" { body.to_string() } else { String::new() };
    let auth = token.map(|token| format!("Authorization: Bearer {token}\r\n")).unwrap_or_default();
    let contract = if fingerprint.is_empty() { String::new() } else { format!("x-aip-contract: {fingerprint}\r\n") };
    let length = if method == "POST" { format!("Content-Length: {}\r\n", text.len()) } else { String::new() };
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect");
    socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}{contract}{length}\r\n{text}").as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(10), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
    let (head, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn token(private: &RsaPrivateKey, subject: &str) -> String {
    let key = private.to_pkcs1_der().unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    let issued = now();
    encode(
        &header,
        &json!({"iss": ISSUER, "aud": "aip-api", "sub": subject, "iat": issued, "nbf": issued, "exp": issued + 300}),
        &EncodingKey::from_rsa_der(key.as_bytes()),
    )
    .unwrap()
}

fn ids(response: &Value) -> Vec<i64> {
    assert_eq!(response["ok"], true, "{response}");
    // The decimal wire carries Ids as strings.
    response["rows"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap().parse().unwrap()).collect()
}

#[tokio::test]
#[ignore = "live: needs a dedicated MySQL 8.4 container; run with -- --ignored"]
async fn mysql_composite_keys_fail_before_introspection_outputs_or_adoption() {
    const SOURCE: &str = "aip_t_mysql_keys_src";
    const META: &str = "aip_t_mysql_keys_meta";
    let url = std::env::var("AIP_TEST_MYSQL_URL").expect("dedicated MySQL fixture URL");
    aip_test_support::validate_mysql_admin_url(&url).unwrap();
    let admin = MysqlDriver::connect(&url).await.unwrap();
    let mut db = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        db.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    db.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
    db.query_drop(format!(
        "CREATE TABLE `{SOURCE}`.parents (id BIGINT PRIMARY KEY, tenant BIGINT NOT NULL, UNIQUE KEY pair_key (id, tenant)) ENGINE=InnoDB"
    ))
    .await
    .unwrap();
    db.query_drop(format!(
        "CREATE TABLE `{SOURCE}`.children (id BIGINT PRIMARY KEY, parent_id BIGINT NOT NULL, tenant BIGINT NOT NULL) ENGINE=InnoDB"
    ))
    .await
    .unwrap();
    db.query_drop(format!("INSERT INTO `{SOURCE}`.parents VALUES (1, 7)")).await.unwrap();
    db.query_drop(format!("INSERT INTO `{SOURCE}`.children VALUES (10, 1, 7)")).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("app.aip");
    // Scalar fields deliberately hide the relationship from the definition. The physical FK must still be checked.
    std::fs::write(&file, "actor principal\nresource Item { fields { id: Id; parentKey: Int; tenant: Int } rows read when true expose read { select id, parentKey, tenant } }\n").unwrap();
    std::fs::write(
        directory.path().join("mapping.json"),
        json!({"schema":SOURCE,"tables":{"Item":{"table":"children","columns":{"id":"id","parentKey":"parent_id","tenant":"tenant"}}}}).to_string(),
    )
    .unwrap();
    let config = directory.path().join("service.json");
    std::fs::write(
        &config,
        json!({
            "schema":META,"database_adapter":"mysql","external_mapping":"mapping.json",
            "database_url_env":"AIP_T_MYSQL_RUNTIME_URL","migrate_database_url_env":"AIP_T_MYSQL_ADOPT_URL",
            "rate_limit":null,"auth":{"issuer":ISSUER,"audience":"aip-api","jwks":{"kind":"file","path":"unused.json"}}
        })
        .to_string(),
    )
    .unwrap();
    let env = Env { adopt_url: url.clone(), runtime_url: url };
    let file = file.to_str().unwrap();
    let good = run(&env, &["adopt-external", file], &config).await.expect("single primary key and scalar columns are supported");
    assert_eq!(good["businessDDL"], json!([]));
    db.query_drop(format!(
        "ALTER TABLE `{SOURCE}`.children ADD CONSTRAINT fk_pair FOREIGN KEY (parent_id, tenant) REFERENCES `{SOURCE}`.parents (id, tenant)"
    ))
    .await
    .unwrap();
    let draft = directory.path().join("draft.aip");
    let mapping = directory.path().join("draft-mapping.json");
    for (code, detail) in [("MYSQL.UNSUPPORTED_COMPOSITE_FK", "fk_pair"), ("MYSQL.UNSUPPORTED_COMPOSITE_PK", "children")] {
        if code.ends_with("_PK") {
            db.query_drop(format!("ALTER TABLE `{SOURCE}`.children DROP FOREIGN KEY fk_pair, DROP PRIMARY KEY, ADD PRIMARY KEY (id, tenant)"))
                .await
                .unwrap();
        }
        for args in [vec!["adopt-external", file], vec!["adopt-external", file, "--apply", good["digest"].as_str().unwrap()]] {
            let error = run(&env, &args, &config).await.expect_err("unsupported physical key must be refused, including scalar-only mappings");
            assert_eq!(error["code"], code, "{error}");
            assert!(error["message"].as_str().unwrap().contains(detail), "{error}");
        }
        let error = run(
            &env,
            &["introspect", "--source-schema", SOURCE, "--out", draft.to_str().unwrap(), "--mapping-out", mapping.to_str().unwrap()],
            &config,
        )
        .await
        .unwrap_err();
        assert_eq!(error["code"], code, "{error}");
        assert!(error["message"].as_str().unwrap().contains(detail), "{error}");
        assert!(!draft.exists() && !mapping.exists(), "refusal cannot leave a partial draft");
        let schemas: u64 = db.exec_first("SELECT COUNT(*) FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?", (META,)).await.unwrap().unwrap();
        assert_eq!(schemas, 0, "rejected adoption cannot create infrastructure");
        let row: (i64, i64, i64) = db.query_first(format!("SELECT id, parent_id, tenant FROM `{SOURCE}`.children")).await.unwrap().unwrap();
        assert_eq!(row, (10, 1, 7), "business data must survive refusals");
        println!("{code}: plan/apply/introspect refused; outputs and metadata absent; business row unchanged");
    }
    db.query_drop(format!("ALTER TABLE `{SOURCE}`.children DROP PRIMARY KEY, ADD PRIMARY KEY (id)")).await.unwrap();
    assert_eq!(run(&env, &["adopt-external", file], &config).await.unwrap()["ok"], true, "removing unsupported keys reopens planning");
    assert_eq!(
        run(&env, &["introspect", "--source-schema", SOURCE, "--out", draft.to_str().unwrap(), "--mapping-out", mapping.to_str().unwrap()], &config)
            .await
            .unwrap()["ok"],
        true
    );
    assert!(draft.exists() && mapping.exists());
    for schema in [META, SOURCE] {
        db.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn external_mysql_adoption_binding_serving_and_drift() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("app.aip");
    std::fs::write(&file, definition()).unwrap();
    let facts = aip_service::load(&file, false).expect("definition loads").output.execution;

    // Business database owned by someone else, a read-only runtime account, and the adoption account (the admin).
    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
    for statement in ordered(source_ddl(&facts)) {
        setup.query_drop(&statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    setup.query_drop(format!("USE `{SOURCE}`")).await.unwrap();
    for statement in SEED {
        setup.query_drop(*statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    setup.query_drop("DROP USER IF EXISTS 'aip_t_cli_rt'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_cli_rt'@'%' IDENTIFIED BY 'runtime_test_only'").await.unwrap();
    // Reads need SELECT; the write path also locks and changes business rows, so it needs DML on them.
    setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE TEMPORARY TABLES ON `{SOURCE}`.* TO 'aip_t_cli_rt'@'%'")).await.unwrap();
    // The AIP schema does not exist yet; the runtime account is granted on it by pattern, as an operator would.
    setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE ON `{META}`.* TO 'aip_t_cli_rt'@'%'")).await.unwrap();
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().to_string();
    let env = Env { adopt_url: admin_url.clone(), runtime_url: format!("mysql://aip_t_cli_rt:runtime_test_only@{host}/{SOURCE}") };

    let mut mapping = json!({"schema": SOURCE, "tables": {}});
    for (resource, definition) in facts["resources"].as_object().unwrap() {
        let columns: serde_json::Map<String, Value> = definition["fields"]
            .as_object()
            .unwrap()
            .keys()
            .map(|field| (field.clone(), json!(sqlgen::column(&facts, resource, field).unwrap())))
            .collect();
        mapping["tables"][resource] = json!({"table": sqlgen::snake(resource), "columns": columns});
    }
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
    let config_value = json!({
        "schema": META, "external_mapping": "mapping.json", "database_adapter": "mysql",
        "database_url_env": "AIP_T_MYSQL_RUNTIME_URL", "migrate_database_url_env": "AIP_T_MYSQL_ADOPT_URL",
        "listen": "127.0.0.1:0", "wire": "decimal", "rate_limit": null,
        "auth": {"issuer": ISSUER, "audience": "aip-api", "jwks": {"kind": "file", "path": "jwks.json"}},
    });
    std::fs::write(&config, config_value.to_string()).unwrap();
    let file_arg = file.to_str().unwrap();

    // Read/write workers are supported by the current MySQL adapter. Adoption planning does not start
    // a worker, and must accept its configuration; file uploads still require unsupported storage.
    let mut with_workers = config_value.clone();
    with_workers["workers"] = json!({"directory": ".", "language": "node"});
    std::fs::write(directory.path().join("workers.json"), with_workers.to_string()).unwrap();
    let worker_plan = run(&env, &["adopt-external", file_arg], &directory.path().join("workers.json")).await.expect("worker adoption plan");
    assert_eq!(worker_plan["ok"], true, "{worker_plan}");
    assert_eq!(worker_plan["businessDDL"], json!([]), "planning leaves the business schema unchanged");
    with_workers["workers"]["enable_file_uploads"] = json!(true);
    std::fs::write(directory.path().join("uploads.json"), with_workers.to_string()).unwrap();
    let refused = run(&env, &["adopt-external", file_arg], &directory.path().join("uploads.json")).await.unwrap_err();
    assert_eq!(refused["code"], "DB_ADAPTER_UNSUPPORTED", "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("workers.enable_file_uploads"), "{refused}");
    // Refusals before anything is created: PostgreSQL-only infrastructure, MariaDB, missing mapping.
    let mut default_limit = config_value.clone();
    default_limit.as_object_mut().unwrap().remove("rate_limit");
    std::fs::write(directory.path().join("limit.json"), default_limit.to_string()).unwrap();
    let refused = run(&env, &["adopt-external", file_arg], &directory.path().join("limit.json")).await.unwrap_err();
    assert!(refused["message"].as_str().unwrap().contains("rate_limit"), "{refused}");
    let mut maria = config_value.clone();
    maria["database_adapter"] = json!("mariadb");
    std::fs::write(directory.path().join("maria.json"), maria.to_string()).unwrap();
    let refused = run(&env, &["adopt-external", file_arg], &directory.path().join("maria.json")).await.unwrap_err();
    assert_eq!(refused["code"], "DB_ADAPTER_UNSUPPORTED", "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("MariaDB"), "{refused}");
    let missing = run(&env, &["serve", file_arg], &config).await.unwrap_err();
    assert!(matches!(missing["code"].as_str(), Some("EXTERNAL_SCHEMA" | "PRINCIPAL_FAILED" | "WIRE.SCHEMA_MISMATCH")), "not adopted yet: {missing}");

    // Plan, then apply exactly that plan.
    let plan = run(&env, &["adopt-external", file_arg], &config).await.expect("plan");
    assert_eq!(plan["mode"], "external", "{plan}");
    assert_eq!(plan["businessDDL"], json!([]));
    let digest = plan["digest"].as_str().unwrap().to_string();
    let wrong = run(&env, &["adopt-external", file_arg, "--apply", &"0".repeat(64)], &config).await.unwrap_err();
    assert_eq!(wrong["code"], "PLAN_CHANGED", "{wrong}");
    let applied = run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("apply");
    assert_eq!(applied["businessDDL"], json!([]), "{applied}");
    let again = run(&env, &["adopt-external", file_arg, "--apply", &digest], &config).await.expect("re-apply");
    assert_eq!(again["alreadyApplied"], true, "{again}");

    // User binding: unknown actor rows are refused, a real one is bound, a different actor cannot take the subject over.
    let missing_actor = run(&env, &["principal", "bind", "--subject", "user-1", "--actor", "999"], &config).await.unwrap_err();
    assert_eq!(missing_actor["code"], "BAD_PRINCIPAL", "{missing_actor}");
    run(&env, &["principal", "bind", "--subject", "user-1", "--actor", "1"], &config).await.expect("bind actor 1");
    run(&env, &["principal", "bind", "--subject", "user-2", "--actor", "2"], &config).await.expect("bind actor 2");
    let takeover = run(&env, &["principal", "bind", "--subject", "user-1", "--actor", "2"], &config).await.unwrap_err();
    assert_eq!(takeover["code"], "PRINCIPAL_CONFLICT", "{takeover}");

    let (child, listening) = start_server(&env, &file, &config).await.expect("serve starts on the adopted deployment");
    assert_eq!(listening["adapter"], "mysql");
    let address = listening["address"].as_str().unwrap().to_string();
    let fingerprint = listening["fingerprint"].as_str().unwrap().to_string();
    let (user1, user2) = (token(&private, "user-1"), token(&private, "user-2"));
    assert_eq!(http(&address, "GET", "/ready", None, "", &Value::Null).await.0, 200);

    let (_, session) = http(&address, "POST", "/session", Some(&user1), "", &json!({})).await;
    assert_eq!(session["principal"]["actorId"], "1", "{session}");
    let recruitments =
        json!({"query": {"read": "Recruitment", "select": ["id", "views", "internalNote"], "sort": [{"field": "views", "dir": "desc"}]}});
    let (_, anonymous) = http(&address, "POST", "/read", None, &fingerprint, &recruitments).await;
    assert_eq!(ids(&anonymous), vec![101], "anonymous sees the school-less club only: {anonymous}");
    let (_, as_manager) = http(&address, "POST", "/read", Some(&user1), &fingerprint, &recruitments).await;
    assert_eq!(ids(&as_manager), vec![101, 100], "{as_manager}");
    assert_eq!(as_manager["rows"][1]["internalNote"], "note", "club manager reads the field");
    let (_, as_member) = http(&address, "POST", "/read", Some(&user2), &fingerprint, &recruitments).await;
    assert_eq!(as_member["rows"][1]["internalNote"], Value::Null, "plain member is redacted");
    let (_, notes) = http(
        &address,
        "POST",
        "/read",
        None,
        &fingerprint,
        &json!({"query": {"read": "Note", "select": ["id", "done", "score", "created"], "sort": [{"field": "id", "dir": "asc"}]}}),
    )
    .await;
    assert_eq!(notes["rows"][0]["done"], true, "{notes}");
    assert_eq!(notes["rows"][0]["score"], "10.50");
    assert_eq!(notes["rows"][0]["created"], "2026-10-01T09:30:00.1234+00:00");

    // A write through the same binary: authenticated by the JWT, applied once, replayed by key, journaled with the audit row.
    let finish = json!({"key": "cli-finish-1", "request": {"apply": "Note.finish", "target": {"ids": ["2"]}}});
    let (_, written) = http(&address, "POST", "/apply", Some(&user1), &fingerprint, &finish).await;
    assert_eq!(written["ok"], true, "{written}");
    assert_eq!(written["changed"], json!(["2"]), "{written}");
    let (_, replayed) = http(&address, "POST", "/apply", Some(&user1), &fingerprint, &finish).await;
    assert_eq!((replayed["ok"].clone(), replayed["replayed"].clone()), (json!(true), json!(true)), "{replayed}");
    let (_, anonymous_write) = http(
        &address,
        "POST",
        "/apply",
        None,
        &fingerprint,
        &json!({"key": "cli-finish-2", "request": {"apply": "Note.finish", "target": {"ids": ["1"]}}}),
    )
    .await;
    assert_eq!(anonymous_write["code"], "WIRE.FORBIDDEN", "an anonymous caller does not pass `actor != null`: {anonymous_write}");
    let done: Option<i64> = setup.query_first(format!("SELECT done FROM `{SOURCE}`.`note` WHERE id = 2")).await.unwrap();
    assert_eq!(done, Some(1));
    let audited: Option<i64> =
        setup.query_first(format!("SELECT COUNT(*) FROM `{META}`.`aip_audit` WHERE request_key = 'cli-finish-1' AND actor_id = 1")).await.unwrap();
    assert_eq!(audited, Some(1), "one audit row, written with the change");

    // What the adapter does not implement is refused by name, and nothing was written.
    let (_, refused) =
        http(&address, "POST", "/apply", Some(&user1), &fingerprint, &json!({"key": "k", "request": {"compose": "Note", "steps": []}})).await;
    assert_eq!(refused["code"], "WIRE.ADAPTER_UNSUPPORTED", "{refused}");

    // A forged or revoked identity gets nothing.
    let (_, forged) = http(&address, "POST", "/read", Some("not.a.token"), &fingerprint, &recruitments).await;
    assert_eq!(forged["code"], "WIRE.UNAUTHENTICATED", "{forged}");
    run(&env, &["principal", "revoke", "--subject", "user-2"], &config).await.expect("revoke");
    let (_, revoked) = http(&address, "POST", "/read", Some(&user2), &fingerprint, &recruitments).await;
    assert_eq!(revoked["code"], "WIRE.UNAUTHENTICATED", "{revoked}");

    // Drift on the business schema: the next request and /ready close, restoring it reopens, with no cache in between.
    setup.query_drop(format!("ALTER TABLE `{SOURCE}`.`club` MODIFY `name` VARCHAR(100) NOT NULL")).await.unwrap();
    let (_, drifted) = http(&address, "POST", "/read", Some(&user1), &fingerprint, &recruitments).await;
    assert_eq!(drifted["code"], "WIRE.DEPLOYMENT_CHANGED", "{drifted}");
    assert_eq!(http(&address, "GET", "/ready", None, "", &Value::Null).await.0, 503);
    let refused_start = start_server(&env, &file, &config).await.expect_err("a drifted schema must not start a server");
    assert_eq!(refused_start["code"], "WIRE.DEPLOYMENT_CHANGED", "{refused_start}");
    setup.query_drop(format!("ALTER TABLE `{SOURCE}`.`club` MODIFY `name` VARCHAR(255) NOT NULL")).await.unwrap();
    let (_, reopened) = http(&address, "POST", "/read", Some(&user1), &fingerprint, &recruitments).await;
    assert_eq!(ids(&reopened), vec![101, 100], "{reopened}");

    drop(child);
    setup.query_drop("DROP USER IF EXISTS 'aip_t_cli_rt'@'%'").await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
}
