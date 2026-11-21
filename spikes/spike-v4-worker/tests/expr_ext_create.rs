//! P0-C: `ctx.data.create` of a PostgreSQL write extension (access `create.<Resource>`) against a real PostgreSQL.
//! It runs the caller's exposed create as a one-operation bundle in the extension's own transaction, so the client's create contract
//! holds (field allow-list, `expose create allow`, readable references) and a refusal poisons the call.
//! - C-S1..3: a created id comes back, a child can use the parent's id, an absence-dependent create runs SERIALIZABLE.
//! - C-F1..5, R-2: refused creates and a later failure leave nothing behind; the cause tag names `ctx.create.<cause>`.
//! - K-4: two extensions racing on a unique create: one commits, the other ends with `WIRE.ALREADY_EXISTS`.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};
use spike_v3_write::{bundle, Knobs};
use spike_v4_worker::{
    write::{prepare_write_in_with_wire, write_needs_serializable},
    Isolation, Lang, Worker, WorkerLimits,
};
use std::{fs, path::PathBuf, time::Duration};
use tokio_postgres::Client;

const DB: &str = aip_test_support::test_database_url!();
const NODE: &str = include_str!("fixtures/expr-ext-create/ex.mjs");

const SPEC: &str = r#"
resource Member { fields { id: Id; org: Int } }
actor Member
resource Task { fields { id: Id; org: Int; title: Text } rows read when org = actor.org }
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Slot where room = r and taken = true
resource Slot {
  fields { id: Id; room: Room; owner: Member; taken: Bool }
  rows read when true
  expose create { allow roomFree(this.room); fields room, owner, taken }
}
resource Secret {
  fields { id: Id; owner: Member; value: Text(1..100) encrypted(scope: service) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, value }
}
resource Tag {
  fields { id: Id; owner: Member; name: Text(1..20) }
  rows read when owner = actor
  unique owner, name
  expose create { allow owner = actor; fields owner, name }
}
resource Note {
  fields { id: Id; task: Task; owner: Member; body: Text(1..50) }
  rows read when owner = actor
  expose create { allow owner = actor; fields task, owner, body }
}
resource Item {
  fields { id: Id; project: Project; owner: Member; label: Text(1..40); secret: Text? }
  rows read when owner = actor
  expose create { allow owner = actor; fields project, owner, label }
}
resource Project {
  fields { id: Id; owner: Member; name: Text(1..40) }
  rows read when owner = actor
  expose create { allow owner = actor and actor.org > 0; fields owner, name }
  extension write createProject { input { owner: Member.Id; name: Text(1..40) } output { id: Project.Id } access create.Project effect db deadline 5s implementation "ex.createProject" }
  extension write family { input { owner: Member.Id; name: Text(1..40) } output { project: Project.Id; item: Item.Id } access create.Project, create.Item effect db deadline 5s implementation "ex.family" }
  extension write familyThenThrow { input { owner: Member.Id; name: Text(1..40) } output { project: Project.Id; item: Item.Id } access create.Project, create.Item effect db deadline 5s implementation "ex.familyThenThrow" }
  extension write swallow { input { owner: Member.Id; name: Text(1..40) } output { project: Project.Id; item: Item.Id } access create.Project, create.Item effect db deadline 5s implementation "ex.swallow" }
  extension write noGrant { input { owner: Member.Id; name: Text(1..40) } output { ok: Bool } access create.Project effect db deadline 5s implementation "ex.noGrant" }
  extension write badField { input { owner: Member.Id; name: Text(1..40) } output { ok: Bool } access create.Project, create.Item effect db deadline 5s implementation "ex.badField" }
  extension write hiddenRef { input { owner: Member.Id; name: Text(1..40) } output { ok: Bool } access create.Note effect db deadline 5s implementation "ex.hiddenRef" }
  extension write badKey { input { owner: Member.Id; name: Text(1..40) } output { ok: Bool } access create.Project effect db deadline 5s implementation "ex.badKey" }
  extension write slot { input { owner: Member.Id; name: Text(1..40) } output { id: Slot.Id } access create.Slot effect db deadline 5s implementation "ex.slot" }
  extension write slotLater { input { owner: Member.Id; name: Text(1..40) } output { id: Slot.Id } access create.Slot effect db deadline 5s implementation "ex.slotLater" }
  extension write secret { input { owner: Member.Id; name: Text(1..40) } output { id: Secret.Id } access create.Secret effect db deadline 5s implementation "ex.secret" }
  extension write tag { input { owner: Member.Id; name: Text(1..40) } output { id: Tag.Id } access create.Tag effect db deadline 5s implementation "ex.tag" }
}
"#;

