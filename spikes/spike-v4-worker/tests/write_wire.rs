use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url,
    id_wire::{emit_id, IdWire},
    plan::Caller,
    sqlgen,
};
use spike_v4_worker::{write::prepare_write_in_with_wire, Isolation, Lang, Worker, WorkerLimits};
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
    output { id: Event.Id; count: Int }
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
async fn write_wire_preserves_ctx_ids_and_rejects_outputs_before_commit() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-write-wire-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(owned.0.join("write.mjs"), "export async function run(input,ctx) { const r=await ctx.data.apply('Event','mark',[input.id]); return {id:(r.changed[0]??r.unchanged[0]),count:1}; }\nexport async function unsafe(input,ctx) { await ctx.data.apply('Event','mark',[input.id]); return {id:input.id,count:9007199254740992}; }\n").unwrap();
    fs::write(owned.0.join("write.py"), "async def run(input,ctx):\n    r=await ctx.data.apply('Event','mark',[input['id']])\n    return {'id':(r['changed']+r['unchanged'])[0],'count':1}\nasync def unsafe(input,ctx):\n    await ctx.data.apply('Event','mark',[input['id']])\n    return {'id':input['id'],'count':9007199254740992}\n").unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut unsafe_facts = facts.clone();
    unsafe_facts["resources"]["Event"]["extensions"]["run"]["implementation"] = json!("write.unsafe");
    let schema = format!("aip_write_wire_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL");
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures = vec![];
    let run: Result<(),tokio_postgres::Error>=async {
        for statement in ddl.iter().skip(1) { db.batch_execute(statement).await?; }
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(9223372036854775807,1,false)")).await?;
        let caller=Caller{actor_id:Some(1),now:"2026-10-05T00:00:00Z".into()};
        for lang in [Lang::Node,Lang::Python] {
            let isolation=if cfg!(target_os="macos"){Isolation::MacNetDeny}else{Isolation::None};
            let mut worker=Worker::try_start_with(lang,owned.0.to_str().unwrap(),isolation,WorkerLimits::default()).await.expect("official worker");
            for (wire,id) in [(IdWire::SafeNumber,11),(IdWire::DecimalString,11),(IdWire::DecimalString,i64::MAX)] {
                let value=emit_id(id,wire).unwrap();let tx=db.transaction().await?;
                let result=prepare_write_in_with_wire(&tx,&mut worker,&facts,"Event.run",&json!({"id":value}),&caller,wire).await;
                match result {
                    Ok(prepared) if prepared.output==json!({"id":value,"count":1}) => { tx.commit().await?; }
                    Ok(prepared) => { failures.push(format!("{lang:?} {wire:?} {id} wrong ctx Id: {}",prepared.output));tx.rollback().await?; }
                    Err(error) => { failures.push(format!("{lang:?} {wire:?} {id}: {}",error.code));tx.rollback().await?; }
                }
                let changed:bool=db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=$1"),&[&id]).await?.get(0);
                if !changed { failures.push(format!("{lang:?} {wire:?} {id} not committed")); }
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await?;
            }
            for wire in [IdWire::SafeNumber,IdWire::DecimalString] {
                let value=emit_id(11,wire).unwrap();let tx=db.transaction().await?;
                let result=prepare_write_in_with_wire(&tx,&mut worker,&unsafe_facts,"Event.run",&json!({"id":value}),&caller,wire).await;
                if !result.is_err_and(|error|error.code=="OUTPUT_INVALID") { failures.push(format!("{lang:?} {wire:?} unsafe Int output accepted")); }
                tx.rollback().await?;
                let wrong=if wire==IdWire::SafeNumber{json!("11")}else{json!(11)};let tx=db.transaction().await?;
                let result=prepare_write_in_with_wire(&tx,&mut worker,&facts,"Event.run",&json!({"id":wrong}),&caller,wire).await;
                if !result.is_err_and(|error|error.code=="BAD_VALUE") { failures.push(format!("{lang:?} {wire:?} wrong input wire accepted")); }
                tx.rollback().await?;
            }
            worker.stop().await;
            let count:i64=db.query_one(&format!("SELECT count(*) FROM {schema}.event WHERE checked"),&[]).await?.get(0);if count!=0 { failures.push(format!("{lang:?} rejected output effects leaked")); }
        }
        Ok(())
    }.await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "owned cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
