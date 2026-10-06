use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, id_wire::IdWire, sqlgen};
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_module_with_apply};
use std::{fs, process::Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SOURCE: &str = include_str!("../fixture/v14.aip");

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

async fn raw_read(port: u16, token: &str) -> Value {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let body = json!({"query":{"read":"Inbox","select":["id"],"filter":[{"field":"title","op":"eq","value":"ids"}]}}).to_string();
    socket
        .write_all(format!("POST /read HTTP/1.1\r\nauthorization: Bearer {token}\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v14_generated_apply_types_and_validated_results_cross_real_http_and_commit_boundary() {
    sqlgen::set_schema("aip_v14");
    let schema = sqlgen::schema();
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let db = connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member (id) VALUES (1),(2);
      INSERT INTO {schema}.inbox (id,member_id,checked,count,title) VALUES
       (42,1,false,17,'ids'),(43,1,false,17,'where'),(44,1,false,17,'lost'),(45,1,false,17,'malformed'),
       (9007199254740993,1,false,17,'large'),(777,2,false,17,'hidden');
      INSERT INTO {schema}.write_only (id,member_id,checked) VALUES (7,1,false),(8,2,false);
      INSERT INTO {schema}.where_only (id,member_id,checked,title) VALUES (9,1,false,'one'),(10,1,false,'two');"
    ))
    .await
    .unwrap();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let mut failures = Vec::new();
    let mut stale = facts.clone();
    stale["resources"]["WriteOnly"]["exposeApply"]["mark"]["bulkMaxRows"] = json!(1);
    for (name, wire) in [("safe", IdWire::SafeNumber), ("string", IdWire::DecimalString)] {
        for (suffix, projection) in [("", &facts), ("-stale", &stale)] {
            fs::write(
                format!("{dir}/generated-v14-{name}{suffix}.ts"),
                contract_module_with_apply(projection, "../../spike-v5-sdk/sdk/generic.ts", wire),
            )
            .unwrap();
        }
    }
    let types = tsc(dir, "v14-type-tests.ts");
    if !types.status.success() {
        failures.push(format!("V14 positive/negative tsc: {}", String::from_utf8_lossy(&types.stdout)));
    }
    let source = fs::read_to_string(format!("{dir}/v14-type-tests.ts")).unwrap();
    assert!(source.contains("@ts-expect-error"));
    let control = format!("{dir}/.v14-negative-control.ts");
    fs::write(&control, source.replace("@ts-expect-error", "")).unwrap();
    let negative = tsc(dir, ".v14-negative-control.ts");
    if negative.status.success() || !String::from_utf8_lossy(&negative.stdout).contains(".v14-negative-control.ts") {
        failures.push("V14 marker removal did not report real errors".into());
    }
    fs::remove_file(control).unwrap();
    for (phase, wire) in [("safe", IdWire::SafeNumber), ("string", IdWire::DecimalString)] {
        let (port, keys) = spike_v6_transport::start_with_apply_contract(facts.clone(), wire).await;
        let token = keys.issue(1, 600);
        let actual = raw_read(port, &token).await;
        if actual["contractFingerprint"] != contract_fingerprint_with_apply(&facts, wire) {
            failures.push(format!("V14 {phase} startup fingerprint differs from generated apply contract"));
        }
        let other = keys.issue(2, 600);
        let node = tokio::task::spawn_blocking(move || {
            Command::new("node")
                .current_dir(dir)
                .env("AIP_V14_URL", format!("http://127.0.0.1:{port}"))
                .env("AIP_V14_TOKEN", token)
                .env("AIP_V14_OTHER", other)
                .env("AIP_V14_WIRE", phase)
                .args(["--test", "v14-http.test.ts", "v14-envelope.test.ts"])
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        let output = format!("{}{}", String::from_utf8_lossy(&node.stdout), String::from_utf8_lossy(&node.stderr));
        eprintln!("V14 phase {phase}: {}", output.lines().filter(|l| l.starts_with("ℹ ")).collect::<Vec<_>>().join("; "));
        if !node.status.success() {
            failures.push(format!("V14 {phase} Node: {output}"));
        }
        for (table, expected) in [
            ("inbox", if wire == IdWire::SafeNumber { vec![42_i64, 43, 44, 45] } else { vec![42_i64, 43, 44, 45, 9_007_199_254_740_993] }),
            ("write_only", vec![7]),
            ("where_only", vec![9]),
        ] {
            let actual: Vec<i64> = db
                .query(&format!("SELECT id FROM {schema}.{table} WHERE checked ORDER BY id"), &[])
                .await
                .unwrap()
                .iter()
                .map(|row| row.get(0))
                .collect();
            if actual != expected {
                failures.push(format!("V14 {phase} DB {table}: {actual:?}, expected {expected:?}"));
            }
            db.batch_execute(&format!("UPDATE {schema}.{table} SET checked=false")).await.unwrap();
        }
    }
    if let Err(e) = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await {
        failures.push(format!("V14 schema cleanup: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
