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
use std::{fs, path::PathBuf};

const SPEC: &str = include_str!("fixtures/write-extension.aip");

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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
    let schema = format!("aip_write_atomic_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL");
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
                let first = write_extension::apply(&mut db, &facts, &body, &caller, wire, &config).await;
                if first["ok"] != true || first["output"] != json!({"id":id,"count":1}) || first["tags"] != json!(["Event"]) {
                    failures.push(format!("{lang:?} {wire:?} first: {first}"));
                }
                let suffix = if lang == WorkerLang::Node { "mjs" } else { "py" };
                let module = owned.0.join(format!("write.{suffix}"));
                let hidden = owned.0.join(format!("write.{suffix}.hidden"));
                fs::rename(&module, &hidden).unwrap();
                let replay = write_extension::apply(&mut db, &facts, &body, &caller, wire, &config).await;
                fs::rename(&hidden, &module).unwrap();
                if replay["ok"] != true || replay["replayed"] != true || replay["output"] != first["output"] {
                    failures.push(format!("{lang:?} {wire:?} replay without module: {replay}"));
                }
                let count = fs::read_to_string(owned.0.join("counter")).unwrap().lines().count();
                if count != 1 {
                    failures.push(format!("{lang:?} {wire:?} worker invoked {count} times"));
                }
                let different = json!({"key":key,"request":{"extension":"Event.run","input":{"id":emit_id(99,wire).unwrap()}}});
                let mismatch = write_extension::apply(&mut db, &facts, &different, &caller, wire, &config).await;
                if mismatch["code"] != "IDEMPOTENCY_MISMATCH" {
                    failures.push(format!("{lang:?} {wire:?} key mismatch: {mismatch}"));
                }
                let row = db
                    .query_one(&format!("SELECT (SELECT checked FROM {schema}.event WHERE id=11),(SELECT count(*) FROM {schema}.aip_idem)"), &[])
                    .await?;
                if !row.get::<_, bool>(0) || row.get::<_, i64>(1) != 1 {
                    failures.push(format!("{lang:?} {wire:?} committed effect/result absent"));
                }
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;
                fs::write(owned.0.join("counter"), "").unwrap();
                let mut parallel = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL");
                let concurrent_body = json!({"key":"concurrent","request":request});
                let (left,right) = tokio::join!(
                    write_extension::apply(&mut db,&facts,&concurrent_body,&caller,wire,&config),
                    write_extension::apply(&mut parallel,&facts,&concurrent_body,&caller,wire,&config)
                );
                if left["ok"]!=true || right["ok"]!=true || (left["replayed"]!=true && right["replayed"]!=true) {
                    failures.push(format!("{lang:?} {wire:?} concurrent replay: {left} / {right}"));
                }
                if fs::read_to_string(owned.0.join("counter")).unwrap().lines().count()!=1 { failures.push(format!("{lang:?} {wire:?} concurrent duplicate execution")); }
                let other_actor = Caller { actor_id:Some(2),now:caller.now.clone() };
                let other_body = json!({"key":"concurrent","request":{"extension":"Event.run","input":{"id":emit_id(99,wire).unwrap()}}});
                let other_result = write_extension::apply(&mut db,&facts,&other_body,&other_actor,wire,&config).await;
                if other_result["ok"]!=true || other_result["replayed"]==true { failures.push(format!("{lang:?} {wire:?} principal scope: {other_result}")); }
                let other_wire = if wire==IdWire::SafeNumber { IdWire::DecimalString } else { IdWire::SafeNumber };
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false WHERE id=11")).await?;
                let wire_body = json!({"key":"concurrent","request":{"extension":"Event.run","input":{"id":emit_id(11,other_wire).unwrap()}}});
                let other_result = write_extension::apply(&mut db,&facts,&wire_body,&caller,other_wire,&config).await;
                if other_result["ok"]!=true || other_result["replayed"]==true { failures.push(format!("{lang:?} {wire:?} wire scope: {other_result}")); }
                let scopes:i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"),&[]).await?.get(0);
                if scopes!=3 { failures.push(format!("{lang:?} {wire:?} scope records: {scopes}")); }

                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem; ALTER TABLE {schema}.aip_idem ADD CONSTRAINT reject_result CHECK(key<>'insert-fail')")).await?;
                let insert_body = json!({"key":"insert-fail","request":request});
                let failed_insert = write_extension::apply(&mut db,&facts,&insert_body,&caller,wire,&config).await;
                if failed_insert["code"]!="INTERNAL" { failures.push(format!("{lang:?} {wire:?} insert failure: {failed_insert}")); }
                let row = db.query_one(&format!("SELECT (SELECT count(*) FROM {schema}.event WHERE checked),(SELECT count(*) FROM {schema}.aip_idem)"),&[]).await?;
                if row.get::<_,i64>(0)!=0 || row.get::<_,i64>(1)!=0 { failures.push(format!("{lang:?} {wire:?} failed idem insert leaked effect")); }
                db.batch_execute(&format!("ALTER TABLE {schema}.aip_idem DROP CONSTRAINT reject_result")).await?;

                db.batch_execute(&format!("CREATE FUNCTION {schema}.slow_idem() RETURNS trigger AS $$ BEGIN PERFORM pg_sleep(1.5); RETURN NULL; END $$ LANGUAGE plpgsql; CREATE CONSTRAINT TRIGGER slow_idem AFTER INSERT ON {schema}.aip_idem DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {schema}.slow_idem()")).await?;
                fs::write(owned.0.join("counter"), "").unwrap();
                let mut delayed_facts = facts.clone();delayed_facts["resources"]["Event"]["extensions"]["run"]["implementation"]=json!("write.delayed");
                let delayed_body = json!({"key":"commit-unknown","request":request});
                let unknown = write_extension::apply(&mut db,&delayed_facts,&delayed_body,&caller,wire,&config).await;
                if unknown["code"]!="COMMIT_UNKNOWN" { failures.push(format!("{lang:?} {wire:?} slow commit: {unknown}")); }
                let recovered = write_extension::apply(&mut db,&delayed_facts,&delayed_body,&caller,wire,&config).await;
                if recovered["ok"]!=true || recovered["replayed"]!=true { failures.push(format!("{lang:?} {wire:?} unknown commit replay: {recovered}")); }
                if fs::read_to_string(owned.0.join("counter")).unwrap().lines().count()!=1 { failures.push(format!("{lang:?} {wire:?} unknown commit reran worker")); }
                db.batch_execute(&format!("DROP TRIGGER slow_idem ON {schema}.aip_idem; DROP FUNCTION {schema}.slow_idem()")).await?;

                for (implementation, code) in [("write.unsafe", "OUTPUT_INVALID"), ("write.denied", "EXTENSION_ERROR")] {
                    db.batch_execute(&format!("UPDATE {schema}.event SET checked=false; TRUNCATE {schema}.aip_idem")).await?;
                    let mut rejected_facts = facts.clone();
                    rejected_facts["resources"]["Event"]["extensions"]["run"]["implementation"] = json!(implementation);
                    let rejected = write_extension::apply(&mut db, &rejected_facts, &body, &caller, wire, &config).await;
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
