//! V16 (decision CD-6, 2026-10-13): the generated TypeScript client sends atomic bundles and composes over real HTTP.
//! Rust generates the typed modules, runs `tsc` over the positive and negative type cases, starts the server per Id
//! wire, runs the Node client tests and then checks the database for what must and must not have been committed.
use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_module_with_apply};
use std::{fs, process::Command};

const SOURCE: &str = include_str!("../fixture/v16.aip");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v16_generated_bundle_and_compose_types_and_calls_cross_real_http_and_commit_boundary() {
    sqlgen::set_schema("aip_v16");
    let schema = sqlgen::schema();
    let facts: Value = load_str(SOURCE, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let db = spike_v2_read::connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let mut failures = Vec::new();
    // an older contract whose only difference is a compose bound: its fingerprint must no longer match the server
    let mut stale = facts.clone();
    stale["resources"]["Project"]["exposeCompose"]["bulkMaxRows"] = serde_json::json!(1);
    for (name, wire) in [("safe", IdWire::SafeNumber), ("string", IdWire::DecimalString)] {
        assert_ne!(contract_fingerprint_with_apply(&facts, wire), contract_fingerprint_with_apply(&stale, wire));
        for (suffix, projection) in [("", &facts), ("-stale", &stale)] {
            fs::write(
                format!("{dir}/generated-v16-{name}{suffix}.ts"),
                contract_module_with_apply(projection, "../../spike-v5-sdk/sdk/generic.ts", wire),
            )
            .unwrap();
        }
    }
    let types = tsc(dir, "v16-type-tests.ts");
    if !types.status.success() {
        failures.push(format!("V16 positive/negative tsc: {}", String::from_utf8_lossy(&types.stdout)));
    }
    let source = fs::read_to_string(format!("{dir}/v16-type-tests.ts")).unwrap();
    assert!(source.contains("@ts-expect-error"));
    // negative control: without the markers every case must be reported as a real error
    let control = format!("{dir}/.v16-negative-control.ts");
    fs::write(&control, source.replace("@ts-expect-error", "")).unwrap();
    let negative = tsc(dir, ".v16-negative-control.ts");
    let reported = String::from_utf8_lossy(&negative.stdout).matches(".v16-negative-control.ts(").count();
    let markers = source.matches("@ts-expect-error").count();
    if negative.status.success() || reported < markers {
        failures.push(format!("V16 marker removal reported {reported} errors for {markers} markers"));
    }
    fs::remove_file(control).unwrap();

    for (phase, wire) in [("safe", IdWire::SafeNumber), ("string", IdWire::DecimalString)] {
        db.batch_execute(&format!(
            "TRUNCATE {schema}.task, {schema}.log, {schema}.link, {schema}.project, {schema}.member, {schema}.aip_idem RESTART IDENTITY CASCADE; \
             INSERT INTO {schema}.member(id) VALUES (1),(2); \
             INSERT INTO {schema}.project(id,owner_id,name,archived) VALUES (10,1,'p10',false),(11,1,'p11',false),(12,1,'p12',false),(13,1,'p13',false),(14,1,'p14',false),(15,1,'p15',false),(20,2,'p20',false)"
        ))
        .await
        .unwrap();
        let (port, keys) = spike_v6_transport::start_with_apply_contract(facts.clone(), wire).await;
        let token = keys.issue(1, 600);
        let node = tokio::task::spawn_blocking(move || {
            Command::new("node")
                .current_dir(dir)
                .env("AIP_V16_URL", format!("http://127.0.0.1:{port}"))
                .env("AIP_V16_TOKEN", token)
                .env("AIP_V16_WIRE", phase)
                .args(["--test", "v16-http.test.ts"])
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        let output = format!("{}{}", String::from_utf8_lossy(&node.stdout), String::from_utf8_lossy(&node.stderr));
        eprintln!("V16 phase {phase}: {}", output.lines().filter(|l| l.starts_with("ℹ ")).collect::<Vec<_>>().join("; "));
        if !node.status.success() {
            failures.push(format!("V16 {phase} Node: {output}"));
        }
        // the Python caller SDK against the same real server
        let py_token = keys.issue(1, 600);
        let python = tokio::task::spawn_blocking(move || {
            Command::new("python3")
                .current_dir(dir)
                .env("AIP_V16_URL", format!("http://127.0.0.1:{port}"))
                .env("AIP_V16_TOKEN", py_token)
                .env("AIP_V16_WIRE", phase)
                .arg("v16_python_http.py")
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        if !python.status.success() {
            failures.push(format!("V16 {phase} Python: {}{}", String::from_utf8_lossy(&python.stdout), String::from_utf8_lossy(&python.stderr)));
        }
        let titles: Vec<String> =
            db.query(&format!("SELECT title FROM {schema}.task ORDER BY title"), &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
        if titles != ["lost", "malformed", "py-1", "py-lost", "ts-1"] {
            failures.push(format!("V16 {phase} tasks {titles:?}: only committed bundles may leave rows (no ghost, no check-1/check-2, no py-ghost)"));
        }
        let archived: Vec<i64> =
            db.query(&format!("SELECT id FROM {schema}.project WHERE archived ORDER BY id"), &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
        if archived != [10, 12, 14, 15] {
            failures.push(format!("V16 {phase} archived {archived:?}"));
        }
        let logs: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.log WHERE name = 'p14'"), &[]).await.unwrap().get(0);
        if logs != 1 {
            failures.push(format!("V16 {phase} compose logs {logs}"));
        }
    }
    if let Err(e) = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await {
        failures.push(format!("V16 schema cleanup: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
