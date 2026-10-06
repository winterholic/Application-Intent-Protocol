use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v4_worker::{write::prepare_write_in, Isolation, Lang, Worker, WorkerLimits};
use std::{fs, path::PathBuf};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Event {
  fields { id: Id; member: Member; checked: Bool }
  rows read when member = actor
  transition mark { allow member = actor; from checked = false; to checked = true }
  expose apply mark { target id; bulk maxRows 2 }
  extension write run {
    input { id: Event.Id }
    output { done: Bool }
    access Event.mark
    effect db
    deadline 2s
    implementation "write.run"
  }
}
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn write_effects_follow_caller_commit_and_rollback() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-write-owned-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(
        owned.0.join("write.mjs"),
        "export async function run(input, ctx) { await ctx.data.apply('Event', 'mark', ['11']); if (input.id !== '11') await ctx.data.apply('Event', 'mark', [input.id]); return {done:true}; }\n",
    )
    .unwrap();
    fs::write(
        owned.0.join("write.py"),
        "async def run(input, ctx):\n    await ctx.data.apply('Event', 'mark', ['11'])\n    if input['id'] != '11':\n        await ctx.data.apply('Event', 'mark', [input['id']])\n    return {'done':True}\n",
    )
    .unwrap();
    let schema = format!("aip_write_owned_{unique}");
    sqlgen::set_schema(&schema);
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL");
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures = vec![];
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) { db.batch_execute(statement).await?; }
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES (1),(2); INSERT INTO {schema}.event(id,member_id,checked) VALUES (11,1,false),(99,2,false); CREATE TABLE {schema}.caller_result (id integer PRIMARY KEY)")).await?;
        let caller = Caller { actor_id: Some(1), now: "2026-10-05T00:00:00Z".into() };
        for lang in [Lang::Node, Lang::Python] {
            let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
            let mut worker = Worker::try_start_with(lang, owned.0.to_str().unwrap(), isolation, WorkerLimits::default()).await.expect("official worker");
            for commit in [false,true] {
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.caller_result")).await?;
                let tx = db.transaction().await?;
                let prepared = prepare_write_in(&tx, &mut worker, &facts, "Event.run", &json!({"id":"11"}), &caller).await;
                match prepared {
                    Ok(value) => {
                        if value.output != json!({"done":true}) || value.expires <= std::time::Instant::now() { failures.push(format!("{lang:?} invalid prepared result")); }
                        let changed: bool = tx.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=11"), &[]).await?.get(0);
                        if !changed { failures.push(format!("{lang:?} effect absent inside caller transaction")); }
                        tx.execute(&format!("INSERT INTO {schema}.caller_result(id) VALUES(1)"), &[]).await?;
                        if commit { tx.commit().await?; } else { tx.rollback().await?; }
                    }
                    Err(error) => { failures.push(format!("{lang:?} prepare: {}",error.code)); tx.rollback().await?; }
                }
                let row = db.query_one(&format!("SELECT (SELECT checked FROM {schema}.event WHERE id=11), (SELECT count(*) FROM {schema}.caller_result)"), &[]).await?;
                if row.get::<_,bool>(0) != commit || row.get::<_,i64>(1) != i64::from(commit) { failures.push(format!("{lang:?} caller commit={commit}: effect/result atomicity")); }
            }
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await?;
            let tx = db.transaction().await?;
            let denied = prepare_write_in(&tx, &mut worker, &facts, "Event.run", &json!({"id":"99"}), &caller).await;
            if !denied.is_err_and(|error| error.code=="EXTENSION_ERROR") { failures.push(format!("{lang:?} foreign actor target not rejected")); }
            let partial: bool = tx.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=11"), &[]).await?.get(0);
            if !partial { failures.push(format!("{lang:?} partial-write failure fixture did not execute first ctx write")); }
            tx.rollback().await?;
            worker.stop().await;
            let own: bool = db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=11"), &[]).await?.get(0);
            if own { failures.push(format!("{lang:?} caller rollback did not undo first ctx write")); }
            let foreign: bool = db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=99"), &[]).await?.get(0);
            if foreign { failures.push(format!("{lang:?} foreign row changed")); }
        }
        Ok(())
    }.await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "owned schema cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
