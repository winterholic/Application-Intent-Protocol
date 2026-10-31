use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url,
    id_wire::{emit_id, IdWire},
    plan::Caller,
    sqlgen,
};
use spike_v6_transport::{
    idempotency_ddl,
    server::{ReadExtensions, WorkerLang},
    write_extension,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const SPEC: &str = include_str!("fixtures/write-extension.aip");

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn apply_observed_fixture(
    db: &mut tokio_postgres::Client,
    facts: &Value,
    body: &Value,
    caller: &Caller,
    wire: IdWire,
    config: &ReadExtensions,
) -> Value {
    // 호출 횟수 관찰 파일을 쓰는 신뢰된 fixture에 한해 sandbox를 끈다.
    let launch = spike_v4_worker::WorkerLaunchConfig { isolation: spike_v4_worker::Isolation::None, ..Default::default() };
    write_extension::apply_with_launch(db, facts, body, caller, wire, config, &launch).await
}

async fn wait_for_worker_count(counter: &Path) -> bool {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if fs::read_to_string(counter).is_ok_and(|contents| contents.lines().count() == 1) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok()
}

async fn wait_for_claim_lock(observer: &tokio_postgres::Client, schema: &str) -> Result<bool, tokio_postgres::Error> {
    let pattern = format!("%{schema}.aip_idem%");
    match tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let waiting: i64 = observer
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE pid<>pg_backend_pid() AND state='active' AND wait_event_type='Lock' AND query LIKE 'INSERT%' AND query LIKE $1",
                    &[&pattern],
                )
                .await?
                .get(0);
            if waiting > 0 {
                return Ok::<_, tokio_postgres::Error>(true);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Ok(false),
    }
}

async fn settle_worker(handle: &mut tokio::task::JoinHandle<Value>) -> Result<Value, String> {
    match tokio::time::timeout(Duration::from_secs(5), &mut *handle).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(format!("request task failed: {error}")),
        Err(_) => {
            handle.abort();
            let _ = handle.await;
            Err("request task exceeded the bounded cleanup deadline".into())
        }
    }
}

