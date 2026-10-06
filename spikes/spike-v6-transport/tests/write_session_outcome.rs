#![cfg(target_os = "macos")]
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_module_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_options, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{fs, time::Duration};

#[tokio::test]
async fn committed_write_is_reported_even_when_its_admitting_session_expires_during_execution() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let path = std::env::temp_dir().join(format!("aip-write-session-{unique}"));
    fs::create_dir(&path).unwrap();
    fs::write(path.join("write.mjs"),"export async function run(input,ctx){const r=await ctx.data.apply('Event','mark',[input.id]);await new Promise(r=>setTimeout(r,2100));return {id:r.changed[0],count:1};}\n").unwrap();
    fs::write(path.join("write.py"),"import asyncio\nasync def run(input,ctx):\n r=await ctx.data.apply('Event','mark',[input['id']])\n await asyncio.sleep(2.1)\n return {'id':r['changed'][0],'count':1}\n").unwrap();
    fs::write(path.join("package.json"), "{\"type\":\"module\",\"private\":true}").unwrap();
    let mut facts = load_str(include_str!("fixtures/write-extension.aip"), Form::A).unwrap().execution;
    facts["resources"]["Event"]["extensions"]["run"]["deadlineMs"] = json!(4000);
    let schema = format!("aip_write_session_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_owned_with_url("host=localhost dbname=postgres").await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh owned schema");
    let mut failures = vec![];
    let result:Result<(),tokio_postgres::Error>=async {
        for statement in &ddl[1..] {db.batch_execute(statement).await?;}
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1);INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false)")).await?;
        let sdk=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prototype/sdk/index.ts").canonicalize().unwrap();
        for lang in [WorkerLang::Node,WorkerLang::Python] {for wire in [IdWire::SafeNumber,IdWire::DecimalString] {
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false;TRUNCATE {schema}.aip_idem")).await?;
            let server=listen_with_prototype_options(facts.clone(),wire,"127.0.0.1:0".parse().unwrap(),"host=localhost dbname=postgres".into(),Some(ReadExtensions{lang,dir:path.clone()}),ServerOptions::default(),true).await.unwrap();
            fs::write(path.join("contract.ts"),contract_module_with_all_extensions(&facts,sdk.to_str().unwrap(),wire)).unwrap();
            let import=serde_json::to_string(sdk.to_str().unwrap()).unwrap();let id=spike_v2_read::id_wire::emit_id(11,wire).unwrap().to_string();
            fs::write(path.join("call.mts"),format!(r#"import assert from 'node:assert/strict';
import {{connect}} from {import};import {{contract}} from './contract.ts';
const client=connect(process.env.AIP_SESSION_TEST_URL,process.env.AIP_SESSION_TEST_TOKEN,contract);
const result=await client.writeExtension('Event.run',{{id:{id}}},{{key:'late-session',retries:0}});
console.log(JSON.stringify({{result,pending:client.pending()}}));
assert.equal(result.ok,true,'a committed write must report its successful outcome');assert.deepEqual(result.output,{{id:{id},count:1}});assert.deepEqual(client.pending(),[]);
await assert.rejects(client.read({{read:'Event',select:['id']}}),error=>error.code==='TOKEN_EXPIRED');
"#)).unwrap();
            let token=server.keys.issue(1,2);
            let mut node=tokio::process::Command::new("node");node.args(["--experimental-strip-types",path.join("call.mts").to_str().unwrap()]).env("AIP_SESSION_TEST_URL",format!("http://{}",server.address)).env("AIP_SESSION_TEST_TOKEN",&token).kill_on_drop(true);
            match tokio::time::timeout(Duration::from_secs(6),node.output()).await {
                Ok(Ok(output)) if output.status.success()=>{},
                Ok(Ok(output))=>failures.push(format!("{lang:?} {wire:?} committed outcome replaced: stdout={} stderr={}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr))),
                other=>failures.push(format!("{lang:?} {wire:?} SDK process: {other:?}")),
            }
            let row=db.query_one(&format!("SELECT checked,(SELECT count(*) FROM {schema}.aip_idem) FROM {schema}.event WHERE id=11"),&[]).await?;
            if !row.get::<_,bool>(0)||row.get::<_,i64>(1)!=1 {failures.push(format!("{lang:?} {wire:?} expected committed effect/result absent"));}
            if server.keys.verify_session(&token).is_ok() {failures.push(format!("{lang:?} {wire:?} session did not actually expire"));}
            server.shutdown().await.unwrap();
        }}
        Ok(())
    }.await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    fs::remove_dir_all(path).unwrap();
    assert!(result.is_ok(), "setup: {result:?}");
    assert!(cleanup.is_ok(), "cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
