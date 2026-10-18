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
use spike_v2_read::backend::RelationalBackend;
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
            let uuid_key = |res: &str| facts["resources"][res]["idKind"] == "uuid";
            let sql_ty = if field == "id" && uuid_key(resource) {
                "CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY".to_string()
            } else if field == "id" {
                "BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY".to_string()
            } else {
                let physical = if let Some(target) = base.strip_prefix("Ref<").and_then(|rest| rest.strip_suffix('>')) {
                    constraints.push(format!(
                        "CONSTRAINT `fk_{}_{column}` FOREIGN KEY (`{column}`) REFERENCES `{}` (`id`)",
                        sqlgen::snake(resource),
                        sqlgen::snake(target)
                    ));
                    if uuid_key(target) { "CHAR(36) CHARACTER SET ascii COLLATE ascii_bin".to_string() } else { "BIGINT".to_string() }
                } else if base.starts_with("Enum<") {
                    "VARCHAR(16)".to_string()
                } else if let Some(inner) = base.strip_prefix("Decimal<").and_then(|rest| rest.strip_suffix('>')) {
                    format!("DECIMAL({inner})")
                } else {
                    match base {
                        "Text" | "Email" | "Url" => "VARCHAR(255)".to_string(),
                        "Int" => "BIGINT".to_string(),
                        "Bool" => "TINYINT(1)".to_string(),
                        "Time" => "DATETIME(6)".to_string(),
                        other => panic!("test DDL has no type for {other}"),
                    }
                };
                let enum_default = base
                    .strip_prefix("Enum<")
                    .and_then(|rest| rest.strip_suffix('>'))
                    .and_then(|name| facts["enums"][name][0].as_str())
                    .filter(|_| fd["databaseDefault"] == true)
                    .map(|first| format!(" DEFAULT '{first}'"));
                let default = enum_default.unwrap_or_else(|| if base == "Bool" || base == "Int" { " DEFAULT 0".to_string() } else { String::new() });
                format!("{physical}{}{default}", if ty.ends_with('?') { " NULL" } else { " NOT NULL" })
            };
            columns.push(format!("`{column}` {sql_ty}"));
        }
        columns.extend(constraints);
        statements.push(format!("CREATE TABLE `{SOURCE}`.`{}` ({}) ENGINE=InnoDB", sqlgen::snake(resource), columns.join(", ")));
    }
    // Referenced tables first.
    let mut done: Vec<String> = vec![];
    let mut ordered = vec![];
    while !statements.is_empty() {
        let before = statements.len();
        statements.retain(|statement| {
            let mine = statement.split('`').nth(3).unwrap().to_string();
            let parents: Vec<&str> = statement.split("REFERENCES `").skip(1).map(|rest| rest.split('`').next().unwrap()).collect();
            if parents.iter().all(|parent| *parent == mine || done.iter().any(|d| d == parent)) {
                done.push(mine);
                ordered.push(statement.clone());
                false
            } else {
                true
            }
        });
        assert!(statements.len() < before, "reference cycle in test DDL");
    }
    ordered
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
    facts: Value,
    backend: Arc<MysqlBackend>,
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
    harness_via(None).await
}

/// `proxy`: a fault proxy the runtime account connects through (the admin connection stays direct).
async fn harness_via(proxy: Option<&MysqlProxy>) -> Harness {
    let seed = [
        "INSERT INTO `{SOURCE}`.`member` (id) VALUES (1), (2)".to_string(),
        "INSERT INTO `{SOURCE}`.`note` (id, owner_id, title, content, version, done, hits) VALUES \
         (10, 1, 'before', 'body', 0, 0, 0), (11, 1, 'second', NULL, 0, 0, 0), (12, 2, 'theirs', NULL, 0, 0, 0)"
            .to_string(),
    ];
    harness_with(DEFINITION, &seed, "SELECT, INSERT, UPDATE, DELETE", proxy).await
}

