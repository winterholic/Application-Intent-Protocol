use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, sqlgen};
use spike_v5_sdk::contract_module;
use std::{fs, process::Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  transition read { allow member = actor; from isChecked = false; to isChecked = true; repeat unchanged }
  expose read { select id, isChecked; sort id; budget { rows 100; depth 1; deadline 1s; cost 100 } }
  expose apply read { target id; bulk maxRows 10 }
}
";
const SEED: &str = "
INSERT INTO S.school (id, name) VALUES (1, 'A대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (5, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES (100, 'A 모집', clock_timestamp() + interval '30 days', 'PUBLISHED', 0, 10, 'note');
INSERT INTO S.apply (id, recruitment_id, member_id, status) VALUES (300, 100, 5, 'PENDING');
INSERT INTO S.member_alarm (id, member_id, is_checked) VALUES (1, 1, false), (2, 1, false);
";
const TSC: &str = "/Users/winterholic/development/projects/aip/spikes/spike-0-ts/node_modules/typescript/bin/tsc";

async fn read(port: u16, token: &str, query: Value) -> Value {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let body = json!({ "query": query }).to_string();
    stream
        .write_all(format!("POST /read HTTP/1.1\r\nauthorization: Bearer {token}\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn tsc(dir: &str, file: &str) -> std::process::Output {
    Command::new(TSC)
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
        .unwrap_or_else(|e| panic!("tsc 실행 실패: {e}"))
}

fn output_text(output: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v12_generated_contract_types_and_fingerprint_cross_the_http_cache_boundary() {
    sqlgen::set_schema("aip_v12");
    let schema = sqlgen::schema();
    let write_src = A
        .replacen(
            "  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }",
            "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }",
            1,
        )
        .replacen(
            "  expose aggregate approvedCount\n",
            "  expose aggregate approvedCount\n  transition approve { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE\n    create ClubMember { club = recruitment.club; member = member; role = MEMBER }\n    notify member \"apply.approved\" }\n  expose apply approve { target id; bulk maxRows 100; sameScope recruitment.club }\n",
            1,
        );
    let facts = load_str(&format!("{write_src}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("V12 fixture 오류: {e:?}")).execution;

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let generated_path = format!("{dir}/generated-v12.ts");
    let generated = contract_module(&facts, "../../spike-v5-sdk/sdk/generic.ts");
    fs::write(&generated_path, generated).expect("generated-v12.ts 쓰기");

    let mut failures = Vec::new();
    let positive = tsc(dir, "v12-type-tests.ts");
    if !positive.status.success() {
        failures.push(format!("정상 타입 예제 실패:\n{}", output_text(&positive)));
    }
    let negative = tsc(dir, "v12-type-neg.ts");
    if !negative.status.success() {
        failures.push(format!("@ts-expect-error 음성 예제의 검사가 실패:\n{}", output_text(&negative)));
    }
    let negative_path = format!("{dir}/.v12-type-neg-control.ts");
    let negative_source = fs::read_to_string(format!("{dir}/v12-type-neg.ts")).unwrap();
    let marker_count = negative_source.matches("@ts-expect-error").count();
    if marker_count == 0 {
        failures.push("음성 대조에 @ts-expect-error가 없어 marker 제거 대조를 만들 수 없음".into());
    } else {
        fs::write(&negative_path, negative_source.replace("@ts-expect-error", "")).unwrap();
        let unchecked = tsc(dir, ".v12-type-neg-control.ts");
        let unchecked_text = output_text(&unchecked);
        if unchecked.status.success() || !unchecked_text.contains(".v12-type-neg-control.ts") {
            failures.push(format!("expect-error 제거 음성 대조가 실패하지 않음:\n{unchecked_text}"));
        }
        if let Err(e) = fs::remove_file(&negative_path) {
            failures.push(format!("임시 음성 대조 파일 제거 실패: {e}"));
        }
    }

    let db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&SEED.replace("S.", &format!("{schema}."))).await.unwrap();
    let (port, keys) = spike_v6_transport::start(facts).await;
    let t1 = keys.issue(1, 600);
    let t1_new = keys.issue(1, 1200);
    let t2 = keys.issue(2, 600);

    let fingerprint_reply = read(port, &t1, json!({ "read": "MemberAlarm", "select": ["id", "isChecked"] })).await;
    let fingerprint = fingerprint_reply["contractFingerprint"].as_str().unwrap_or("");
    if !fingerprint_reply["ok"].as_bool().unwrap_or(false)
        || fingerprint.len() != 64
        || !fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        failures.push(format!("HTTP read contractFingerprint는 lowercase 64자리 hex여야 함: {fingerprint_reply}"));
    }

    let node = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .current_dir(dir)
            .env("AIP_URL", format!("http://127.0.0.1:{port}"))
            .env("AIP_TOKEN_1", &t1)
            .env("AIP_TOKEN_1_NEW", &t1_new)
            .env("AIP_TOKEN_2", &t2)
            .args(["--test", "--test-concurrency=1", "v12-contract.test.ts", "v12.e2e.test.ts"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&node.stdout).to_string();
    let stderr = String::from_utf8_lossy(&node.stderr).to_string();
    if !node.status.success() {
        failures.push(format!("V12 Node HTTP/계약 테스트 실패:\n{stdout}{stderr}"));
    }
    eprintln!("{}", stdout.lines().filter(|line| line.starts_with("ℹ ") || line.starts_with("not ok")).collect::<Vec<_>>().join("\n"));

    if let Err(e) = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await {
        failures.push(format!("V12 테스트 schema 제거 실패: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
