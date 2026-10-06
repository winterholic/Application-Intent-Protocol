use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_module_with_apply;
use std::{fs, process::Command};

const SOURCE: &str = include_str!("../../../prototype/example/app.aip");

fn tsc(dir: &str, file: &str) -> std::process::Output {
    Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-0-ts/node_modules/.bin/tsc"))
        .current_dir(dir)
        .args([
            "--noEmit",
            "--strict",
            "--target",
            "esnext",
            "--module",
            "nodenext",
            "--moduleResolution",
            "nodenext",
            "--allowImportingTsExtensions",
            "--skipLibCheck",
            file,
        ])
        .output()
        .unwrap()
}

// V14's actual public projection, kept as a protocol compatibility control.
fn previous_module(facts: &Value, wire: IdWire) -> String {
    let old_ts = spike_v5_sdk::contract_ts_with_apply(facts, wire);
    let (read, apply) = old_ts.split_once("export interface ApplyContract").unwrap();
    let apply = apply
        .lines()
        .map(|line| {
            if ["link:", "phase:", "date:"].iter().any(|field| line.trim_start().starts_with(field)) {
                format!("      {} never;", line.trim().split_once(':').unwrap().0.to_string() + ":")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let types = format!("{read}export interface ApplyContract{apply}\n");
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(types.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    format!("{types}\nimport type {{ApplyBinding}} from '../../spike-v5-sdk/sdk/generic.ts';\nexport const contract: ApplyBinding<Contract, ApplyContract> = {{fingerprint:'{hash}',idWire:'{}'}};",wire.label())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_filter_values_cross_generated_sdk_http_and_actual_commit() {
    sqlgen::set_schema("aip_v15");
    let schema = sqlgen::schema();
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let db = connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member (id) VALUES (1),(2);
      INSERT INTO {schema}.event (id,member_id,checked,title,link,phase,date) VALUES
      (11,1,false,'url','https://example.test/a?x='';--','READY','2020-01-01Z'),
      (12,1,false,'enum','enum','DONE','2020-01-01Z'),
      (13,1,false,'js','js','READY','2026-10-04T00:00:00.123Z'),
      (14,1,false,'python','py','READY','2026-10-04T00:00:00.123456Z'),
      (15,1,false,'nano','nano','READY','2026-10-04T00:00:00.123456789Z'),
      (16,1,false,'unchanged','none','READY','2020-01-01Z'),
      (99,2,false,'other','https://example.test/a?x='';--','READY','2026-10-04T00:00:00.123Z');"
    ))
    .await
    .unwrap();
    for (row, title) in
        [(13_i64, "100%_\\' 서울"), (16, "100%_\\' 서울 두번째"), (11, "100xwrong"), (14, "서울 모집"), (15, "서울 공고"), (99, "서울 숨김")]
    {
        db.execute(&format!("UPDATE {schema}.event SET title=$1 WHERE id=$2"), &[&title, &row]).await.unwrap();
    }
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let mut failures = vec![];
    for (name, wire) in [("safe", IdWire::SafeNumber), ("string", IdWire::DecimalString)] {
        fs::write(format!("{dir}/generated-v15-{name}.ts"), contract_module_with_apply(&facts, "../../spike-v5-sdk/sdk/generic.ts", wire)).unwrap();
        fs::write(format!("{dir}/generated-v15-{name}-stale.ts"), previous_module(&facts, wire)).unwrap();
    }
    let types = tsc(dir, "v15-type-tests.ts");
    if !types.status.success() {
        failures.push(format!("positive types: {}", String::from_utf8_lossy(&types.stdout)));
    }
    let source = fs::read_to_string(format!("{dir}/v15-type-tests.ts")).unwrap();
    assert_eq!(source.matches("@ts-expect-error").count(), 8);
    fs::write(format!("{dir}/.v15-negative-control.ts"), source.replace("@ts-expect-error", "")).unwrap();
    let negative = tsc(dir, ".v15-negative-control.ts");
    if negative.status.success() || !String::from_utf8_lossy(&negative.stdout).contains(".v15-negative-control.ts") {
        failures.push("type checker missed negative control".into());
    }
    fs::remove_file(format!("{dir}/.v15-negative-control.ts")).unwrap();
    let python=Command::new("python3").args(["-c","from datetime import datetime,timezone,timedelta; print(datetime(2026,10,4,9,0,0,123456,tzinfo=timezone(timedelta(hours=9))).isoformat())"]).output().unwrap();
    assert!(python.status.success());
    let python = String::from_utf8(python.stdout).unwrap().trim().to_string();
    for (name, wire) in [("safe", IdWire::SafeNumber), ("string", IdWire::DecimalString)] {
        let (port, keys) = spike_v6_transport::start_with_apply_contract(facts.clone(), wire).await;
        let token = keys.issue(1, 600);
        let other = keys.issue(2, 600);
        let python = python.clone();
        let output = tokio::task::spawn_blocking(move || {
            Command::new("node")
                .current_dir(dir)
                .env("AIP_V15_URL", format!("http://127.0.0.1:{port}"))
                .env("AIP_V15_TOKEN", token)
                .env("AIP_V15_OTHER", other)
                .env("AIP_V15_WIRE", name)
                .env("AIP_V15_PYTHON_TIME", python)
                .args(["--test", "v15-http.test.ts"])
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        let logs = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        eprintln!("V15 {name}: {}", logs.lines().filter(|line| line.starts_with("ℹ ")).collect::<Vec<_>>().join("; "));
        if !output.status.success() {
            failures.push(format!("HTTP {name}: {logs}"));
        }
        let changed: Vec<i64> =
            db.query(&format!("SELECT id FROM {schema}.event WHERE checked ORDER BY id"), &[]).await.unwrap().iter().map(|row| row.get(0)).collect();
        if changed != vec![11, 12, 13, 14, 15] {
            failures.push(format!("{name} actual DB changes {changed:?}"));
        }
        db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await.unwrap();
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