const TABLES: [&str; 6] = ["project", "item", "note", "slot", "tag", "secret"];

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Env {
    facts: Value,
    db: Client,
    schema: String,
    worker: Worker,
    _dir: OwnedDir,
}

fn caller(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-11-21T00:00:00Z".into() }
}

/// `[tag] text` of a `Reject.msg`, written out here so the test does not depend on the helper it checks.
fn tag_of(msg: &str) -> Option<&str> {
    msg.strip_prefix('[')?.split_once("] ").map(|(tag, _)| tag)
}

async fn start_worker(dir: &std::path::Path) -> Worker {
    let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
    Worker::try_start_with(Lang::Node, dir.to_str().unwrap(), isolation, WorkerLimits::default()).await.expect("official worker")
}

async fn setup() -> Env {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-expr-ext-create-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(owned.0.join("ex.mjs"), NODE).unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!("aip_expr_ext_create_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DB).await.expect("local PostgreSQL");
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    // Members 1 and 2 are in orgs 1 and 2, member 3 has org 0 (the Project create allow needs org > 0). Task 20 is org 2's.
    // `seen` records the isolation level an INSERT into a created table ran under.
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,org) VALUES (1,1),(2,2),(3,0);
         INSERT INTO {schema}.task(id,org,title) VALUES (10,1,'mine'),(20,2,'theirs');
         INSERT INTO {schema}.room(id,name) VALUES (1,'a');
         CREATE TABLE {schema}.seen(tbl text NOT NULL, level text NOT NULL);
         CREATE FUNCTION {schema}.capture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO {schema}.seen VALUES(TG_TABLE_NAME, current_setting('transaction_isolation')); RETURN NEW; END $$;
         CREATE TRIGGER capture BEFORE INSERT ON {schema}.slot FOR EACH ROW EXECUTE FUNCTION {schema}.capture();
         CREATE TRIGGER capture BEFORE INSERT ON {schema}.project FOR EACH ROW EXECUTE FUNCTION {schema}.capture()"
    ))
    .await
    .unwrap();
    let worker = start_worker(&owned.0).await;
    Env { facts, db, schema, worker, _dir: owned }
}

impl Env {
    async fn counts(&self) -> Vec<i64> {
        let mut counts = vec![];
        for table in TABLES {
            counts.push(self.db.query_one(&format!("SELECT count(*) FROM {}.{table}", self.schema), &[]).await.unwrap().get(0));
        }
        counts
    }

    /// The extension in its own transaction, committed only when the call succeeds, the way an adapter owning the transaction does.
    async fn run(&mut self, extension: &str, who: i64, input: Value) -> Result<Value, spike_v2_read::plan::Reject> {
        let tx = self.db.transaction().await.unwrap();
        let result = prepare_write_in_with_wire(
            &tx,
            &mut self.worker,
            &self.facts,
            &format!("Project.{extension}"),
            &input,
            &caller(who),
            IdWire::DecimalString,
        )
        .await;
        match result {
            Ok(prepared) => {
                tx.commit().await.unwrap();
                Ok(prepared.output)
            }
            Err(error) => {
                tx.rollback().await.unwrap();
                Err(error)
            }
        }
    }

    async fn reset(&self) {
        let tables = TABLES.map(|table| format!("{}.{table}", self.schema)).join(", ");
        self.db.batch_execute(&format!("TRUNCATE {tables}; TRUNCATE {}.seen", self.schema)).await.unwrap();
    }

    async fn drop_schema(self) {
        self.worker.stop().await;
        self.db.batch_execute(&format!("DROP SCHEMA {} CASCADE", self.schema)).await.unwrap();
    }
}

async fn an_extension_creates_a_row_and_a_child_with_the_parent_id(env: &mut Env) {
    let input = json!({"owner": "1", "name": "plan"});

    // C-S1: the id of the created row comes back and the whole call committed.
    let out = env.run("createProject", 1, input.clone()).await.unwrap_or_else(|e| panic!("{e:?}"));
    let id = out["id"].as_str().expect("a created id").to_string();
    let stored: (i64, String) = env
        .db
        .query_one(&format!("SELECT owner_id, name FROM {}.project WHERE id = {id}", env.schema), &[])
        .await
        .map(|r| (r.get(0), r.get(1)))
        .unwrap();
    assert_eq!(stored, (1, "plan".to_string()));
    assert_eq!(env.counts().await, vec![1, 0, 0, 0, 0, 0]);

    // C-S2: the child refers to the id the parent create returned; both commit.
    let out = env.run("family", 1, input).await.unwrap_or_else(|e| panic!("{e:?}"));
    let (project, item) = (out["project"].as_str().unwrap().to_string(), out["item"].as_str().unwrap().to_string());
    assert_ne!(project, id, "a new parent row");
    let parent_of_item: i64 = env.db.query_one(&format!("SELECT project_id FROM {}.item WHERE id = {item}", env.schema), &[]).await.unwrap().get(0);
    assert_eq!(parent_of_item.to_string(), project);
    assert_eq!(env.counts().await, vec![2, 1, 0, 0, 0, 0]);
}

