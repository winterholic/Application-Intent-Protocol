use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, sqlgen};
use std::process::Command;
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
resource Permission {
  fields { id: Id; member: Member; granted: Bool }
  rows read when member = actor
  transition revoke { allow member = actor; from granted = true; to granted = false; repeat unchanged }
  expose apply revoke { target id; bulk maxRows 10 }
}
resource ManagerNote {
  fields { id: Id; note: Text }
  rows read when exists Permission where member = actor and granted = true
  expose read { select id, note; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
";

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v9_read_lifetimes_and_expiry_cross_the_http_sdk_boundary() {
    sqlgen::set_schema("aip_v9_lifetime");
    let schema = sqlgen::schema();
    let facts = load_str(&format!("{A}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school (id, name) VALUES (1, 'A대');\
         INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1);\
         INSERT INTO {schema}.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1), (11, 'B', NULL, 1);\
         INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (11, 1, 'MANAGER');\
         INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES\
           (100, '미래 모집', clock_timestamp() + interval '30 days', 'PUBLISHED', 0, 10, 'note'),\
           (101, '마감 모집', clock_timestamp() - interval '1 day', 'PUBLISHED', 0, 11, 'note');\
         INSERT INTO {schema}.member_alarm (id, member_id, is_checked) VALUES (1, 1, false), (2, 1, false);\
         INSERT INTO {schema}.permission (id, member_id, granted) VALUES (1, 1, true);\
         INSERT INTO {schema}.manager_note (id, note) VALUES (1, 'secret');"
    ))
    .await
    .unwrap();

    let (port, keys) = spike_v6_transport::start(facts).await;
    let token = keys.issue(1, 600);
    let alarm = read(port, &token, json!({ "read": "MemberAlarm", "select": ["id", "isChecked"], "sort": [{ "field": "id" }] })).await;
    let recruitment = read(port, &token, json!({ "read": "Recruitment", "select": ["id"], "sort": [{ "field": "id" }] })).await;

    let alarm_age = alarm["maxAgeMs"].as_u64();
    let recruitment_ids: Vec<i64> = recruitment["rows"].as_array().unwrap().iter().map(|row| row["id"].as_i64().unwrap()).collect();
    let mut contract_failures = Vec::new();
    if !alarm["ok"].as_bool().unwrap_or(false) || !alarm_age.is_some_and(|age| (1..=1000).contains(&age)) {
        contract_failures.push(format!("알림 maxAgeMs는 1..=1000이어야 함: {alarm}"));
    }
    if !recruitment["ok"].as_bool().unwrap_or(false) || recruitment["maxAgeMs"].as_u64() != Some(0) || recruitment_ids != [100] {
        contract_failures.push(format!("마감 모집은 숨기고 시간 의존 조회는 미캐시해야 함: {recruitment}"));
    }

    let expiring = keys.issue(1, 2);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let node = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .current_dir(dir)
            .env("AIP_URL", format!("http://127.0.0.1:{port}"))
            .env("AIP_TOKEN", &token)
            .env("AIP_EXPIRING_TOKEN", &expiring)
            .args(["--test", "--test-concurrency=1", "lifetime.e2e.test.ts"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&node.stdout).to_string();
    let stderr = String::from_utf8_lossy(&node.stderr).to_string();
    eprintln!("{}", stdout.lines().filter(|line| line.starts_with("ℹ ")).collect::<Vec<_>>().join("\n"));
    let checked: bool = db.query_one(&format!("SELECT is_checked FROM {schema}.member_alarm WHERE id = 1"), &[]).await.unwrap().get(0);
    let granted: bool = db.query_one(&format!("SELECT granted FROM {schema}.permission WHERE id = 1"), &[]).await.unwrap().get(0);
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.ok();

    assert!(contract_failures.is_empty(), "{}", contract_failures.join("\n"));
    assert!(node.status.success(), "{stdout}{stderr}");
    assert!(checked && !granted, "다른 연결의 변경과 권한 회수가 실제 DB에 반영되어야 함");
}
