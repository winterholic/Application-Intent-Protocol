#![cfg(feature = "mysql")]
//! WBS 7.2.1.2 write path over a real MySQL 8.4: `/apply` through the adoption views, the idempotency journal, audit,
//! outbox, effects, capacity limits, commit-time checks, concurrency and the deployment fence.
//!
//! The deployment is built through the library calls the CLI uses (`adopt_external`, `deployment`), then served in
//! process. Run with `-- --ignored` against a run-owned container (`scripts/mysql-live-fixture.sh`).
use aip_migrate::mysql_adopt;
use aip_migrate::mysql_backend::MysqlBackend;
use aip_migrate::mysql_driver::mysql_async::{Row, prelude::Queryable};
use aip_migrate::mysql_driver::{MysqlDeploymentFence, MysqlDriver};
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

const DEFINITION: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Log { fields { id: Id; title: Text } }
limit maxOpen on Note = atMost 2 where done = false
resource Note {
  fields { id: Id; owner: Member; title: Text(1..8); content: Text?; amount: Decimal(8,2)?; version: Int(0..100) default database; done: Bool default database; hits: Int(0..5) default database }
  rows read when true
  transition edit { from true; allow owner = actor; patch title, content, amount; expect version; to version = version + 1; create Log { title = title }; notify owner "note.edited" }
  transition finish { from done = false; allow owner = actor; to done = true; repeat unchanged }
  transition reopen { from done = true; allow owner = actor; to done = false }
  transition hit { from true; allow actor != null; to hits = hits + 1 }
  expose apply edit { target id; bulk maxRows 1 }
  expose apply finish { target id; bulk maxRows 5 }
  expose apply reopen { target id; bulk maxRows 5 }
  expose apply hit { target id; bulk maxRows 5 }
  expose create { allow owner = actor; fields owner, title, content }
  expose read { select id, title, content, amount, version, done, hits; filter done.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  invariant maxOpen per owner
  check notBlocked when title != "blocked"
}
"#;
// One schema per process (the planner's namespace is process-wide), so the tests take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const META: &str = "aip_t_mysql_write";
const SOURCE: &str = "aip_t_mysql_write_src";

fn ddl_for(facts: &Value) -> Vec<String> {
    let mut statements = vec![];
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
                    "BIGINT".to_string()
                } else if let Some(inner) = base.strip_prefix("Decimal<").and_then(|rest| rest.strip_suffix('>')) {
                    format!("DECIMAL({inner})")
                } else {
                    match base {
                        "Text" | "Email" | "Url" => "VARCHAR(8)".to_string(),
                        "Int" => "BIGINT".to_string(),
                        "Bool" => "TINYINT(1)".to_string(),
                        "Time" => "DATETIME(6)".to_string(),
                        other => panic!("test DDL has no type for {other}"),
                    }
                };
                let default = if base == "Bool" || base == "Int" { " DEFAULT 0" } else { "" };
                format!("{physical}{}{default}", if ty.ends_with('?') { " NULL" } else { " NOT NULL" })
            };
            columns.push(format!("`{column}` {sql_ty}"));
        }
        columns.extend(constraints);
        statements.push(format!("CREATE TABLE `{SOURCE}`.`{}` ({}) ENGINE=InnoDB", sqlgen::snake(resource), columns.join(", ")));
    }
    // Referenced tables first.
    statements.sort_by_key(|s| (s.contains("REFERENCES"), s.clone()));
    statements
}

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
    tokio::time::timeout(std::time::Duration::from_secs(15), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

struct Harness {
    addr: std::net::SocketAddr,
    contract: String,
    owner: String,
    other: String,
    admin: MysqlDriver,
    _server: spike_v6_transport::server::Server,
}

impl Harness {
    async fn apply(&self, token: &str, key: &str, request: Value) -> Value {
        post(self.addr, "/apply", Some(token), &self.contract, &json!({"key": key, "request": request})).await
    }
    async fn sql(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        let mut conn = self.admin.pool().get_conn().await.unwrap();
        let rows: Vec<Row> = conn.query(sql).await.unwrap();
        rows.into_iter().map(|row| (0..row.len()).map(|i| row.get::<Option<String>, _>(i).unwrap_or(None)).collect()).collect()
    }
    async fn count(&self, sql: &str) -> i64 {
        self.sql(sql).await[0][0].clone().unwrap().parse().unwrap()
    }
}

async fn harness() -> Harness {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let facts = load_str(DEFINITION, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    sqlgen::try_set_schema(META).unwrap();
    let admin = MysqlDriver::connect(&admin_url).await.expect("admin connection");
    let mut setup = admin.pool().get_conn().await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
    }
    setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
    for statement in ddl_for(&facts) {
        setup.query_drop(&statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    setup.query_drop(format!("INSERT INTO `{SOURCE}`.`member` (id) VALUES (1), (2)")).await.unwrap();
    setup
        .query_drop(format!(
            "INSERT INTO `{SOURCE}`.`note` (id, owner_id, title, content, version, done, hits) VALUES \
             (10, 1, 'before', 'body', 0, 0, 0), (11, 1, 'second', NULL, 0, 0, 0), (12, 2, 'theirs', NULL, 0, 0, 0)"
        ))
        .await
        .unwrap();
    let mut mapping = Mapping { schema: SOURCE.into(), tables: Default::default() };
    for (resource, definition) in facts["resources"].as_object().unwrap() {
        let columns =
            definition["fields"].as_object().unwrap().keys().map(|field| (field.clone(), sqlgen::column(&facts, resource, field).unwrap())).collect();
        mapping.tables.insert(resource.clone(), Table { table: sqlgen::snake(resource), columns });
    }
    // The runtime account may read and write the business tables and the AIP schema, nothing more.
    setup.query_drop("DROP USER IF EXISTS 'aip_t_writer'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_writer'@'%' IDENTIFIED BY 'writer_test_only'").await.unwrap();
    for schema in [META, SOURCE] {
        setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE ON `{schema}`.* TO 'aip_t_writer'@'%'")).await.unwrap();
    }
    drop(setup);

    let plan = mysql_adopt::plan_external(&admin_url, META, &facts, &mapping, "decimal").await.unwrap_or_else(|e| panic!("plan: {e}"));
    let digest = plan["digest"].as_str().unwrap();
    mysql_adopt::adopt_external(&admin_url, META, &facts, &mapping, "decimal", digest).await.unwrap_or_else(|e| panic!("adopt: {e}"));

    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap();
    let runtime_url = format!("mysql://aip_t_writer:writer_test_only@{host}/{SOURCE}");
    let deployment = mysql_adopt::deployment(&runtime_url, META, &facts, &mapping, "decimal").await.unwrap_or_else(|e| panic!("deployment: {e}"));
    let runtime = MysqlDriver::from_url(&runtime_url).unwrap();
    let fence = MysqlDeploymentFence::new(META, SOURCE, &deployment.source_tables, &deployment.facts_digest, &deployment.structure_digest).unwrap();
    let backend = MysqlBackend::new(runtime, fence);
    backend.check_server_profile().await.expect("MySQL 8.4 strict profile");
    let wire = IdWire::DecimalString;
    let deployed = deployment.facts;
    let contract = spike_v5_sdk::contract_fingerprint_with_apply(&deployed, wire);
    let server = listen_relational(
        deployed,
        wire,
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(backend),
        ServerOptions::default(),
        Infrastructure::default(),
        None,
    )
    .await
    .expect("listen");
    let (owner, other) = (server.keys.issue(1, 600), server.keys.issue(2, 600));
    Harness { addr: server.address, contract, owner, other, admin, _server: server }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_apply_is_atomic_idempotent_audited_and_fenced() {
    let _turn = SERIAL.lock().await;
    let h = harness().await;
    let meta = |table: &str| format!("`{META}`.`{table}`");

    // A patch with an expectation, effects and the journal in one transaction.
    let edit = json!({"apply": "Note.edit", "target": {"ids": ["10"]}, "values": {"title": "새 제목", "amount": "123.45"}, "expect": {"version": 0}});
    let done = h.apply(&h.owner, "edit-1", edit.clone()).await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["changed"], json!(["10"]));
    assert_eq!(done["tags"], json!(["Log", "Note"]));
    let rows = h.sql(&format!("SELECT title, content, version, amount FROM `{SOURCE}`.`note` WHERE id = 10")).await;
    assert_eq!(
        rows[0],
        [Some("새 제목".into()), Some("body".into()), Some("1".into()), Some("123.45".into())],
        "content is untouched by a partial patch"
    );
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`log` WHERE title = '새 제목'")).await, 1, "create effect");
    assert_eq!(
        h.count(&format!("SELECT COUNT(*) FROM {} WHERE topic = 'note.edited' AND recipient_id = '1' AND source_id = '10'", meta("aip_outbox")))
            .await,
        1,
        "notify effect"
    );
    let audit = h
        .sql(&format!(
            "SELECT actor_id, via, request_key, resource, transition, target_id, CAST(before_values AS CHAR), CAST(after_values AS CHAR) FROM {}",
            meta("aip_audit")
        ))
        .await;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        &audit[0][..6],
        &[Some("1".into()), Some("apply".into()), Some("edit-1".into()), Some("Note".into()), Some("edit".into()), Some("10".into())]
    );
    let (before, after): (Value, Value) =
        (serde_json::from_str(audit[0][6].as_ref().unwrap()).unwrap(), serde_json::from_str(audit[0][7].as_ref().unwrap()).unwrap());
    assert_eq!((before["title"].clone(), before["version"].clone()), (json!("before"), json!(0)), "{before}");
    assert_eq!((after["title"].clone(), after["version"].clone(), after["amount"].clone()), (json!("새 제목"), json!(1), json!("123.45")), "{after}");

    // The same key replays the stored result and writes nothing; another request on the key is refused.
    let replay = h.apply(&h.owner, "edit-1", edit.clone()).await;
    assert_eq!((replay["ok"].clone(), replay["replayed"].clone(), replay["changed"].clone()), (json!(true), json!(true), json!(["10"])), "{replay}");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`log`")).await, 1);
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM {}", meta("aip_audit"))).await, 1);
    let mut other_request = edit.clone();
    other_request["values"]["title"] = json!("다른");
    assert_eq!(h.apply(&h.owner, "edit-1", other_request).await["code"], "WIRE.IDEMPOTENCY_MISMATCH");
    let status = post(h.addr, "/status", Some(&h.owner), &h.contract, &json!({"key": "edit-1", "request": edit})).await;
    assert_eq!((status["ok"].clone(), status["replayed"].clone()), (json!(true), json!(true)), "{status}");
    assert_eq!(post(h.addr, "/status", Some(&h.owner), &h.contract, &json!({"key": "never", "request": {}})).await["code"], "WIRE.NOT_FOUND");

    // A stale expectation matches nothing: the whole write is cancelled, including its effects and journal row.
    let stale = json!({"apply": "Note.edit", "target": {"ids": ["10"]}, "values": {"title": "stale"}, "expect": {"version": 0}});
    let rejected = h.apply(&h.owner, "edit-stale", stale).await;
    assert_eq!(rejected["code"], "WIRE.CONFLICT", "{rejected}");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`log`")).await, 1, "effects rolled back");
    assert_eq!(
        h.count(&format!("SELECT COUNT(*) FROM {} WHERE idem_key = 'edit-stale'", meta("aip_idem"))).await,
        0,
        "no journal row for a failed write"
    );
    assert_eq!(h.count(&format!("SELECT version FROM `{SOURCE}`.`note` WHERE id = 10")).await, 1);

    // Authorization, values and the declared limits are decided before anything is written.
    let theirs = json!({"apply": "Note.edit", "target": {"ids": ["12"]}, "values": {"title": "x"}, "expect": {"version": 0}});
    assert_eq!(h.apply(&h.owner, "edit-theirs", theirs).await["code"], "WIRE.FORBIDDEN");
    for (values, code) in
        [(json!({"title": "123456789"}), "WIRE.BAD_VALUE"), (json!({"title": null}), "WIRE.BAD_VALUE"), (json!({}), "WIRE.BAD_VALUE")]
    {
        let request = json!({"apply": "Note.edit", "target": {"ids": ["10"]}, "values": values, "expect": {"version": 1}});
        assert_eq!(h.apply(&h.owner, &format!("bad-{code}-{values}"), request).await["code"], code, "{values}");
    }
    let hidden = json!({"apply": "Note.edit", "target": {"ids": ["999"]}, "values": {"title": "x"}, "expect": {"version": 0}});
    assert_eq!(h.apply(&h.owner, "edit-missing", hidden).await["code"], "WIRE.MISSING_TARGET");

    // The commit-time check sees exactly the rows this transaction wrote.
    let blocked = json!({"apply": "Note.edit", "target": {"ids": ["10"]}, "values": {"title": "blocked"}, "expect": {"version": 1}});
    let failed = h.apply(&h.owner, "edit-blocked", blocked).await;
    assert_eq!(failed["code"], "WIRE.CHECK_FAILED", "{failed}");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`note` WHERE title = 'blocked'")).await, 0);

    // Repeat-unchanged, bulk and the other direction.
    let finish = json!({"apply": "Note.finish", "target": {"ids": ["10", "11"]}});
    let first = h.apply(&h.owner, "finish-1", finish.clone()).await;
    assert_eq!((first["changed"].clone(), first["unchanged"].clone()), (json!(["10", "11"]), json!([])), "{first}");
    let again = h.apply(&h.owner, "finish-2", finish).await;
    assert_eq!((again["changed"].clone(), again["unchanged"].clone()), (json!([]), json!(["10", "11"])), "{again}");
    assert_eq!(h.apply(&h.owner, "reopen-1", json!({"apply": "Note.reopen", "target": {"ids": ["10"]}})).await["changed"], json!(["10"]));

    // Bounded increment: the declared range stops the sixth hit and the failed one changes nothing.
    for n in 1..=5 {
        let hit = h.apply(&h.owner, &format!("hit-{n}"), json!({"apply": "Note.hit", "target": {"ids": ["12"]}})).await;
        assert_eq!(hit["ok"], true, "{n}: {hit}");
    }
    let over = h.apply(&h.owner, "hit-6", json!({"apply": "Note.hit", "target": {"ids": ["12"]}})).await;
    assert_eq!(over["code"], "WIRE.BAD_VALUE", "{over}");
    assert_eq!(h.count(&format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = 12")).await, 5);

    // Bundles: create then act, in one transaction; a refusal anywhere undoes the lot.
    let bundle = json!({"atomic": [
        {"create": "Note", "values": {"owner": "1", "title": "made", "content": "c"}},
        {"apply": "Note.hit", "target": {"ids": ["10"]}},
    ]});
    let made = h.apply(&h.owner, "bundle-1", bundle).await;
    assert_eq!(made["ok"], true, "{made}");
    let new_id = made["created"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(made["created"][0]["resource"], "Note");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`note` WHERE id = {new_id} AND title = 'made' AND owner_id = 1")).await, 1);
    let forged_owner = json!({"atomic": [{"create": "Note", "values": {"owner": "2", "title": "forged"}}]});
    // Member has no row-read policy, so another member is not a row the caller may point at: same answer as a missing row.
    assert_eq!(h.apply(&h.owner, "bundle-forged", forged_owner).await["code"], "WIRE.MISSING_TARGET");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`note` WHERE title = 'forged'")).await, 0, "a refused create leaves no row");
    let half = json!({"atomic": [
        {"apply": "Note.hit", "target": {"ids": ["10"]}},
        {"create": "Note", "values": {"owner": "2", "title": "denied"}},
    ]});
    let hits_before = h.count(&format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = 10")).await;
    assert_eq!(h.apply(&h.owner, "bundle-half", half).await["code"], "WIRE.MISSING_TARGET");
    assert_eq!(h.count(&format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = 10")).await, hits_before, "the first operation was undone too");

    // Anything not implemented is refused by name.
    let compose = h.apply(&h.owner, "compose-1", json!({"compose": "Note", "steps": []})).await;
    assert_eq!(compose["code"], "WIRE.ADAPTER_UNSUPPORTED", "{compose}");

    // The runtime role cannot rewrite the audit trail.
    let mut conn = h.admin.pool().get_conn().await.unwrap();
    assert!(conn.query_drop(format!("UPDATE {} SET via = 'x'", meta("aip_audit"))).await.is_err(), "audit rows are append-only");
    assert!(conn.query_drop(format!("DELETE FROM {}", meta("aip_audit"))).await.is_err(), "audit rows are append-only");

    // Drift closes the write path as it does the read path, and writes nothing.
    conn.query_drop(format!("ALTER TABLE `{SOURCE}`.`log` MODIFY `title` VARCHAR(9) NOT NULL")).await.unwrap();
    let drifted = h.apply(&h.owner, "after-drift", json!({"apply": "Note.hit", "target": {"ids": ["11"]}})).await;
    assert_eq!(drifted["code"], "WIRE.DEPLOYMENT_CHANGED", "{drifted}");
    assert_eq!(h.count(&format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = 11")).await, 0);
    conn.query_drop(format!("ALTER TABLE `{SOURCE}`.`log` MODIFY `title` VARCHAR(8) NOT NULL")).await.unwrap();
    assert_eq!(h.apply(&h.owner, "after-restore", json!({"apply": "Note.hit", "target": {"ids": ["11"]}})).await["ok"], true);
    let _ = &h.other;
}

/// Concurrent writers: a capacity limit that two requests race for, increments that must not overwrite each other, a key
/// sent many times at once, and an expectation only one writer can meet.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_concurrent_writes_keep_capacity_counters_and_keys_exact() {
    let _turn = SERIAL.lock().await;
    let h = Arc::new(harness().await);
    let note = |id: i64| format!("`{SOURCE}`.`note` WHERE id = {id}");

    // Capacity: owner 1 may hold at most 2 open notes. Three finished notes are reopened at once, several rounds.
    h.sql(&format!("INSERT INTO `{SOURCE}`.`note` (id, owner_id, title, done) VALUES (13, 1, 'third', 0)")).await;
    for round in 0..5 {
        h.sql(&format!("UPDATE `{SOURCE}`.`note` SET done = 1 WHERE owner_id = 1")).await;
        let mut joins = vec![];
        for id in [10, 11, 13] {
            let h = h.clone();
            joins.push(tokio::spawn(async move {
                h.apply(&h.owner, &format!("reopen-{round}-{id}"), json!({"apply": "Note.reopen", "target": {"ids": [id.to_string()]}})).await
            }));
        }
        let mut answers = vec![];
        for join in joins {
            answers.push(join.await.unwrap());
        }
        let ok = answers.iter().filter(|a| a["ok"] == true).count();
        let refused = answers.iter().filter(|a| a["code"] == "WIRE.INVARIANT_VIOLATED").count();
        assert_eq!((ok, refused), (2, 1), "round {round}: {answers:?}");
        assert_eq!(
            h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`note` WHERE owner_id = 1 AND done = 0")).await,
            2,
            "round {round}: never above the limit"
        );
    }

    // Counters: eight concurrent hits on a row capped at 5 end at exactly 5; none is lost, none overshoots.
    h.sql(&format!("UPDATE `{SOURCE}`.`note` SET hits = 0 WHERE id = 12")).await;
    let mut joins = vec![];
    for n in 0..8 {
        let h = h.clone();
        joins.push(tokio::spawn(async move {
            h.apply(&h.other, &format!("hit-race-{n}"), json!({"apply": "Note.hit", "target": {"ids": ["12"]}})).await
        }));
    }
    let mut ok = 0;
    for join in joins {
        let answer = join.await.unwrap();
        if answer["ok"] == true {
            ok += 1;
        } else {
            assert_eq!(answer["code"], "WIRE.BAD_VALUE", "{answer}");
        }
    }
    assert_eq!(ok, 5);
    assert_eq!(h.count(&format!("SELECT hits FROM {}", note(12))).await, 5);

    // One key sent six times at once: executed once, the rest replay it.
    h.sql(&format!("UPDATE `{SOURCE}`.`note` SET version = 0, title = 'start' WHERE id = 10")).await;
    let before_logs = h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`log`")).await;
    let request = json!({"apply": "Note.edit", "target": {"ids": ["10"]}, "values": {"title": "once"}, "expect": {"version": 0}});
    let mut joins = vec![];
    for _ in 0..6 {
        let (h, request) = (h.clone(), request.clone());
        joins.push(tokio::spawn(async move { h.apply(&h.owner, "same-key", request).await }));
    }
    let mut fresh = 0;
    for join in joins {
        let answer = join.await.unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        if answer["replayed"] != true {
            fresh += 1;
        }
    }
    assert_eq!(fresh, 1, "exactly one execution");
    assert_eq!(h.count(&format!("SELECT version FROM {}", note(10))).await, 1);
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`log`")).await, before_logs + 1, "the create effect ran once");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_audit` WHERE request_key = 'same-key'")).await, 1);

    // An expectation only one writer can meet: the others see the changed version and are cancelled.
    h.sql(&format!("UPDATE `{SOURCE}`.`note` SET version = 0 WHERE id = 11")).await;
    let mut joins = vec![];
    for n in 0..6 {
        let h = h.clone();
        let request = json!({"apply": "Note.edit", "target": {"ids": ["11"]}, "values": {"title": format!("t{n}")}, "expect": {"version": 0}});
        joins.push(tokio::spawn(async move { h.apply(&h.owner, &format!("expect-race-{n}"), request).await }));
    }
    let mut winners = 0;
    for join in joins {
        let answer = join.await.unwrap();
        if answer["ok"] == true {
            winners += 1;
        } else {
            assert_eq!(answer["code"], "WIRE.CONFLICT", "{answer}");
        }
    }
    assert_eq!(winners, 1);
    assert_eq!(h.count(&format!("SELECT version FROM {}", note(11))).await, 1);
}
