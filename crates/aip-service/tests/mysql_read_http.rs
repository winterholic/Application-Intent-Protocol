#![cfg(feature = "mysql")]
//! WBS 7.2.1.1 read path over a real MySQL 8.4: policy `/read`, `/ready`, the request-time deployment fence and the
//! explicit refusal of everything the adapter does not implement yet.
//!
//! The business tables are plain MySQL tables the test creates; the AIP schema holds only the adoption views and the
//! deployment marker, as an external adoption would leave it. Run with `-- --ignored` against a run-owned container
//! (`scripts/mysql-live-fixture.sh`).
use aip_migrate::mysql_backend::MysqlBackend;
use aip_migrate::mysql_driver::{MysqlDeploymentFence, MysqlDriver, mysql_async::prelude::Queryable};
use aip_migrate::mysql_external::view_statements;
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};
use spike_v2_read::external::{Mapping, Table};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::sqlgen;
use spike_v6_transport::infrastructure::Infrastructure;
use spike_v6_transport::server::{ServerOptions, listen_relational};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const RECRUITMENT: &str = include_str!("../../../spikes/spike-v1-fixture/fixture/recruitment.aip");
const NOTE: &str = "
resource Note {
  fields { id: Id; title: Text; done: Bool; score: Decimal(10,2); due: Date?; created: Time; kind: NoteKind; owner: Member? }
  rows read when true
  expose read {
    select id, title, done, score, due, created, kind, owner
    filter done.eq, score.gte, title.contains, created.lte
    sort created, score, id
    traverse comments via Comment.note { select id, body, secret; sort id desc; limit 3 }
    budget { rows 20; depth 2; deadline 2s; cost 1000 }
  }
}
enum NoteKind { MEMO, TODO }
enum CommentState { VISIBLE, HIDDEN }
resource Comment {
  fields { id: Id; note: Note; author: Member?; body: Text(1..200); secret: Text?; state: CommentState }
  rows read when state = VISIBLE
  field secret read when author = actor
  expose read { select id, body, secret, state }
}
";
const META: &str = "aip_t_mysql_read_meta";
const SOURCE: &str = "aip_t_mysql_read_src";

/// Test-only DDL for the engine's field types. Real deployments bring their own tables.
fn source_ddl(facts: &Value) -> Vec<String> {
    let mut out = vec![];
    for (resource, definition) in facts["resources"].as_object().unwrap() {
        let mut columns = vec![];
        for (field, fd) in definition["fields"].as_object().unwrap() {
            let ty = fd["ty"].as_str().unwrap();
            let base = ty.trim_end_matches('?');
            let column = sqlgen::column(facts, resource, field).unwrap();
            let sql_ty = if field == "id" {
                "BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY".to_string()
            } else {
                let physical = if base.starts_with("Ref<") {
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
                        // Keep Recruitment.periodEnd as DATETIME while Note.created exercises TIMESTAMP.
                        // Both physical types must produce the same UTC wire format below.
                        "Time" if resource == "Note" && field == "created" => "TIMESTAMP(6)",
                        "Time" => "DATETIME(6)",
                        "Date" => "DATE",
                        other => panic!("test DDL has no type for {other}"),
                    }
                };
                format!("{physical}{}", if ty.ends_with('?') { " NULL" } else { " NOT NULL" })
            };
            columns.push(format!("`{column}` {sql_ty}"));
        }
        out.push(format!("CREATE TABLE `{SOURCE}`.`{}` ({}) ENGINE=InnoDB", sqlgen::snake(resource), columns.join(", ")));
    }
    out
}

