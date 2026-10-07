use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v4_worker::{write::invoke_write, Isolation, Lang, Worker, WorkerLimits};
use std::{fs, path::PathBuf};

const SPEC: &str = include_str!("../../spike-v6-transport/tests/fixtures/write-extension.aip");
const DB: &str = "host=localhost dbname=postgres";

struct Owned(PathBuf);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn worker_write_pins_its_transaction_to_read_committed() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-worker-isolation-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = Owned(dir);
    fs::write(
        owned.0.join("write.mjs"),
        "export async function run(input,ctx){await ctx.data.apply('Event','mark',[input.id]);return {id:input.id,count:1};}\n",
    )
    .unwrap();

    let source = SPEC.replace("actor Member\n", "actor Member\nlimit maxChecked on Event = atMost 2 where checked = true\n").replace(
        " fields { id: Id; member: Member; checked: Bool }",
        "  fields { id: Id; member: Member; checked: Bool }\n invariant maxChecked per member",
    );
    let facts: Value = load_str(&source, Form::A).unwrap().execution;
    assert_eq!(facts["resources"]["Event"]["invariants"]["maxChecked"]["enforcement"]["kind"], "lockedCountCheck", "{facts}");
    let schema = format!("aip_isolation_v4_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(12,1,true),(13,1,false);
         CREATE TABLE {schema}.isolation_seen(level text NOT NULL);
         CREATE FUNCTION {schema}.capture_isolation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO {schema}.isolation_seen VALUES(current_setting('transaction_isolation')); RETURN NEW; END $$;
         CREATE TRIGGER capture_isolation BEFORE UPDATE ON {schema}.event FOR EACH ROW EXECUTE FUNCTION {schema}.capture_isolation();
         SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL REPEATABLE READ"
    )).await.unwrap();

    let caller = Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() };
    let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
    let mut worker = Worker::try_start_with(Lang::Node, owned.0.to_str().unwrap(), isolation, WorkerLimits::default()).await.unwrap();
    let result = invoke_write(&mut db, &mut worker, &facts, "Event.run", &json!({"id":"11"}), &caller).await;
    assert_eq!(result.unwrap(), json!({"id":"11","count":1}));
    let rejected = invoke_write(&mut db, &mut worker, &facts, "Event.run", &json!({"id":"13"}), &caller).await;
    let state: Vec<(i64, bool)> =
        db.query(&format!("SELECT id, checked FROM {schema}.event ORDER BY id"), &[]).await.unwrap().iter().map(|r| (r.get(0), r.get(1))).collect();
    assert_eq!(rejected.unwrap_err().code, "INVARIANT_VIOLATED", "state={state:?}");
    let level: String = db.query_one(&format!("SELECT level FROM {schema}.isolation_seen"), &[]).await.unwrap().get(0);
    assert_eq!(level, "read committed");
    worker.stop().await;
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