/// `seed`: SQL run on the business schema before adoption; `{SOURCE}` stands for its name.
async fn harness_with(definition: &str, seed: &[String], source_grants: &str, proxy: Option<&MysqlProxy>) -> Harness {
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL 8.4 container");
    aip_test_support::validate_mysql_admin_url(&admin_url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
    assert!(admin_url.starts_with("mysql://"), "this test targets MySQL 8.4");
    let facts = load_str(definition, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
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
    for statement in seed {
        let statement = statement.replace("{SOURCE}", SOURCE);
        setup.query_drop(&statement).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    let mut mapping = Mapping { schema: SOURCE.into(), tables: Default::default() };
    for (resource, definition) in facts["resources"].as_object().unwrap() {
        let columns =
            definition["fields"].as_object().unwrap().keys().map(|field| (field.clone(), sqlgen::column(&facts, resource, field).unwrap())).collect();
        mapping.tables.insert(resource.clone(), Table { table: sqlgen::snake(resource), columns, shared_pk_without_fk: false });
    }
    // The runtime account may read and write the business tables and the AIP schema, nothing more.
    setup.query_drop("DROP USER IF EXISTS 'aip_t_writer'@'%'").await.unwrap();
    setup.query_drop("CREATE USER 'aip_t_writer'@'%' IDENTIFIED BY 'writer_test_only'").await.unwrap();
    setup.query_drop(format!("GRANT SELECT, INSERT, UPDATE, DELETE ON `{META}`.* TO 'aip_t_writer'@'%'")).await.unwrap();
    setup.query_drop(format!("GRANT {source_grants}, CREATE TEMPORARY TABLES ON `{SOURCE}`.* TO 'aip_t_writer'@'%'")).await.unwrap();
    drop(setup);

    let plan = mysql_adopt::plan_external(&admin_url, META, &facts, &mapping, "decimal").await.unwrap_or_else(|e| panic!("plan: {e}"));
    let digest = plan["digest"].as_str().unwrap();
    mysql_adopt::adopt_external(&admin_url, META, &facts, &mapping, "decimal", digest).await.unwrap_or_else(|e| panic!("adopt: {e}"));

    let host = admin_url.split_once('@').unwrap().1.split('/').next().unwrap();
    let runtime_host = proxy.map_or_else(|| host.to_string(), |proxy| proxy.addr.to_string());
    let runtime_url = format!("mysql://aip_t_writer:writer_test_only@{runtime_host}/{SOURCE}");
    let deployment = mysql_adopt::deployment(&runtime_url, META, &facts, &mapping, "decimal").await.unwrap_or_else(|e| panic!("deployment: {e}"));
    let runtime = MysqlDriver::from_url(&runtime_url).unwrap();
    let fence = MysqlDeploymentFence::new(META, SOURCE, &deployment.source_tables, &deployment.facts_digest, &deployment.structure_digest).unwrap();
    let backend = Arc::new(MysqlBackend::new(runtime, fence));
    backend.check_server_profile().await.expect("MySQL 8.4 strict profile");
    let wire = IdWire::DecimalString;
    let deployed = deployment.facts;
    let contract = spike_v5_sdk::contract_fingerprint_with_apply(&deployed, wire);
    let server = listen_relational(
        deployed.clone(),
        wire,
        "127.0.0.1:0".parse().unwrap(),
        backend.clone(),
        ServerOptions::default(),
        Infrastructure::default(),
        None,
    )
    .await
    .expect("listen");
    let (owner, other) = (server.keys.issue(1, 600), server.keys.issue(2, 600));
    Harness { addr: server.address, contract, owner, other, admin, facts: deployed, backend, _server: server }
}

const WORKER_ACTION: &str = include_str!("../../../product/tests/fixtures/mysql-worker-action/video-action.aip");

/// A callback name that merely resembles a declared transition must never widen its exact grant.
#[tokio::test]
#[ignore = "requires a run-owned MySQL 8.4 fixture on AIP_TEST_MYSQL_URL"]
async fn mysql_worker_action_rejects_transition_prefix_and_wildcard_grants() {
    let harness = harness_with(
        WORKER_ACTION,
        &[
            "INSERT INTO `{SOURCE}`.`member` (id) VALUES (1)".into(),
            "INSERT INTO `{SOURCE}`.`video` (id, owner_id, views) VALUES (10, 1, 0)".into(),
        ],
        "SELECT, INSERT, UPDATE, DELETE",
        None,
    )
    .await;
    let wire = IdWire::DecimalString;
    let caller = spike_v2_read::plan::Caller { actor_id: Some(1), now: harness.backend.now().await.unwrap() };
    let request = json!({"extension":"Video.view","input":{"id":"10"}});
    let start = harness.backend.begin_worker_write(&harness.facts, "exact-grant", &request, &caller, wire).await.unwrap();
    let spike_v2_read::backend::RelationalWorkerWriteStart::New(mut session) = start else { panic!("new key must start a transaction") };
    for name in ["Video.incremen", "Video.incrementExtra", "Video.increment*", "Video.*"] {
        let forged_transition = json!({"apply":name,"target":{"ids":["10"]}});
        assert_eq!(
            session.apply(name, &forged_transition, &caller, wire).await.unwrap_err().code,
            "WIRE.ACCESS_NOT_DECLARED",
            "grant expansion through {name}"
        );
    }
    drop(session);
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=10")).await, 0);
    assert_eq!(harness.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_audit` WHERE request_key = 'exact-grant'")).await, 0);
    assert_eq!(harness.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_idem` WHERE idem_key = 'exact-grant'")).await, 0);
}

#[tokio::test]
#[ignore = "requires a run-owned MySQL 8.4 fixture on AIP_TEST_MYSQL_URL"]
async fn mysql_worker_action_uses_one_fenced_idempotent_transaction() {
    let harness = harness_with(
        WORKER_ACTION,
        &[
            "INSERT INTO `{SOURCE}`.`member` (id) VALUES (1), (2)".into(),
            "INSERT INTO `{SOURCE}`.`video` (id, owner_id, views) VALUES (10, 1, 0), (11, 2, 0)".into(),
        ],
        "SELECT, INSERT, UPDATE, DELETE",
        None,
    )
    .await;
    let wire = IdWire::DecimalString;
    let now = harness.backend.now().await.unwrap();
    let caller = spike_v2_read::plan::Caller { actor_id: Some(1), now };
    let request = json!({"extension":"Video.view","input":{"id":"10"}});
    let transition = json!({"apply":"Video.increment","target":{"ids":["10"]}});
    let forged_caller = spike_v2_read::plan::Caller { actor_id: Some(2), now: caller.now.clone() };
    let start = harness.backend.begin_worker_write(&harness.facts, "action-ok", &request, &caller, wire).await.unwrap();
    let spike_v2_read::backend::RelationalWorkerWriteStart::New(mut session) = start else { panic!("new key must start a transaction") };
    let applied = session.apply("Video.increment", &transition, &forged_caller, wire).await.unwrap();
    assert_eq!(applied["changed"], json!(["10"]));
    let committed = session.commit(json!({"ok":true,"output":{"count":1},"tags":["Video"]})).await.unwrap();
    assert_eq!(committed["output"]["count"], 1);
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=10")).await, 1);
    assert_eq!(harness.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_audit` WHERE request_key = 'action-ok'")).await, 1);

    let mut no_grant_facts = harness.facts.clone();
    no_grant_facts["resources"]["Video"]["extensions"]["view"]["access"] = json!({});
    let no_grant = harness.backend.begin_worker_write(&no_grant_facts, "action-no-grant", &request, &caller, wire).await;
    assert_eq!(no_grant.err().unwrap().code, "WIRE.ACCESS_NOT_DECLARED");
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=10")).await, 1);

    let replay = harness.backend.begin_worker_write(&harness.facts, "action-ok", &request, &caller, wire).await.unwrap();
    let spike_v2_read::backend::RelationalWorkerWriteStart::Replay(replay) = replay else { panic!("same request must replay") };
    assert_eq!(replay["replayed"], true);
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=10")).await, 1);
    assert_eq!(harness.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_audit` WHERE request_key = 'action-ok'")).await, 1);

    let conflicting_request = json!({"extension":"Video.view","input":{"id":"11"}});
    let conflict = harness.backend.begin_worker_write(&harness.facts, "action-ok", &conflicting_request, &caller, wire).await;
    assert_eq!(conflict.err().unwrap().code, "WIRE.IDEMPOTENCY_MISMATCH");
    assert_eq!(
        spike_v4_worker::write::validate_write(&harness.facts, "Video.view", &json!({"id":"10","actor":2}), wire).unwrap_err().code,
        "WIRE.BAD_VALUE"
    );

    let other_owner_request = json!({"apply":"Video.increment","target":{"ids":["11"]}});
    let start = harness.backend.begin_worker_write(&harness.facts, "action-denied", &request, &caller, wire).await.unwrap();
    let spike_v2_read::backend::RelationalWorkerWriteStart::New(mut session) = start else { panic!("new denial case") };
    assert_eq!(session.apply("Video.increment", &other_owner_request, &forged_caller, wire).await.unwrap_err().code, "WIRE.FORBIDDEN");
    drop(session);
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=11")).await, 0);

    let mut deadline_facts = harness.facts.clone();
    deadline_facts["resources"]["Video"]["extensions"]["view"]["deadlineMs"] = json!(10);
    let start = harness.backend.begin_worker_write(&deadline_facts, "action-deadline", &request, &caller, wire).await.unwrap();
    let spike_v2_read::backend::RelationalWorkerWriteStart::New(mut session) = start else { panic!("new rollback case") };
    session.apply("Video.increment", &transition, &caller, wire).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(session.commit(json!({"ok":true})).await.unwrap_err().code, "WIRE.DEADLINE_EXCEEDED");
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=10")).await, 1);

    let start = harness.backend.begin_worker_write(&harness.facts, "action-invalid-output", &request, &caller, wire).await.unwrap();
    let spike_v2_read::backend::RelationalWorkerWriteStart::New(mut session) = start else { panic!("new invalid output case") };
    session.apply("Video.increment", &transition, &caller, wire).await.unwrap();
    let invalid_output = spike_v4_worker::write::validate_write_output(&harness.facts, "Video.view", &json!({"count":11}), wire);
    assert_eq!(invalid_output.unwrap_err().code, "WIRE.OUTPUT_INVALID");
    // The transport returns this validation error without calling commit, so dropping the owned session rolls back.
    drop(session);
    assert_eq!(harness.count(&format!("SELECT views FROM `{SOURCE}`.`video` WHERE id=10")).await, 1);
}

/// One-shot MySQL fault injection between the server's runtime account and the database. Faults fire on the first matching
/// COM_QUERY and every later connection passes through untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// COMMIT is swallowed and both sides are closed: the server never saw it and rolls the transaction back.
    CutOnCommit,
    /// COMMIT reaches the server, which commits; the reply is dropped and both sides are closed.
    DropAckAfterCommit,
    /// The statement containing this text is swallowed and both sides are closed: nothing of the write is committed.
    CutOnStatement(&'static str),
}

struct MysqlProxy {
    addr: std::net::SocketAddr,
    armed: Arc<std::sync::Mutex<Option<Fault>>>,
}

impl MysqlProxy {
    async fn start(target: std::net::SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let armed: Arc<std::sync::Mutex<Option<Fault>>> = Arc::default();
        let shared = armed.clone();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else { return };
                tokio::spawn(Self::connection(client, target, shared.clone()));
            }
        });
        Self { addr, armed }
    }

    fn arm(&self, fault: Fault) {
        *self.armed.lock().unwrap() = Some(fault);
    }

    fn take(armed: &std::sync::Mutex<Option<Fault>>, wanted: impl Fn(Fault) -> bool) -> bool {
        let mut slot = armed.lock().unwrap();
        if slot.is_some_and(wanted) {
            *slot = None;
            true
        } else {
            false
        }
    }

    async fn connection(client: TcpStream, target: std::net::SocketAddr, armed: Arc<std::sync::Mutex<Option<Fault>>>) {
        let Ok(server) = TcpStream::connect(target).await else { return };
        let (mut client_read, mut client_write) = client.into_split();
        let (mut server_read, mut server_write) = server.into_split();
        let drop_ack = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let forward = {
            let (armed, drop_ack) = (armed.clone(), drop_ack.clone());
            async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    let Ok(n) = client_read.read(&mut buf).await else { return };
                    if n == 0 {
                        return;
                    }
                    // Packets are 3 length bytes, 1 sequence byte and the payload; a command payload starts with its command byte.
                    let (mut at, mut commit, mut statement) = (0usize, false, String::new());
                    while at + 4 < n {
                        let len = usize::from(buf[at]) | usize::from(buf[at + 1]) << 8 | usize::from(buf[at + 2]) << 16;
                        if at + 4 + len > n {
                            break;
                        }
                        // COM_QUERY (0x03) carries text; the driver prepares statements with COM_STMT_PREPARE (0x16).
                        if matches!(buf[at + 4], 0x03 | 0x16) {
                            let text = String::from_utf8_lossy(&buf[at + 5..at + 4 + len]).to_string();
                            if buf[at + 4] == 0x03 && text.trim().eq_ignore_ascii_case("COMMIT") {
                                commit = true;
                            }
                            statement.push_str(&text);
                        }
                        at += 4 + len;
                    }
                    if commit && Self::take(&armed, |f| f == Fault::CutOnCommit) {
                        return;
                    }
                    if !statement.is_empty() && Self::take(&armed, |f| matches!(f, Fault::CutOnStatement(text) if statement.contains(text))) {
                        return;
                    }
                    if commit && Self::take(&armed, |f| f == Fault::DropAckAfterCommit) {
                        drop_ack.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    if server_write.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            }
        };
        let backward = async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let Ok(n) = server_read.read(&mut buf).await else { return };
                if n == 0 || drop_ack.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                if client_write.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        };
        // Either direction ending closes both: the dropped halves reset the sockets.
        tokio::select! { _ = forward => {}, _ = backward => {} }
    }
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
    conn.query_drop(format!("ALTER TABLE `{SOURCE}`.`log` MODIFY `title` VARCHAR(254) NOT NULL")).await.unwrap();
    let drifted = h.apply(&h.owner, "after-drift", json!({"apply": "Note.hit", "target": {"ids": ["11"]}})).await;
    assert_eq!(drifted["code"], "WIRE.DEPLOYMENT_CHANGED", "{drifted}");
    assert_eq!(h.count(&format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = 11")).await, 0);
    conn.query_drop(format!("ALTER TABLE `{SOURCE}`.`log` MODIFY `title` VARCHAR(255) NOT NULL")).await.unwrap();
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

/// The three ways a connection can fail around COMMIT, and what each lets the client conclude (WBS 7.2.1.2 gate W4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_connection_loss_around_commit_is_reported_by_what_could_have_happened() {
    let _turn = SERIAL.lock().await;
    let admin_url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL");
    let target = admin_url.split_once('@').unwrap().1.split('/').next().unwrap().parse().unwrap();
    let proxy = MysqlProxy::start(target).await;
    let h = harness_via(Some(&proxy)).await;
    let hit = |id: &str| json!({"apply": "Note.hit", "target": {"ids": [id]}});
    let hits = |id: i64| format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = {id}");
    let journal = |key: &str| format!("SELECT COUNT(*) FROM `{META}`.`aip_idem` WHERE idem_key = '{key}'");

    // The statement is cut before COMMIT: nothing committed and the client may simply send the same key again.
    proxy.arm(Fault::CutOnStatement("UPDATE"));
    let cut = h.apply(&h.owner, "w4-c", hit("11")).await;
    assert_eq!(cut["code"], "WIRE.DB_UNAVAILABLE", "{cut}");
    assert_eq!((h.count(&hits(11)).await, h.count(&journal("w4-c")).await), (0, 0));
    let retried = h.apply(&h.owner, "w4-c", hit("11")).await;
    assert_eq!((retried["ok"].clone(), retried["replayed"].clone()), (json!(true), Value::Null), "{retried}");
    assert_eq!(h.count(&hits(11)).await, 1);

    // COMMIT never reaches the server: the connection drop rolls the transaction back, yet the client cannot know that.
    proxy.arm(Fault::CutOnCommit);
    let unknown = h.apply(&h.owner, "w4-a", hit("11")).await;
    assert_eq!(unknown["code"], "WIRE.COMMIT_UNKNOWN", "{unknown}");
    assert_eq!((h.count(&hits(11)).await, h.count(&journal("w4-a")).await), (1, 0), "server rolled back");
    let resolved = h.apply(&h.owner, "w4-a", hit("11")).await;
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(h.count(&hits(11)).await, 2, "the retry did the write");

    // The server commits and the acknowledgement is lost: the same key replays the committed result and writes nothing more.
    proxy.arm(Fault::DropAckAfterCommit);
    let lost = h.apply(&h.owner, "w4-b", hit("11")).await;
    assert_eq!(lost["code"], "WIRE.COMMIT_UNKNOWN", "{lost}");
    assert_eq!((h.count(&hits(11)).await, h.count(&journal("w4-b")).await), (3, 1), "server committed");
    let replayed = h.apply(&h.owner, "w4-b", hit("11")).await;
    assert_eq!(
        (replayed["ok"].clone(), replayed["replayed"].clone(), replayed["changed"].clone()),
        (json!(true), json!(true), json!(["11"])),
        "{replayed}"
    );
    assert_eq!(h.count(&hits(11)).await, 3, "no second effect");
}

const TEAMS: &str = r#"
enum Role { OWNER, MEMBER }
enum RequestState { PENDING, APPROVED }
resource User { fields { id: Uuid; name: Text(1..20) } rows read when true }
actor User
resource Team { fields { id: Uuid; name: Text(1..20) } rows read when true }
resource Membership { fields { id: Uuid; team: Team; user: User; role: Role } rows read when true }
predicate ownerOf(u: User, t: Team) = exists Membership where team = t and user = u and role = OWNER
predicate slotTaken(s: Slot) = exists Booking where slot = s
resource Flag { fields { id: Uuid; post: Post; raised: Bool default database } rows read when false }
resource Post {
  fields { id: Uuid; team: Team; author: User; title: Text(1..20); pinned: Bool default database; likes: Int default database }
  rows read when true
  transition pin { from pinned = false; allow ownerOf(actor, team); to pinned = true
    update many Flag maxRows 3 where post = this.id and raised = true { raised = false } }
  transition like { from true; allow actor != null; to likes = likes + 1 }
  expose apply pin { target id; bulk maxRows 5 }
  expose apply like { target id; bulk maxRows 5 }
  expose create { allow author = actor; fields team, author, title }
  expose read { select id, title, pinned, likes; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Slot { fields { id: Uuid; name: Text(1..20) } rows read when true }
resource Booking { fields { id: Uuid; slot: Slot; user: User } rows read when true }
resource Request {
  fields { id: Uuid; slot: Slot; user: User; state: RequestState default database }
  rows read when true
  transition approve { from state = PENDING; allow not slotTaken(slot)
    to state = APPROVED
    create Booking { slot = this.slot; user = this.user } }
  expose apply approve { target id; bulk maxRows 1 }
}
"#;

fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// UUID resources and actors, role policies that read other rows (locked FOR SHARE), the `update many` effect, and a policy
/// that needs a row to be absent, which MySQL runs SERIALIZABLE and retries.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_uuid_resources_cross_row_policies_effects_and_absence_races() {
    let _turn = SERIAL.lock().await;
    let (u1, u2, team, post1, post2) = (uuid(1), uuid(2), uuid(10), uuid(20), uuid(21));
    let mut seed = vec![
        format!("INSERT INTO `{{SOURCE}}`.`user` (id, name) VALUES ('{u1}', 'owner'), ('{u2}', 'member')"),
        format!("INSERT INTO `{{SOURCE}}`.`team` (id, name) VALUES ('{team}', 'team')"),
        format!(
            "INSERT INTO `{{SOURCE}}`.`membership` (id, team_id, user_id, role) VALUES ('{}', '{team}', '{u1}', 'OWNER'), ('{}', '{team}', '{u2}', 'MEMBER')",
            uuid(30),
            uuid(31)
        ),
        format!(
            "INSERT INTO `{{SOURCE}}`.`post` (id, team_id, author_id, title, pinned, likes) VALUES ('{post1}', '{team}', '{u2}', 'one', 0, 0), ('{post2}', '{team}', '{u2}', 'two', 0, 0)"
        ),
        format!(
            "INSERT INTO `{{SOURCE}}`.`flag` (id, post_id, raised) VALUES ('{}', '{post1}', 1), ('{}', '{post1}', 1), ('{}', '{post2}', 1)",
            uuid(40),
            uuid(41),
            uuid(42)
        ),
        format!("INSERT INTO `{{SOURCE}}`.`slot` (id, name) VALUES ('{}', 'slot')", uuid(50)),
    ];
    for n in 0..8 {
        seed.push(format!(
            "INSERT INTO `{{SOURCE}}`.`request` (id, slot_id, user_id, state) VALUES ('{}', '{}', '{}', 'PENDING')",
            uuid(100 + n),
            uuid(50),
            if n % 2 == 0 { &u1 } else { &u2 }
        ));
    }
    let h = Arc::new(harness_with(TEAMS, &seed, "SELECT, INSERT, UPDATE, DELETE", None).await);
    // Actors are UUID resources: a numeric handle per actor lives in aip_actor_identities, and tokens carry the handle.
    h.sql(&format!("INSERT INTO `{META}`.`aip_actor_identities` (resource_id) VALUES ('{u1}'), ('{u2}')")).await;
    assert_eq!(h.count(&format!("SELECT id FROM `{META}`.`aip_actor_identities` WHERE resource_id = '{u1}'")).await, 1);
    let (owner, member) = (h.owner.clone(), h.other.clone());

    // Role policy reads another table; the member is not an owner.
    let pin = |id: &str| json!({"apply": "Post.pin", "target": {"ids": [id]}});
    assert_eq!(h.apply(&member, "pin-denied", pin(&post1)).await["code"], "WIRE.FORBIDDEN");
    let pinned = h.apply(&owner, "pin-1", pin(&post1)).await;
    assert_eq!((pinned["ok"].clone(), pinned["changed"].clone()), (json!(true), json!([post1.clone()])), "{pinned}");
    // The `update many` effect cleared exactly this post's flags, and nothing else.
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`flag` WHERE post_id = '{post1}' AND raised = 0")).await, 2);
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`flag` WHERE post_id = '{post2}' AND raised = 1")).await, 1);
    assert_eq!(h.apply(&owner, "pin-again", pin(&post1)).await["code"], "WIRE.INVALID_STATE");

    // Create with an engine-generated UUID; allow `author = actor` refuses a forged author.
    let create = |author: &str| json!({"atomic": [{"create": "Post", "values": {"team": team, "author": author, "title": "fresh"}}]});
    let made = h.apply(&member, "create-1", create(&u2)).await;
    assert_eq!(made["ok"], true, "{made}");
    let new_id = made["created"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(new_id.len(), 36, "{made}");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`post` WHERE id = '{new_id}' AND author_id = '{u2}' AND title = 'fresh'")).await, 1);
    let forged = h.apply(&member, "create-forged", create(&u1)).await;
    assert_eq!(forged["code"], "WIRE.FORBIDDEN", "{forged}");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`post` WHERE title = 'fresh'")).await, 1, "the refused create left no row");

    // Increments by UUID target.
    let like = h.apply(&member, "like-1", json!({"apply": "Post.like", "target": {"ids": [post2.clone()]}})).await;
    assert_eq!(like["ok"], true, "{like}");
    assert_eq!(h.count(&format!("SELECT likes FROM `{SOURCE}`.`post` WHERE id = '{post2}'")).await, 1);

    // The read path shows the same rows through the UUID wire.
    let read = post(
        h.addr,
        "/read",
        Some(&member),
        &h.contract,
        &json!({"query": {"read": "Post", "select": ["id", "title", "pinned", "likes"], "sort": [{"field": "id", "dir": "asc"}]}}),
    )
    .await;
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["rows"][0]["id"], json!(post1), "{read}");
    assert_eq!(read["rows"][0]["pinned"], true);

    // Absence race: eight requests for one slot, approved at once. A policy that needs a row to be absent runs SERIALIZABLE
    // and the server retries the deadlock victim, so exactly one booking ever exists.
    let mut joins = vec![];
    for n in 0..8u32 {
        let (h, token, id) = (h.clone(), if n % 2 == 0 { owner.clone() } else { member.clone() }, uuid(100 + n));
        joins.push(tokio::spawn(async move {
            h.apply(&token, &format!("approve-{n}"), json!({"apply": "Request.approve", "target": {"ids": [id]}})).await
        }));
    }
    let mut approved = 0;
    for join in joins {
        let answer = join.await.unwrap();
        if answer["ok"] == true {
            approved += 1;
        } else {
            assert!(matches!(answer["code"].as_str(), Some("WIRE.FORBIDDEN" | "WIRE.CONFLICT")), "{answer}");
        }
    }
    assert_eq!(approved, 1, "exactly one request wins the slot");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`booking`")).await, 1, "no double booking");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`request` WHERE state = 'APPROVED'")).await, 1);
}

/// A read-only business grant is enough to serve reads and never enough to write: the failure names the missing grant and
/// nothing is committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_write_without_business_grants_names_the_missing_privilege() {
    let _turn = SERIAL.lock().await;
    let seed = [
        "INSERT INTO `{SOURCE}`.`member` (id) VALUES (1), (2)".to_string(),
        "INSERT INTO `{SOURCE}`.`note` (id, owner_id, title, version, done, hits) VALUES (10, 1, 'before', 0, 0, 0)".to_string(),
    ];
    let h = harness_with(DEFINITION, &seed, "SELECT", None).await;
    let read = post(
        h.addr,
        "/read",
        Some(&h.owner),
        &h.contract,
        &json!({"query": {"read": "Note", "select": ["id", "title"], "sort": [{"field": "id", "dir": "asc"}]}}),
    )
    .await;
    assert_eq!(read["rows"][0]["title"], "before", "{read}");
    let denied = h.apply(&h.owner, "no-grant", json!({"apply": "Note.hit", "target": {"ids": ["10"]}})).await;
    assert_eq!(denied["code"], "WIRE.INTERNAL", "{denied}");
    assert!(denied["msg"].as_str().unwrap().contains("INSERT·UPDATE·DELETE 권한"), "{denied}");
    assert_eq!(h.count(&format!("SELECT hits FROM `{SOURCE}`.`note` WHERE id = 10")).await, 0);
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{META}`.`aip_idem`")).await, 0);
}

const TASKS: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Project {
  fields { id: Id; name: Text; touched: Bool default database }
  rows read when true
  expose read { select id, name, touched; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
resource Task {
  fields { id: Id; project: Project; owner: Member; tag: Text(1..8); done: Bool default database }
  rows read when true
  transition complete { from done = false; allow owner = actor; to done = true; update Project where id = project { touched = true } }
  transition sweep { from done = false; allow owner = actor; to done = true }
  expose apply complete { target id; bulk maxRows 3 }
  expose apply sweep { target id, where; bulk maxRows 3; sameScope project }
  expose read { select id, project, owner, tag, done; filter project.eq, done.eq, tag.eq; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
"#;

/// A `where` target, `sameScope` and the single-row `update` effect, none of which the other tests reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn mysql_where_target_same_scope_and_single_update_effect() {
    let _turn = SERIAL.lock().await;
    let seed = [
        "INSERT INTO `{SOURCE}`.`member` (id) VALUES (1), (2)".to_string(),
        "INSERT INTO `{SOURCE}`.`project` (id, name, touched) VALUES (1, 'a', 0), (2, 'b', 0)".to_string(),
        "INSERT INTO `{SOURCE}`.`task` (id, project_id, owner_id, tag, done) VALUES \
         (10, 1, 1, 'x', 0), (11, 1, 1, 'x', 0), (12, 2, 1, 'solo', 0), (13, 1, 2, 'x', 0), (14, 1, 1, 'x', 1), (15, 2, 1, 'x', 0)"
            .to_string(),
    ];
    let h = harness_with(TASKS, &seed, "SELECT, INSERT, UPDATE, DELETE", None).await;
    let touched = |project: i64| format!("SELECT touched FROM `{SOURCE}`.`project` WHERE id = {project}");
    let done = |task: i64| format!("SELECT done FROM `{SOURCE}`.`task` WHERE id = {task}");
    let owner = h.owner.clone();
    let sweep = |target: Value| json!({"apply": "Task.sweep", "target": target});

    // `where` picks the rows `from` lets change now: tag x and still open are 10, 11, 13 and 15, more than the cap of 3.
    let wide = h.apply(&owner, "where-1", sweep(json!({"where": [{"field": "tag", "op": "eq", "value": "x"}]}))).await;
    // The rows span two projects, so `sameScope` refuses it before the row cap is reached.
    assert_eq!(wide["code"], "WIRE.NOT_SAME_SCOPE", "{wide}");
    assert_eq!(h.count(&format!("SELECT COUNT(*) FROM `{SOURCE}`.`task` WHERE done = 0")).await, 5, "a refused request changes nothing");

    // A reference field is not an allowed `where` condition: the caller names rows by id instead.
    let by_reference = h.apply(&owner, "where-ref", sweep(json!({"where": [{"field": "project", "op": "eq", "value": "1"}]}))).await;
    assert_eq!(by_reference["code"], "WIRE.FILTER_NOT_ALLOWED", "{by_reference}");

    // A `where` that matches exactly one row.
    let single = h.apply(&owner, "where-2", sweep(json!({"where": [{"field": "tag", "op": "eq", "value": "solo"}]}))).await;
    assert_eq!(single["ok"], true, "{single}");
    assert_eq!(single["changed"], json!(["12"]), "{single}");
    assert_eq!(h.count(&done(12)).await, 1);
    let replay = h.apply(&owner, "where-2", sweep(json!({"where": [{"field": "tag", "op": "eq", "value": "solo"}]}))).await;
    assert_eq!(replay["replayed"], true, "{replay}");

    // Two tasks of one project would update the same project row twice. The effect needs one target row per task, so the whole
    // request is refused and both tasks stay open (the PostgreSQL path has the same rule).
    let twice = h.apply(&owner, "complete-twice", json!({"apply": "Task.complete", "target": {"ids": ["10", "11"]}})).await;
    assert_eq!(twice["code"], "WIRE.EFFECT_TARGET_MISSING", "{twice}");
    assert_eq!((h.count(&done(10)).await, h.count(&done(11)).await, h.count(&touched(1)).await), (0, 0, 0), "nothing of it was kept");

    // `sameScope project`: two tasks of one project pass, two projects in one request are refused though both are the caller's.
    let mixed = h.apply(&owner, "scope-2", sweep(json!({"ids": ["11", "15"]}))).await;
    assert_eq!(mixed["code"], "WIRE.NOT_SAME_SCOPE", "{mixed}");
    assert_eq!((h.count(&done(11)).await, h.count(&done(15)).await), (0, 0), "a refused request changes nothing");
    let same = h.apply(&owner, "scope-1", sweep(json!({"ids": ["10", "11"]}))).await;
    assert_eq!((same["ok"].clone(), same["changed"].clone()), (json!(true), json!(["10", "11"])), "{same}");

    // The single-row `update` effect: one task, one project row, marked in the same transaction.
    let completed = h.apply(&owner, "complete-1", json!({"apply": "Task.complete", "target": {"ids": ["15"]}})).await;
    assert_eq!(completed["ok"], true, "{completed}");
    assert_eq!(h.count(&touched(2)).await, 1, "the effect marked the project");
    assert_eq!(h.count(&touched(1)).await, 0, "and only the project it names");
    let again = h.apply(&owner, "complete-2", json!({"apply": "Task.complete", "target": {"ids": ["15"]}})).await;
    assert_ne!(again["ok"], true, "a finished task is no longer a target: {again}");
}
