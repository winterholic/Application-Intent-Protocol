#![cfg(target_os = "macos")]
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{listen_with_prototype_options, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{sleep, timeout},
};

fn alive(pid: i32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}
async fn pids(path: &Path, count: usize) -> Vec<i32> {
    timeout(Duration::from_secs(2), async {
        loop {
            let rows: Vec<i32> = fs::read_to_string(path).unwrap_or_default().lines().map(|line| line.parse().unwrap()).collect();
            if rows.len() >= count {
                return rows;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned workers report PIDs")
}
async fn dead(pids: &[i32]) -> bool {
    timeout(Duration::from_secs(2), async {
        while pids.iter().any(|pid| alive(*pid)) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}
async fn post(address: std::net::SocketAddr, token: &str, fp: &str, id: i64, key: &str) -> Value {
    let body = json!({"key":key,"request":{"extension":"Event.run","input":{"id":id}}}).to_string();
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(format!("POST /apply HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    let mut bytes = vec![];
    timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes)).await.unwrap().unwrap();
    serde_json::from_str(String::from_utf8(bytes).unwrap().split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn write_slots_deadlines_and_shutdown_release_children_and_uncommitted_effects() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let root = std::env::temp_dir().join(format!("aip-write-lifetime-{unique}"));
    fs::create_dir(&root).expect("fresh worker directory");
    let schema = format!("aip_write_lifetime_{unique}");
    sqlgen::set_schema(&schema);
    let facts = load_str(include_str!("fixtures/write-extension.aip"), Form::A).unwrap().execution;
    let db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures = vec![];
    let run: Result<(),tokio_postgres::Error>=async {
        for statement in ddl.iter().skip(1) { db.batch_execute(statement).await?; }
        db.batch_execute(&idempotency_ddl()).await?;
        db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked) VALUES(11,1,false),(12,1,false),(13,1,false),(14,1,false),(15,1,false)")).await?;
        for lang in [WorkerLang::Node,WorkerLang::Python] {
            let log=root.join("pids");let finish=root.join("finish");
            fs::write(&log,"").unwrap();
            let quoted=serde_json::to_string(root.to_str().unwrap()).unwrap();
            fs::write(root.join("write.mjs"),format!("import fs from 'node:fs';\nconst root={quoted};\nexport async function run(input,ctx){{const r=await ctx.data.apply('Event','mark',[input.id]);fs.appendFileSync(root+'/pids',process.pid+'\\n');if(!fs.existsSync(root+'/finish')){{while(true){{}}}}return {{id:r.changed[0]??r.unchanged[0],count:1}};}}\n")).unwrap();
            fs::write(root.join("write.py"),format!("import os\nfrom pathlib import Path\nroot=Path({quoted})\nasync def run(input,ctx):\n    r=await ctx.data.apply('Event','mark',[input['id']])\n    with (root/'pids').open('a') as f: f.write(str(os.getpid())+'\\n')\n    if not (root/'finish').exists():\n        while True: pass\n    return {{'id':(r['changed']+r['unchanged'])[0],'count':1}}\n")).unwrap();
            let server=listen_with_prototype_options(facts.clone(),IdWire::SafeNumber,"127.0.0.1:0".parse().unwrap(),"host=localhost dbname=postgres".into(),Some(ReadExtensions{lang,dir:root.clone()}),ServerOptions::default(),true).await.unwrap();
            let fp=contract_fingerprint_with_all_extensions(&facts,IdWire::SafeNumber);
            let token=server.keys.issue(1,60);
            let mut calls=vec![];
            for id in 11..15 {
                let (address,token,fp)=(server.address,token.clone(),fp.clone());
                calls.push(tokio::spawn(async move{post(address,&token,&fp,id,&format!("loop-{id}")).await}));
            }
            let owned=pids(&log,4).await;
            let busy=post(server.address,&token,&fp,15,"overflow").await;
            if busy["code"]!="WORKER_BUSY" {failures.push(format!("{lang:?} overflow: {busy}"));}
            if fs::read_to_string(&log).unwrap().lines().count()!=4 {failures.push(format!("{lang:?} overflow started another worker"));}
            for call in calls {
                let result=call.await.unwrap();
                if result["code"]!="DEADLINE_EXCEEDED" {failures.push(format!("{lang:?} timeout: {result}"));}
            }
            if !dead(&owned).await {failures.push(format!("{lang:?} timeout children survived"));}
            let row=db.query_one(&format!("SELECT (SELECT count(*) FROM {schema}.event WHERE checked),(SELECT count(*) FROM {schema}.aip_idem)"),&[]).await?;
            if row.get::<_,i64>(0)!=0 || row.get::<_,i64>(1)!=0 {failures.push(format!("{lang:?} timed-out effects/results survived"));}
            fs::write(&finish,"").unwrap();
            let resumed=post(server.address,&token,&fp,11,"after-timeout").await;
            if resumed["ok"]!=true {failures.push(format!("{lang:?} slot not returned: {resumed}"));}
            fs::remove_file(&finish).unwrap();
            let (address,t,fp2)=(server.address,token.clone(),fp.clone());
            let pending=tokio::spawn(async move{post(address,&t,&fp2,15,"shutdown").await});
            let owned=pids(&log,6).await;
            server.shutdown().await.unwrap();
            pending.abort();let _=pending.await;
            if !dead(&owned).await {failures.push(format!("{lang:?} shutdown child survived"));}
            let rolled_back=timeout(Duration::from_secs(2),async {
                loop {
                    // MVCC can show false while an uncommitted writer still holds the row lock.
                    db.batch_execute("SET lock_timeout='500ms'").await?;
                    let row=db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=15 FOR UPDATE"),&[]).await?;
                    let records=db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"),&[]).await?;
                    if !row.get::<_,bool>(0) && records.get::<_,i64>(0)==1 {return Ok::<_,tokio_postgres::Error>(());}
                    sleep(Duration::from_millis(10)).await;
                }
            }).await;
            if !matches!(rolled_back,Ok(Ok(()))) {failures.push(format!("{lang:?} shutdown transaction not released: {rolled_back:?}"));}
            for pid in owned {if alive(pid){let _=Command::new("kill").args(["-TERM",&pid.to_string()]).status();}}
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false;TRUNCATE {schema}.aip_idem")).await?;
        }
        Ok(())
    }.await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    fs::remove_dir_all(&root).unwrap();
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