const SEED: &[&str] = &[
    "INSERT INTO school (id, name) VALUES (1, 'A대'), (2, 'B대')",
    "INSERT INTO member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (5, NULL)",
    "INSERT INTO club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1), (11, 'Open', NULL, NULL), (12, 'B', NULL, 2)",
    "INSERT INTO club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 2, 'MEMBER')",
    "INSERT INTO recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES \
     (100, 'A 모집', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY), 'PUBLISHED', 0, 10, 'note'), \
     (101, 'Open 모집', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 31 DAY), 'PUBLISHED', 5, 11, NULL), \
     (102, 'B 모집', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 32 DAY), 'PUBLISHED', 2, 12, NULL), \
     (103, '지난 모집', DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 DAY), 'PUBLISHED', 1, 10, NULL), \
     (104, '초안', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 33 DAY), 'DRAFT', 0, 10, NULL)",
    "INSERT INTO recruitment_bookmark (recruitment_id, member_id) VALUES (100, 1), (100, 2), (101, 1)",
    "INSERT INTO note (id, title, done, score, due, created, kind, owner_id) VALUES \
     (1, 'first', 1, 10.50, '2026-10-18', '2026-10-01 09:30:00.123400', 'MEMO', 1), \
     (2, 'second', 0, 2.25, NULL, '2026-10-02 00:00:00.000000', 'TODO', NULL), \
     (3, 'Third%_', 1, 99999999.99, '2026-12-31', '2026-10-03 23:59:59.999999', 'TODO', 3)",
    "INSERT INTO comment (id, note_id, author_id, body, secret, state) VALUES \
     (1, 1, 1, 'c1', 's1', 'VISIBLE'), (2, 1, 2, 'c2-hidden', 's2', 'HIDDEN'), (3, 1, 2, 'c3', 's3', 'VISIBLE'), \
     (4, 1, NULL, 'c4', NULL, 'VISIBLE'), (5, 1, 1, 'c5', 's5', 'VISIBLE'), (6, 2, 1, 'other-note', 's6', 'VISIBLE')",
];