async fn an_absence_dependent_create_grant_runs_the_extension_serializable(env: &mut Env) {
    // The marker is read from the create grant: `slot` (absence in the create allow) needs SERIALIZABLE, `createProject` does not.
    assert_eq!(env.facts["resources"]["Slot"]["exposeCreate"]["isolation"], "serializable");
    assert!(write_needs_serializable(&env.facts, "Project.slot"));
    assert!(!write_needs_serializable(&env.facts, "Project.createProject"));
    assert!(!write_needs_serializable(&env.facts, "Project.family"));

    // C-S3: what the transaction ran under, seen from inside the INSERT.
    env.run("slot", 1, json!({"owner": "1", "name": "n"})).await.unwrap_or_else(|e| panic!("{e:?}"));
    env.run("createProject", 1, json!({"owner": "1", "name": "n"})).await.unwrap_or_else(|e| panic!("{e:?}"));
    let seen: Vec<(String, String)> = env
        .db
        .query(&format!("SELECT tbl, level FROM {}.seen ORDER BY tbl", env.schema), &[])
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    assert_eq!(seen, vec![("project".to_string(), "read committed".to_string()), ("slot".to_string(), "serializable".to_string())]);
}

async fn a_refused_create_or_a_later_failure_commits_nothing_and_names_its_cause(env: &mut Env) {
    let input = json!({"owner": "1", "name": "plan"});
    // (extension, caller, input, cause tag, the same refusal a client create gets as `(request, code)`)
    let cases = [
        // R-2: both rows existed in the transaction when the extension threw.
        ("familyThenThrow", 1, input.clone(), "extension.exception", None),
        // C-F4: the extension caught the refusal and returned normally; the call still does not commit, parent and child included.
        ("swallow", 1, input.clone(), "ctx.create.field_not_exposed", None),
        // C-F1: no `create.Item` in access.
        ("noGrant", 1, input.clone(), "ctx.create.access_not_declared", None),
        // C-F2: `secret` is not in `expose create fields`.
        (
            "badField",
            1,
            input.clone(),
            "ctx.create.field_not_exposed",
            Some((json!({"atomic":[{"create":"Item","values":{"project":"1","owner":"1","label":"x","secret":"s"}}]}), "WIRE.FIELD_NOT_EXPOSED")),
        ),
        // C-F3: the create `allow` is false for member 3 (org 0).
        (
            "createProject",
            3,
            json!({"owner": "3", "name": "plan"}),
            "ctx.create.forbidden",
            Some((json!({"atomic":[{"create":"Project","values":{"owner":"3","name":"plan"}}]}), "WIRE.FORBIDDEN")),
        ),
        // A reference the caller cannot read: member 2's row (E4), and org 2's task.
        (
            "createProject",
            1,
            json!({"owner": "2", "name": "plan"}),
            "ctx.create.missing_target",
            Some((json!({"atomic":[{"create":"Project","values":{"owner":"2","name":"plan"}}]}), "WIRE.MISSING_TARGET")),
        ),
        (
            "hiddenRef",
            1,
            input.clone(),
            "ctx.create.missing_target",
            Some((json!({"atomic":[{"create":"Note","values":{"task":"20","owner":"1","body":"org 2 task"}}]}), "WIRE.MISSING_TARGET")),
        ),
        // Review C-4: a resource with an encrypted field cannot be created from an extension (no sealer in this transaction).
        ("secret", 1, input.clone(), "ctx.create.unsupported", None),
        // C-F5: a key outside the documented form.
        ("badKey", 1, input.clone(), "ctx.create.key_format", None),
    ];
    let mut failures = vec![];
    for (extension, who, extension_input, tag, client) in cases {
        let result = env.run(extension, who, extension_input).await;
        match &result {
            Err(error) if error.code == "WIRE.EXTENSION_ERROR" && tag_of(&error.msg) == Some(tag) => {}
            other => failures.push(format!("{extension}: expected EXTENSION_ERROR tagged {tag}, got {other:?}")),
        }
        let counts = env.counts().await;
        if counts != vec![0; 6] {
            failures.push(format!("{extension}: left rows behind {counts:?}"));
        }
        if let Some((request, code)) = client {
            // The client create must meet the very same refusal (same contract, same principal).
            let by_client = bundle(&mut env.db, &env.facts, &request, &caller(who), &Knobs::default()).await;
            if by_client.as_ref().err().map(|e| e.code) != Some(code) {
                failures.push(format!("{extension}: a client create gave {by_client:?}, expected {code}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

async fn two_extensions_creating_the_same_unique_row_one_wins(first: &mut Env) {
    let schema = first.schema.clone();
    let mut second_db = connect_with_url(DB).await.unwrap();
    let mut second_worker = start_worker(&first._dir.0).await;
    let input = json!({"owner": "1", "name": "same"});
    let facts = first.facts.clone();
    let winner = async {
        let tx = first.db.transaction().await.unwrap();
        let prepared = prepare_write_in_with_wire(&tx, &mut first.worker, &facts, "Project.tag", &input, &caller(1), IdWire::DecimalString)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Hold the unique index entry long enough for the other call to wait on it.
        tokio::time::sleep(Duration::from_millis(600)).await;
        tx.commit().await.unwrap();
        prepared.output
    };
    let loser = async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let tx = second_db.transaction().await.unwrap();
        let result = prepare_write_in_with_wire(&tx, &mut second_worker, &facts, "Project.tag", &input, &caller(1), IdWire::DecimalString).await;
        let _ = tx.rollback().await;
        result.map(|prepared| prepared.output)
    };
    let (won, lost) = tokio::join!(winner, loser);
    assert!(won["id"].is_string(), "{won}");
    let lost = lost.expect_err("the second create meets the unique constraint");
    // The extension ended with the code of the refused create unchanged, so the caller gets `ALREADY_EXISTS` itself (review C-2).
    assert_eq!(lost.code, "WIRE.ALREADY_EXISTS", "{lost:?}");
    let tags: i64 = first.db.query_one(&format!("SELECT count(*) FROM {schema}.tag"), &[]).await.unwrap().get(0);
    assert_eq!(tags, 1);
    second_worker.stop().await;
}

/// Review C-2: a SERIALIZABLE create that loses to a concurrent commit ends the call with `WIRE.CONFLICT` (retry later), not with the
/// opaque `EXTENSION_ERROR`, and the loser leaves nothing behind.
async fn a_serialization_failure_in_ctx_create_is_a_retryable_conflict(first: &mut Env) {
    let schema = first.schema.clone();
    let mut second_db = connect_with_url(DB).await.unwrap();
    let mut second_worker = start_worker(&first._dir.0).await;
    let input = json!({"owner": "1", "name": "n"});
    let facts = first.facts.clone();
    let slow = async {
        let tx = first.db.transaction().await.unwrap();
        let result = prepare_write_in_with_wire(&tx, &mut first.worker, &facts, "Project.slotLater", &input, &caller(1), IdWire::DecimalString).await;
        let _ = tx.rollback().await;
        result.map(|prepared| prepared.output)
    };
    let quick = async {
        // Starts after the slow call's first create and commits before its second one.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let tx = second_db.transaction().await.unwrap();
        let prepared = prepare_write_in_with_wire(&tx, &mut second_worker, &facts, "Project.slot", &input, &caller(1), IdWire::DecimalString)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        tx.commit().await.unwrap();
        prepared.output
    };
    let (slow, quick) = tokio::join!(slow, quick);
    assert!(quick["id"].is_string(), "{quick}");
    let lost = slow.expect_err("the slow call read a state the quick one changed");
    assert_eq!(lost.code, "WIRE.CONFLICT", "{lost:?}");
    let slots: i64 = first.db.query_one(&format!("SELECT count(*) FROM {schema}.slot"), &[]).await.unwrap().get(0);
    assert_eq!(slots, 1, "only the quick call's row");
    second_worker.stop().await;
}

/// One schema per process (`sqlgen::set_schema`), so the scenarios share it and start from empty created tables.
#[tokio::test]
async fn ctx_data_create_of_a_postgres_write_extension() {
    let mut env = setup().await;
    an_extension_creates_a_row_and_a_child_with_the_parent_id(&mut env).await;
    env.reset().await;
    an_absence_dependent_create_grant_runs_the_extension_serializable(&mut env).await;
    env.reset().await;
    a_refused_create_or_a_later_failure_commits_nothing_and_names_its_cause(&mut env).await;
    env.reset().await;
    two_extensions_creating_the_same_unique_row_one_wins(&mut env).await;
    env.reset().await;
    a_serialization_failure_in_ctx_create_is_a_retryable_conflict(&mut env).await;
    env.drop_schema().await;
}
