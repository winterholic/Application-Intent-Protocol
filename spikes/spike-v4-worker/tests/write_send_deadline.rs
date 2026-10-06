use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v4_worker::{write::invoke_write, Isolation, Lang, Worker, WorkerLimits};
use std::{fs, path::PathBuf, time::Duration};
use tokio::time::timeout;

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Event {
  fields { id: Id; member: Member; checked: Bool }
  rows read when member = actor
  transition mark { allow member = actor; from checked = false; to checked = true }
  expose apply mark { target id; bulk maxRows 2 }
  extension write blocked {
    input { value: Text }
    output { done: Bool }
    access Event.mark
    effect db
    deadline 1s
    implementation "blocked.run"
  }
}
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_write_input_has_a_deadline_and_discards_partial_protocol() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-write-send-{unique}"));
    fs::create_dir(&path).unwrap();
    let owned = OwnedDir(path);
    fs::write(
        owned.0.join("blocked.mjs"),
        "export async function run(input, ctx) { await ctx.data.apply('Event', 'mark', ['11']); while (true) {} }\n",
    )
    .unwrap();
    fs::write(
        owned.0.join("blocked.py"),
        "async def run(input, ctx):\n    await ctx.data.apply('Event', 'mark', ['11'])\n    while True:\n        pass\n",
    )
    .unwrap();
    let schema = format!("aip_write_send_{unique}");
    sqlgen::set_schema(&schema);
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL");
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures = vec![];
    let result: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.event(id,member_id,checked) VALUES (11,1,false)"
        ))
        .await?;
        let caller = Caller { actor_id: Some(1), now: "2026-10-05T00:00:00Z".into() };
        for lang in [Lang::Node, Lang::Python] {
            let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
            let mut worker = match Worker::try_start_with(lang, owned.0.to_str().unwrap(), isolation, WorkerLimits::default()).await {
                Ok(worker) => worker,
                Err(error) => {
                    failures.push(format!("{lang:?} startup: {:?}", error.kind()));
                    continue;
                }
            };
            let first =
                timeout(Duration::from_secs(3), invoke_write(&mut db, &mut worker, &facts, "Event.blocked", &json!({"value":"small"}), &caller))
                    .await;
            if !matches!(&first, Ok(Err(error)) if error.code == "DEADLINE_EXCEEDED") {
                failures.push(format!("{lang:?} initial CPU deadline: {first:?}"));
            }
            let large = json!({"value":"x".repeat(4 * 1024 * 1024)});
            let second = timeout(Duration::from_millis(2500), invoke_write(&mut db, &mut worker, &facts, "Event.blocked", &large, &caller)).await;
            if !matches!(&second, Ok(Err(error)) if error.code == "DEADLINE_EXCEEDED") {
                failures.push(format!("{lang:?} blocked input deadline: {second:?}"));
            }
            let third =
                timeout(Duration::from_millis(500), invoke_write(&mut db, &mut worker, &facts, "Event.blocked", &json!({"value":"small"}), &caller))
                    .await;
            if !matches!(&third, Ok(Err(error)) if error.code == "WORKER_FAILED") {
                failures.push(format!("{lang:?} partial protocol reused: {third:?}"));
            }
            worker.stop().await;
            let checked: bool = db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=11"), &[]).await?.get(0);
            if checked {
                failures.push(format!("{lang:?} timed-out ctx write committed"));
            }
        }
        Ok(())
    }
    .await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(result.is_ok(), "database execution: {result:?}");
    assert!(cleanup.is_ok(), "owned schema cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
