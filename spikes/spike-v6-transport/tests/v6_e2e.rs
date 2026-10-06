//! V6 전송 계층 end-to-end: Rust 서버 + TS 클라이언트(V5 캐시) + 로컬 PG.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, sqlgen};
use std::process::Command;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  transition read { allow member = actor; from isChecked = false; to isChecked = true; repeat unchanged }
  expose read { select id, isChecked; filter isChecked.eq; sort id; budget { rows 100; depth 1; deadline 1s; cost 100 } }
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
INSERT INTO S.member_alarm (id, member_id, is_checked) VALUES (1, 1, false), (2, 1, false), (3, 1, false), (4, 1, false), (5, 1, false), (6, 1, false);
";

#[test]
fn v9_v10_pending_recovery() {
    let out = Command::new("node")
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/client"))
        .args(["--test", "recovery.test.ts", "session.test.ts"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{}", stdout.lines().filter(|line| line.starts_with("ℹ ")).collect::<Vec<_>>().join("\n"));
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v6_transport_e2e() {
    sqlgen::set_schema("aip_v6");
    let s = sqlgen::schema();
    let src = A
        .replacen("  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }", "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }", 1)
        .replacen(
            "  expose aggregate approvedCount\n",
            "  expose aggregate approvedCount\n  transition approve { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE\n    create ClubMember { club = recruitment.club; member = member; role = MEMBER }\n    notify member \"apply.approved\" }\n  expose apply approve { target id; bulk maxRows 100; sameScope recruitment.club }\n",
            1,
        );
    let facts = load_str(&format!("{src}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let db = connect().await;
    for st in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&st).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&SEED.replace("S.", &format!("{s}."))).await.unwrap();
    let (port, keys) = spike_v6_transport::start(facts).await;
    let (t1, t2) = (keys.issue(1, 600), keys.issue(2, 600));
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/client");
    let out = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .current_dir(dir)
            .env("AIP_URL", format!("http://127.0.0.1:{port}"))
            .env("AIP_TOKEN_1", &t1)
            .env("AIP_TOKEN_2", &t2)
            .args(["--test", "--test-concurrency=1", "e2e.test.ts"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let txt = String::from_utf8_lossy(&out.stdout).to_string();
    eprintln!("{}", txt.lines().filter(|l| l.starts_with("ℹ ") || l.starts_with("not ok")).collect::<Vec<_>>().join("\n"));
    let members: i64 = db.query_one(format!("SELECT count(*) FROM {s}.club_member WHERE member_id = 5").as_str(), &[]).await.unwrap().get(0);
    let outbox: i64 = db.query_one(format!("SELECT count(*) FROM {s}.aip_outbox").as_str(), &[]).await.unwrap().get(0);
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(out.status.success(), "{txt}{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!((members, outbox), (1, 1), "응답 유실·재시도 뒤에도 회원 생성과 알림은 한 번");
}
