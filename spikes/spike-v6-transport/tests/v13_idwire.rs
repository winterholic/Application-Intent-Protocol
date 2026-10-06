use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_module_with_wire;
use std::{fs, process::Command};

const FIXTURE: &str = "
resource Member { fields { id: Id } }
actor Member
resource Parent {
  fields { id: Id; member: Member; title: Text }
  rows read when member = actor
  expose read { select id, title }
}
resource Item {
  fields { id: Id; member: Member; parent: Parent?; isChecked: Bool; count: Int; label: Text }
  rows read when member = actor
  transition read { allow member = actor; from isChecked = false; to isChecked = true; repeat unchanged }
  expose read {
    select id, member, parent, isChecked, count, label
    filter id.eq, label.eq
    sort id
    traverse parent { select id, title }
    budget { rows 50; depth 2; deadline 1s; cost 200 }
  }
  expose apply read { target id, where; bulk maxRows 20 }
}
";
const IDS: [i64; 7] = [0, 42, 9_007_199_254_740_991, 9_007_199_254_740_992, 9_007_199_254_740_993, 999_999_999_999_999_999, i64::MAX];

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
async fn v13_id_modes_preserve_identity_or_reject_before_commit() {
    sqlgen::set_schema("aip_v13");
    let facts: Value = load_str(FIXTURE, Form::A).unwrap_or_else(|e| panic!("V13 fixture: {e:?}")).execution;
    let schema = sqlgen::schema();
    let db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member (id) VALUES (1),(2);
      INSERT INTO {schema}.parent (id, member_id, title) VALUES (42,1,'small'),(9007199254740993,1,'large'),(88,2,'hidden');"
    ))
    .await
    .unwrap();
    for id in IDS {
        let label = match id {
            i64::MAX => "top",
            42 => "small",
            _ => "item",
        };
        let parent = match id {
            42 => Some(42_i64),
            9_007_199_254_740_993 => Some(9_007_199_254_740_993),
            0 => Some(88),
            _ => None,
        };
        db.execute(
            &format!("INSERT INTO {schema}.item (id, member_id, parent_id, is_checked, count, label) VALUES ($1,1,$2,false,17,$3)"),
            &[&id, &parent, &label],
        )
        .await
        .unwrap();
    }
    db.execute(&format!("INSERT INTO {schema}.item (id, member_id, parent_id, is_checked, count, label) VALUES (777,2,NULL,false,17,'hidden')"), &[])
        .await
        .unwrap();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let mut failures = Vec::new();
    for (wire, filename) in [(IdWire::SafeNumber, "generated-v13-safe.ts"), (IdWire::DecimalString, "generated-v13-string.ts")] {
        fs::write(format!("{dir}/{filename}"), contract_module_with_wire(&facts, "../../spike-v5-sdk/sdk/generic.ts", wire)).unwrap();
    }
    let checked = tsc(dir, "v13-type-tests.ts");
    if !checked.status.success() {
        failures.push(format!("V13 type fixtures: {}", String::from_utf8_lossy(&checked.stdout)));
    }
    let source = fs::read_to_string(format!("{dir}/v13-type-tests.ts")).unwrap();
    assert!(source.contains("@ts-expect-error"));
    let control_file = format!("{dir}/.v13-negative-control.ts");
    fs::write(&control_file, source.replace("@ts-expect-error", "")).unwrap();
    let negative = tsc(dir, ".v13-negative-control.ts");
    if negative.status.success() || !String::from_utf8_lossy(&negative.stdout).contains(".v13-negative-control.ts") {
        failures.push("V13 marker 제거 음성 대조 미검출".into());
    }
    fs::remove_file(control_file).unwrap();
    let (legacy_port, legacy_keys) = spike_v6_transport::start(facts.clone()).await;
    let (safe_port, safe_keys) = spike_v6_transport::start_with_id_wire(facts.clone(), IdWire::SafeNumber).await;
    let (string_port, string_keys) = spike_v6_transport::start_with_id_wire(facts, IdWire::DecimalString).await;
    let tokens = [legacy_keys.issue(1, 600), safe_keys.issue(1, 600), string_keys.issue(1, 600), string_keys.issue(2, 600)];
    for phase in ["legacy", "safe", "string"] {
        let tokens = tokens.clone();
        let node = tokio::task::spawn_blocking(move || {
            Command::new("node")
                .current_dir(dir)
                .env("AIP_LEGACY_URL", format!("http://127.0.0.1:{legacy_port}"))
                .env("AIP_SAFE_URL", format!("http://127.0.0.1:{safe_port}"))
                .env("AIP_STRING_URL", format!("http://127.0.0.1:{string_port}"))
                .env("AIP_LEGACY_TOKEN", &tokens[0])
                .env("AIP_SAFE_TOKEN", &tokens[1])
                .env("AIP_STRING_TOKEN", &tokens[2])
                .env("AIP_OTHER_TOKEN", &tokens[3])
                .args(if phase == "legacy" {
                    vec!["--test".to_string(), "v13.legacy.test.ts".to_string(), "v13-binding.test.ts".to_string()]
                } else {
                    vec!["--test".to_string(), format!("v13.{phase}.test.ts")]
                })
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        let out = format!("{}{}", String::from_utf8_lossy(&node.stdout), String::from_utf8_lossy(&node.stderr));
        eprintln!("V13 phase {phase}: {}", out.lines().filter(|l| l.starts_with("ℹ ")).collect::<Vec<_>>().join("; "));
        if !node.status.success() {
            failures.push(format!("V13 {phase} HTTP: {out}"));
        }
        let changed: Vec<i64> =
            db.query(&format!("SELECT id FROM {schema}.item WHERE is_checked ORDER BY id"), &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
        let expected: Vec<i64> = match phase {
            "legacy" => vec![9_007_199_254_740_992],
            "safe" => vec![0, 42, 9_007_199_254_740_991],
            _ => IDS.to_vec(),
        };
        if changed != expected {
            failures.push(format!("V13 {phase} DB 변경행: {changed:?}, expected {expected:?}"));
        }
        db.batch_execute(&format!("UPDATE {schema}.item SET is_checked=false")).await.unwrap();
    }
    if let Err(e) = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await {
        failures.push(format!("V13 schema cleanup: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
