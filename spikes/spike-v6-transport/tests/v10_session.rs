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
";

async fn post(port: u16, path: &str, token: Option<&str>, body: &Value) -> Value {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let body = body.to_string();
    let authorization = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    stream.write_all(format!("POST {path} HTTP/1.1\r\n{authorization}content-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v10_session_confirmation_and_pending_retry_keep_principal_scope() {
    let mut failures = Vec::new();

    // Empty facts also allow session confirmation; principal does not depend on application policy.
    let (session_port, session_keys) = spike_v6_transport::start(json!({})).await;
    let t1 = session_keys.issue(1, 600);
    let negative_actor = session_keys.issue(-1, 600);
    let expired = session_keys.issue(1, -1);
    let wrong_key = spike_v6_transport::auth::Keyring::generate().issue(1, 600);
    let valid = post(session_port, "/session", Some(&t1), &json!({})).await;
    if valid["ok"] != true || valid["principal"]["actorId"] != "1" || !valid["remainingMs"].as_u64().is_some_and(|ms| (1..=600_000).contains(&ms)) {
        failures.push(format!("유효 세션 계약 불일치: {valid}"));
    }
    let negative = post(session_port, "/session", Some(&negative_actor), &json!({})).await;
    if negative["ok"] != true || negative["principal"]["actorId"] != "-1" {
        failures.push(format!("음수 signed principal은 canonical decimal로 반환해야 함: {negative}"));
    }
    let anonymous = post(session_port, "/session", None, &json!({})).await;
    if anonymous["ok"] != true || !anonymous["principal"]["actorId"].is_null() || !anonymous["remainingMs"].is_null() {
        failures.push(format!("익명 세션 계약 불일치: {anonymous}"));
    }
    for (name, token, code) in [
        ("변조/형식 오류", "not-a-token", "UNAUTHENTICATED"),
        ("다른 키", wrong_key.as_str(), "UNAUTHENTICATED"),
        ("만료", expired.as_str(), "TOKEN_EXPIRED"),
    ] {
        let result = post(session_port, "/session", Some(token), &json!({})).await;
        if result["code"] != code || !result.get("principal").is_none() {
            failures.push(format!("{name} 토큰 처리 불일치: {result}"));
        }
    }
    let body_actor = post(session_port, "/session", Some(&t1), &json!({ "actor": "2" })).await;
    if body_actor["code"] != "BAD_REQUEST" || !body_actor.get("principal").is_none() {
        failures.push(format!("본문 actor 거부 불일치: {body_actor}"));
    }

    sqlgen::set_schema("aip_v10");
    let schema = sqlgen::schema();
    let src = A
        .replacen("  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }", "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }", 1)
        .replacen(
            "  expose aggregate approvedCount\n",
            "  expose aggregate approvedCount\n  transition approve { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE\n    create ClubMember { club = recruitment.club; member = member; role = MEMBER }\n    notify member \"apply.approved\" }\n  expose apply approve { target id; bulk maxRows 100; sameScope recruitment.club }\n",
            1,
        );
    let facts = load_str(&format!("{src}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school (id, name) VALUES (1, 'A대');\
         INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1), (5, 1), (-1, 1);\
         INSERT INTO {schema}.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1);\
         INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');\
         INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note)\
           VALUES (100, 'A 모집', clock_timestamp() + interval '30 days', 'PUBLISHED', 0, 10, 'note');\
         INSERT INTO {schema}.apply (id, recruitment_id, member_id, status) VALUES (300, 100, 5, 'PENDING');\
         INSERT INTO {schema}.member_alarm (id, member_id, is_checked)\
           VALUES (1, 1, false), (2, 1, false), (3, 1, false), (4, 1, false), (5, 1, false), (6, 1, false), (7, -1, false);"
    ))
    .await
    .unwrap();

    let (port, keys) = spike_v6_transport::start(facts).await;
    let actor_minus_one = keys.issue(-1, 600);
    let body = json!({ "request": { "apply": "MemberAlarm.read", "target": { "ids": ["7"] } }, "key": "signed-minus-one" });
    let signed_write = post(port, "/apply", Some(&actor_minus_one), &body).await;
    if signed_write["ok"] != true {
        failures.push(format!("signed actor -1 쓰기 실패: {signed_write}"));
    }
    let anonymous_status = post(port, "/status", None, &json!({ "key": "signed-minus-one", "request": body["request"] })).await;
    if anonymous_status["code"] != "NOT_FOUND" {
        failures.push(format!("익명 status가 signed actor -1 결과를 공유함: {anonymous_status}"));
    }
    let anonymous_apply = post(port, "/apply", None, &body).await;
    if anonymous_apply["ok"] == true || anonymous_apply["replayed"] == true {
        failures.push(format!("익명 apply가 signed actor -1 결과를 공유함: {anonymous_apply}"));
    }

    let t1 = keys.issue(1, 600);
    let t1_new = keys.issue(1, 1200);
    let t1_short = keys.issue(1, 3);
    let t2 = keys.issue(2, 600);
    let expired = keys.issue(1, -1);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let node = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .current_dir(dir)
            .env("AIP_URL", format!("http://127.0.0.1:{port}"))
            .env("AIP_TOKEN_1", &t1)
            .env("AIP_TOKEN_1_NEW", &t1_new)
            .env("AIP_TOKEN_1_SHORT", &t1_short)
            .env("AIP_TOKEN_2", &t2)
            .env("AIP_TOKEN_EXPIRED", &expired)
            .args(["--test", "--test-concurrency=1", "session.e2e.test.ts"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&node.stdout).to_string();
    let stderr = String::from_utf8_lossy(&node.stderr).to_string();
    eprintln!("{}", stdout.lines().filter(|line| line.starts_with("ℹ ")).collect::<Vec<_>>().join("\n"));
    let members: i64 = db.query_one(format!("SELECT count(*) FROM {schema}.club_member WHERE member_id = 5").as_str(), &[]).await.unwrap().get(0);
    let outbox: i64 = db.query_one(format!("SELECT count(*) FROM {schema}.aip_outbox").as_str(), &[]).await.unwrap().get(0);
    let alarm_minus_one: bool =
        db.query_one(format!("SELECT is_checked FROM {schema}.member_alarm WHERE id = 7").as_str(), &[]).await.unwrap().get(0);
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.ok();

    if !node.status.success() {
        failures.push(format!("Node session recovery 실패:\n{stdout}{stderr}"));
    }
    if (members, outbox) != (1, 1) {
        failures.push(format!("pending replay가 중복 실행됨: club_member={members}, outbox={outbox}"));
    }
    if !alarm_minus_one {
        failures.push("signed actor -1 쓰기 결과가 저장되지 않음".into());
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