/// Holds the target row so the first worker keeps its claim open. The second request must be
/// observed waiting on the actual aip_idem INSERT before the row lock is released.
async fn assert_same_key_contention(
    db: &mut tokio_postgres::Client,
    schema: &str,
    counter: &Path,
    facts: &Value,
    caller: &Caller,
    wire: IdWire,
    config: &ReadExtensions,
    key: &str,
) -> Result<(), String> {
    db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem"))
        .await
        .map_err(|error| format!("reset contention fixture: {error}"))?;
    fs::write(counter, "").map_err(|error| format!("reset worker counter: {error}"))?;

    let mut blocker_client =
        connect_with_url(aip_test_support::test_database_url!()).await.map_err(|error| format!("connect row-lock barrier: {error}"))?;
    let blocker = blocker_client.transaction().await.map_err(|error| format!("begin row-lock barrier: {error}"))?;
    blocker
        .query_one(&format!("SELECT id FROM {schema}.event WHERE id=11 FOR UPDATE"), &[])
        .await
        .map_err(|error| format!("hold target row: {error}"))?;

    let observer = connect_with_url(aip_test_support::test_database_url!()).await.map_err(|error| format!("connect lock observer: {error}"))?;
    let mut first_db = connect_with_url(aip_test_support::test_database_url!()).await.map_err(|error| format!("connect first request: {error}"))?;
    let mut second_db = connect_with_url(aip_test_support::test_database_url!()).await.map_err(|error| format!("connect second request: {error}"))?;
    let id = emit_id(11, wire).map_err(|error| format!("encode contention target: {error}"))?;
    let body = json!({"key":key,"request":{"extension":"Event.run","input":{"id":id}}});
    let first_caller = Caller { actor_id: caller.actor_id, now: caller.now.clone() };
    let (first_facts, first_body, first_config) = (facts.clone(), body.clone(), config.clone());
    let first =
        tokio::spawn(async move { apply_observed_fixture(&mut first_db, &first_facts, &first_body, &first_caller, wire, &first_config).await });
    if !wait_for_worker_count(counter).await {
        let _ = blocker.commit().await;
        let mut first = first;
        let _ = settle_worker(&mut first).await;
        return Err("first worker did not reach the held row within 1s; blocker released and request joined/aborted within 5s".into());
    }

    let second_caller = Caller { actor_id: caller.actor_id, now: caller.now.clone() };
    let (second_facts, second_body, second_config) = (facts.clone(), body, config.clone());
    let second =
        tokio::spawn(async move { apply_observed_fixture(&mut second_db, &second_facts, &second_body, &second_caller, wire, &second_config).await });
    let observed_wait = wait_for_claim_lock(&observer, schema).await;
    let release_result = blocker.commit().await;
    let (mut first, mut second) = (first, second);
    let first_result = settle_worker(&mut first).await;
    let second_result = settle_worker(&mut second).await;
    let mut errors = Vec::new();
    match observed_wait {
        Ok(true) => {}
        Ok(false) => errors.push("second request was not observed waiting on the aip_idem claim lock within 1s".into()),
        Err(error) => errors.push(format!("observe aip_idem claim wait: {error}")),
    }
    if let Err(error) = release_result {
        errors.push(format!("release target row lock: {error}"));
    }
    match first_result {
        Ok(result) if result["ok"] == true && result.get("replayed").is_none() => {}
        Ok(result) => errors.push(format!("first contended request did not commit once: {result}")),
        Err(error) => errors.push(format!("first contended request: {error}")),
    }
    match second_result {
        Ok(result) if result["ok"] == true && result["replayed"] == true => {}
        Ok(result) => errors.push(format!("second contended request did not replay: {result}")),
        Err(error) => errors.push(format!("second contended request: {error}")),
    }
    let invocations = fs::read_to_string(counter).map(|contents| contents.lines().count()).unwrap_or_default();
    if invocations != 1 {
        errors.push(format!("contended worker invocation count was {invocations}, expected 1"));
    }
    let row = db
        .query_one(&format!("SELECT (SELECT checked FROM {schema}.event WHERE id=11),(SELECT count(*) FROM {schema}.aip_idem)"), &[])
        .await
        .map_err(|error| format!("verify contention commit: {error}"))?;
    if !row.get::<_, bool>(0) || row.get::<_, i64>(1) != 1 {
        errors.push("contention did not leave exactly one committed effect and idem result".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[tokio::test]
async fn write_effects_and_idempotent_result_commit_together_without_worker_replay() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-write-atomic-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(owned.0.join("write.mjs"), "import fs from 'node:fs';\nconst countFile=new URL('./counter',import.meta.url);\nfunction count(){fs.appendFileSync(countFile,'1\\n');}\nexport async function run(input,ctx){count();const result=await ctx.data.apply('Event','mark',[input.id]);return {id:result.changed[0]??result.unchanged[0],count:1};}\nexport async function delayed(input,ctx){const out=await run(input,ctx);await new Promise(r=>setTimeout(r,800));return out;}\nexport async function unsafe(input,ctx){count();await ctx.data.apply('Event','mark',[input.id]);return {id:input.id,count:9007199254740992};}\nexport async function denied(input,ctx){count();await ctx.data.apply('Event','mark',['11']);await ctx.data.apply('Event','mark',['99']);return {id:input.id,count:1};}\n").unwrap();
    fs::write(owned.0.join("write.py"), "from pathlib import Path\nimport asyncio\ndef count():\n    with (Path(__file__).parent/'counter').open('a') as f: f.write('1\\n')\nasync def run(input,ctx):\n    count()\n    r=await ctx.data.apply('Event','mark',[input['id']])\n    return {'id':(r['changed']+r['unchanged'])[0],'count':1}\nasync def delayed(input,ctx):\n    out=await run(input,ctx)\n    await asyncio.sleep(0.8)\n    return out\nasync def unsafe(input,ctx):\n    count()\n    await ctx.data.apply('Event','mark',[input['id']])\n    return {'id':input['id'],'count':9007199254740992}\nasync def denied(input,ctx):\n    count()\n    await ctx.data.apply('Event','mark',['11'])\n    await ctx.data.apply('Event','mark',['99'])\n    return {'id':input['id'],'count':1}\n").unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let serializable_facts: Value =
        load_str(include_str!("fixtures/write-extension-serializable.aip"), Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(facts["resources"]["Event"]["transitions"]["mark"]["isolation"], Value::Null);
    assert_eq!(serializable_facts["resources"]["Event"]["transitions"]["mark"]["isolation"], "serializable");
    let schema = format!("aip_write_atomic_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(aip_test_support::test_database_url!()).await.expect("local PostgreSQL");
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let caller = Caller { actor_id: Some(1), now: "2026-10-05T00:00:00Z".into() };
    let mut failures = vec![];
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(99,2,false)"
        ))
        .await?;
        for lang in [WorkerLang::Node, WorkerLang::Python] {
            for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
                let config = ReadExtensions { lang, dir: owned.0.clone() };
                let id = emit_id(11, wire).unwrap();
                let key = format!("{lang:?}-{wire:?}");
                let request = json!({"extension":"Event.run","input":{"id":id}});
                let body = json!({"key":key,"request":request});
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;
                fs::write(owned.0.join("counter"), "").unwrap();
                let first = apply_observed_fixture(&mut db, &facts, &body, &caller, wire, &config).await;
                if first["ok"] != true || first["output"] != json!({"id":id,"count":1}) || first["tags"] != json!(["Event"]) {
                    failures.push(format!("{lang:?} {wire:?} first: {first}"));
                }
                let suffix = if lang == WorkerLang::Node { "mjs" } else { "py" };
                let module = owned.0.join(format!("write.{suffix}"));
                let hidden = owned.0.join(format!("write.{suffix}.hidden"));
                fs::rename(&module, &hidden).unwrap();
                let replay = apply_observed_fixture(&mut db, &facts, &body, &caller, wire, &config).await;
                fs::rename(&hidden, &module).unwrap();
                if replay["ok"] != true || replay["replayed"] != true || replay["output"] != first["output"] {
                    failures.push(format!("{lang:?} {wire:?} replay without module: {replay}"));
                }
                let count = fs::read_to_string(owned.0.join("counter")).unwrap().lines().count();
                if count != 1 {
                    failures.push(format!("{lang:?} {wire:?} worker invoked {count} times"));
                }
                let different = json!({"key":key,"request":{"extension":"Event.run","input":{"id":emit_id(99,wire).unwrap()}}});
                let mismatch = apply_observed_fixture(&mut db, &facts, &different, &caller, wire, &config).await;
                if mismatch["code"] != "WIRE.IDEMPOTENCY_MISMATCH" {
                    failures.push(format!("{lang:?} {wire:?} key mismatch: {mismatch}"));
                }
                let row = db
                    .query_one(&format!("SELECT (SELECT checked FROM {schema}.event WHERE id=11),(SELECT count(*) FROM {schema}.aip_idem)"), &[])
                    .await?;
                if !row.get::<_, bool>(0) || row.get::<_, i64>(1) != 1 {
                    failures.push(format!("{lang:?} {wire:?} committed effect/result absent"));
                }
                for (mode, contention_facts) in [("read-committed", &facts), ("serializable", &serializable_facts)] {
                    if let Err(error) = assert_same_key_contention(
                        &mut db,
                        &schema,
                        &owned.0.join("counter"),
                        contention_facts,
                        &caller,
                        wire,
                        &config,
                        "concurrent",
                    )
                    .await
                    {
                        failures.push(format!("{lang:?} {wire:?} {mode} deterministic contention: {error}"));
                    }
                }
                let other_actor = Caller { actor_id:Some(2),now:caller.now.clone() };
                let other_body = json!({"key":"concurrent","request":{"extension":"Event.run","input":{"id":emit_id(99,wire).unwrap()}}});
                let other_result = apply_observed_fixture(&mut db,&facts,&other_body,&other_actor,wire,&config).await;
                if other_result["ok"]!=true || other_result["replayed"]==true { failures.push(format!("{lang:?} {wire:?} principal scope: {other_result}")); }
                let other_wire = if wire==IdWire::SafeNumber { IdWire::DecimalString } else { IdWire::SafeNumber };
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false WHERE id=11")).await?;
                let wire_body = json!({"key":"concurrent","request":{"extension":"Event.run","input":{"id":emit_id(11,other_wire).unwrap()}}});
                let other_result = apply_observed_fixture(&mut db,&facts,&wire_body,&caller,other_wire,&config).await;
                if other_result["ok"]!=true || other_result["replayed"]==true { failures.push(format!("{lang:?} {wire:?} wire scope: {other_result}")); }
                let scopes:i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"),&[]).await?.get(0);
                if scopes!=3 { failures.push(format!("{lang:?} {wire:?} scope records: {scopes}")); }

                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem; ALTER TABLE {schema}.aip_idem ADD CONSTRAINT reject_claim CHECK(key<>'insert-fail')")).await?;
                fs::write(owned.0.join("counter"), "").unwrap();
                let insert_body = json!({"key":"insert-fail","request":request});
                let failed_insert = apply_observed_fixture(&mut db,&facts,&insert_body,&caller,wire,&config).await;
                if failed_insert["code"]!="WIRE.INTERNAL" { failures.push(format!("{lang:?} {wire:?} claim insert failure: {failed_insert}")); }
                if fs::read_to_string(owned.0.join("counter")).unwrap().lines().count()!=0 { failures.push(format!("{lang:?} {wire:?} worker ran before rejected claim")); }
                let row = db.query_one(&format!("SELECT (SELECT count(*) FROM {schema}.event WHERE checked),(SELECT count(*) FROM {schema}.aip_idem)"),&[]).await?;
                if row.get::<_,i64>(0)!=0 || row.get::<_,i64>(1)!=0 { failures.push(format!("{lang:?} {wire:?} rejected claim leaked effect")); }
                db.batch_execute(&format!("ALTER TABLE {schema}.aip_idem DROP CONSTRAINT reject_claim")).await?;

                db.batch_execute(&format!("CREATE FUNCTION {schema}.slow_idem() RETURNS trigger AS $$ BEGIN PERFORM pg_sleep(1.5); RETURN NULL; END $$ LANGUAGE plpgsql; CREATE CONSTRAINT TRIGGER slow_idem AFTER INSERT ON {schema}.aip_idem DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {schema}.slow_idem()")).await?;
                fs::write(owned.0.join("counter"), "").unwrap();
                let mut delayed_facts = facts.clone();delayed_facts["resources"]["Event"]["extensions"]["run"]["implementation"]=json!("write.delayed");
                let delayed_body = json!({"key":"commit-unknown","request":request});
                let unknown = apply_observed_fixture(&mut db,&delayed_facts,&delayed_body,&caller,wire,&config).await;
                if unknown["code"]!="WIRE.COMMIT_UNKNOWN" { failures.push(format!("{lang:?} {wire:?} slow commit: {unknown}")); }
                let recovered = apply_observed_fixture(&mut db,&delayed_facts,&delayed_body,&caller,wire,&config).await;
                if recovered["ok"]!=true || recovered["replayed"]!=true { failures.push(format!("{lang:?} {wire:?} unknown commit replay: {recovered}")); }
                if fs::read_to_string(owned.0.join("counter")).unwrap().lines().count()!=1 { failures.push(format!("{lang:?} {wire:?} unknown commit reran worker")); }
                db.batch_execute(&format!("DROP TRIGGER slow_idem ON {schema}.aip_idem; DROP FUNCTION {schema}.slow_idem()")).await?;

                for (implementation, code) in [("write.unsafe", "WIRE.OUTPUT_INVALID"), ("write.denied", "WIRE.EXTENSION_ERROR")] {
                    db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;
                    let mut rejected_facts = facts.clone();
                    rejected_facts["resources"]["Event"]["extensions"]["run"]["implementation"] = json!(implementation);
                    let rejected = apply_observed_fixture(&mut db, &rejected_facts, &body, &caller, wire, &config).await;
                    if rejected["code"] != code {
                        failures.push(format!("{lang:?} {wire:?} {implementation}: {rejected}"));
                    }
                    let row = db
                        .query_one(
                            &format!("SELECT (SELECT count(*) FROM {schema}.event WHERE checked),(SELECT count(*) FROM {schema}.aip_idem)"),
                            &[],
                        )
                        .await?;
                    if row.get::<_, i64>(0) != 0 || row.get::<_, i64>(1) != 0 {
                        failures.push(format!("{lang:?} {wire:?} rejected extension effect/result leaked"));
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "owned cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