async fn post(addr: std::net::SocketAddr, path: &str, token: Option<&str>, fingerprint: &str, body: &Value) -> Value {
    let body = body.to_string();
    let auth = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!("POST {path} HTTP/1.1\r\nhost: localhost\r\n{auth}x-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(10), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

async fn get(addr: std::net::SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(format!("GET {path} HTTP/1.1\r\nhost: localhost\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(10), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    String::from_utf8(out).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap()
}

fn read(query: Value) -> Value {
    json!({ "query": query })
}

fn ids(response: &Value) -> Vec<i64> {
    assert_eq!(response["ok"], true, "{response}");
    response["rows"].as_array().unwrap().iter().map(|row| row["id"].as_i64().unwrap()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_read_path_serves_policy_reads_and_fences_ddl() {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let facts = load_str(&format!("{RECRUITMENT}\n{NOTE}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    sqlgen::try_set_schema(META).unwrap();

    // Business database, adoption views, deployment marker.
    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
    for statement in source_ddl(&facts) {
        setup.query_drop(statement).await.unwrap_or_else(|e| panic!("{e}"));
    }
    setup.query_drop(format!("USE `{SOURCE}`")).await.unwrap();
    for statement in SEED {
        setup.query_drop(*statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    let mut mapping = Mapping { schema: SOURCE.into(), tables: Default::default() };
    for (resource, definition) in facts["resources"].as_object().unwrap() {
        let columns =
            definition["fields"].as_object().unwrap().keys().map(|field| (field.clone(), sqlgen::column(&facts, resource, field).unwrap())).collect();
        mapping.tables.insert(resource.clone(), Table { table: sqlgen::snake(resource), columns, shared_pk_without_fk: false });
    }
    let facts_digest = spike_v1_fixture::digest(&facts);
    let fingerprint = admin.ensure_meta_schema_bootstrap(META, SOURCE, &facts_digest).await.expect("meta bootstrap");
    for statement in view_statements(META, &facts, &mapping).unwrap() {
        setup.query_drop(statement).await.unwrap_or_else(|e| panic!("view: {e}"));
    }
    setup.query_drop("DROP USER IF EXISTS 'aip_t_reader'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_reader'@'%' IDENTIFIED BY 'reader_test_only'").await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("GRANT SELECT, CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_reader'@'%'")).await.unwrap();
    }
    drop(setup);

    // The runtime account is read-only: the read path must work with SELECT alone.
    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap();
    let runtime = MysqlDriver::from_url(&format!("mysql://aip_t_reader:reader_test_only@{host}/{SOURCE}")).unwrap();
    let fence = MysqlDeploymentFence::from_fingerprint(META, SOURCE, &facts_digest, &fingerprint).unwrap();
    let backend = MysqlBackend::new(runtime, fence);
    backend.check_server_profile().await.expect("MySQL 8.4 strict profile");

    let wire = IdWire::Legacy;
    let contract = spike_v5_sdk::contract_fingerprint_with_apply(&facts, wire);
    let server = listen_relational(
        facts.clone(),
        wire,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        ServerOptions::default(),
        Infrastructure::default(),
        None,
    )
    .await
    .expect("listen");
    let addr = server.address;
    let (anon, member1, member2) = (None, Some(server.keys.issue(1, 600)), Some(server.keys.issue(2, 600)));
    let list = |token: &Option<String>, query: Value| {
        let token = token.clone();
        let contract = contract.clone();
        async move { post(addr, "/read", token.as_deref(), &contract, &read(query)).await }
    };

    assert_eq!(get(addr, "/ready").await, 200, "ready before any change");

    // Row policy: active(this) and (club.school = null or club.school = actor.school), actor from the signed token.
    let recruitments = json!({ "read": "Recruitment", "select": ["id", "title", "views", "internalNote"], "sort": [{"field":"views","dir":"desc"}] });
    assert_eq!(ids(&list(&anon, recruitments.clone()).await), vec![101], "anonymous: only school-less clubs, no expired or draft rows");
    assert_eq!(ids(&list(&member1, recruitments.clone()).await), vec![101, 100], "school 1 member: own school and open clubs, sorted by views desc");
    let response = list(&member1, recruitments.clone()).await;
    assert_eq!(response["rows"][1]["internalNote"], "note", "club manager sees the field");
    assert_eq!(response["rows"][0]["internalNote"], Value::Null);
    assert_eq!(list(&member2, recruitments).await["rows"][1]["internalNote"], Value::Null, "plain member: field read policy redacts to null");

    // Time is the engine's ISO shape, traverse and aggregates come back as nested JSON.
    let rich = list(
        &member1,
        json!({ "read": "Recruitment", "select": ["id", "periodEnd", "bookmarkCount", {"club": {"select": ["id", "name"]}}], "sort": [{"field":"id","dir":"asc"}] }),
    )
    .await;
    assert_eq!(ids(&rich), vec![100, 101], "{rich}");
    let period_end = rich["rows"][0]["periodEnd"].as_str().unwrap();
    assert!(period_end.ends_with("+00:00") && period_end.contains('T'), "UTC ISO time: {period_end}");
    assert_eq!(rich["rows"][0]["club"], json!({"id": 10, "name": "A"}));
    assert_eq!(rich["rows"][0]["bookmarkCount"], 2, "{rich}");
    assert_eq!(rich["rows"][1]["bookmarkCount"], 1);

    // Scalar mapping and operators: Bool, Decimal, Date, Time, Enum, Ref, contains, sort with a tie-break.
    let notes = list(&anon, json!({ "read": "Note", "select": ["id", "title", "done", "score", "due", "created", "kind", "owner"], "sort": [{"field":"created","dir":"asc"}] })).await;
    assert_eq!(ids(&notes), vec![1, 2, 3], "{notes}");
    assert_eq!(notes["rows"][0]["done"], true, "tinyint(1) is a JSON bool: {notes}");
    assert_eq!(notes["rows"][1]["done"], false);
    assert_eq!(notes["rows"][0]["score"], "10.50", "decimals stay strings with their scale");
    assert_eq!(notes["rows"][2]["score"], "99999999.99");
    assert_eq!(notes["rows"][0]["due"], "2026-10-18");
    assert_eq!(notes["rows"][1]["due"], Value::Null);
    assert_eq!(notes["rows"][0]["created"], "2026-10-01T09:30:00.1234+00:00", "PostgreSQL trims trailing fraction zeros");
    assert_eq!(notes["rows"][1]["created"], "2026-10-02T00:00:00+00:00", "no fraction when zero");
    assert_eq!(notes["rows"][2]["created"], "2026-10-03T23:59:59.999999+00:00");
    assert_eq!(notes["rows"][0]["kind"], "MEMO");
    // Member has no row-read policy, so the reference guard shows null instead of the raw foreign key (security audit O-01).
    assert_eq!(notes["rows"][0]["owner"], Value::Null, "{notes}");

    // 1:N traverse: hidden children do not take a slot, the definition's `sort id desc` and `limit 3` decide the list, and the
    // per-field policy redacts a secret to null unless the caller is its author.
    let many = |token: &Option<String>| {
        list(
            token,
            json!({ "read": "Note", "select": ["id", {"comments": {"select": ["id", "body", "secret"]}}], "sort": [{"field":"id","dir":"asc"}] }),
        )
    };
    let (as_anon, as_author) = (many(&anon).await, many(&member1).await);
    let comment_ids = |response: &Value, note: usize| -> Vec<i64> {
        response["rows"][note]["comments"].as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect()
    };
    assert_eq!(comment_ids(&as_anon, 0), vec![5, 4, 3], "newest first, HIDDEN skipped, capped at 3: {as_anon}");
    assert_eq!(comment_ids(&as_anon, 1), vec![6], "{as_anon}");
    assert_eq!(comment_ids(&as_anon, 2), Vec::<i64>::new(), "a note without comments gets an empty list, not null: {as_anon}");
    assert!(as_anon["rows"][0]["comments"].as_array().unwrap().iter().all(|c| c["secret"].is_null()), "anonymous never sees a secret");
    assert_eq!(as_author["rows"][0]["comments"][0]["secret"], "s5", "actor 1 wrote comment 5: {as_author}");
    assert_eq!(as_author["rows"][0]["comments"][1]["secret"], Value::Null, "comment 4 has no author");
    assert_eq!(as_author["rows"][0]["comments"][2]["secret"], Value::Null, "comment 3 was written by actor 2");

    let filtered = |filter: Value| list(&anon, json!({ "read": "Note", "select": ["id"], "filter": [filter], "sort": [{"field":"id","dir":"asc"}] }));
    assert_eq!(ids(&filtered(json!({"field":"done","op":"eq","value":true})).await), vec![1, 3]);
    assert_eq!(ids(&filtered(json!({"field":"score","op":"gte","value":"10.50"})).await), vec![1, 3]);
    assert_eq!(ids(&filtered(json!({"field":"title","op":"contains","value":"%_"})).await), vec![3], "LIKE wildcards are literal text");
    assert_eq!(ids(&filtered(json!({"field":"created","op":"lte","value":"2026-10-02T00:00:00Z"})).await), vec![1, 2]);
    // Out-of-contract requests keep the planner's own rejection codes.
    let rejected = filtered(json!({"field":"title","op":"eq","value":"x"})).await;
    assert_eq!(rejected["code"], "WIRE.FILTER_NOT_ALLOWED", "{rejected}");

    // Everything outside the implemented read path is refused by name, not emulated.
    for path in ["/capabilities"] {
        let refused = post(addr, path, member1.as_deref(), &contract, &json!({})).await;
        assert_eq!(refused["code"], "WIRE.ADAPTER_UNSUPPORTED", "{path}: {refused}");
    }

    // The fence has no cache: DDL on the business schema is rejected on the very next request, and /ready follows.
    let mut ddl = admin.pool().get_conn().await.unwrap();
    ddl.query_drop(format!("ALTER TABLE `{SOURCE}`.`club` ADD COLUMN `drift_probe` INT NULL")).await.unwrap();
    let drifted = list(&member1, recruitments_for_drift()).await;
    assert_eq!(drifted["code"], "WIRE.DEPLOYMENT_CHANGED", "{drifted}");
    assert_eq!(get(addr, "/ready").await, 503);
    ddl.query_drop(format!("ALTER TABLE `{SOURCE}`.`club` DROP COLUMN `drift_probe`")).await.unwrap();
    assert_eq!(ids(&list(&member1, recruitments_for_drift()).await), vec![101, 100], "reverting the drift reopens the service");
    assert_eq!(get(addr, "/ready").await, 200);
    // A change to the deployment marker closes it as well.
    ddl.query_drop(format!("UPDATE `{META}`.`aip_migrate_meta` SET facts_digest = REPEAT('0', 64) WHERE singleton = 1")).await.unwrap();
    assert_eq!(list(&member1, recruitments_for_drift()).await["code"], "WIRE.DEPLOYMENT_CHANGED");
    drop(ddl);

    server.shutdown().await.unwrap();
    let mut cleanup = admin.pool().get_conn().await.unwrap();
    cleanup.query_drop("DROP USER IF EXISTS 'aip_t_reader'@'%'").await.unwrap();
    for schema in [META, SOURCE] {
        cleanup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
}

fn recruitments_for_drift() -> Value {
    json!({ "read": "Recruitment", "select": ["id"], "sort": [{"field":"views","dir":"desc"}] })
}
