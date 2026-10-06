//! Codex r2b(plan-docs/reviews/codex-v2v3-r2b.md) 쓰기 쪽 반례 재발 방지.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, Applied, Knobs};
use std::time::{Duration, Instant};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool; title: Text? }
  rows read when member = actor
  transition read { allow member = actor; from isChecked = false; to isChecked = true; repeat unchanged }
  transition clearTitle { allow member = actor; from isChecked = true; to title = null; repeat unchanged }
  transition touch { allow member = actor; from isChecked = false or isChecked = true; to isChecked = true; repeat unchanged }
  expose read { select id, isChecked; filter isChecked.eq; budget { rows 100; depth 1; deadline 1s; cost 100 } }
  expose apply read { target id, where; bulk maxRows 10 }
  expose apply clearTitle { target id; bulk maxRows 10 }
  expose apply touch { target id; bulk maxRows 10 }
}
";
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts() -> Value {
    let a = A.replacen(
        "  invariant atMostOnePublished per club",
        "  expose apply close { target id; bulk maxRows 10 }\n  invariant atMostOnePublished per club",
        1,
    );
    load_str(&format!("{a}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("facts {e:?}")).execution
}
fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: NOW.into() }
}
fn req(name: &str, ids: &[i64]) -> Value {
    json!({ "apply": name, "target": { "ids": ids.iter().map(|x| x.to_string()).collect::<Vec<_>>() } })
}
fn short(r: Result<Applied, spike_v2_read::plan::Reject>) -> String {
    match r {
        Ok(a) => format!("changed{:?} unchanged{:?}", a.changed, a.unchanged),
        Err(e) => e.code.to_string(),
    }
}

const SEED: &str = "
INSERT INTO S.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 2), (14, 'A3동아리', NULL, 1);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (11, 3, 'ADMIN'), (14, 1, 'MANAGER');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, NULL),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, NULL),
 (103, 'A3 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 14, NULL);
INSERT INTO S.member_alarm (id, member_id, is_checked, title) VALUES
 (1, 1, true, '공지'), (2, 1, false, NULL), (3, 1, true, '모집'), (4, 1, false, NULL);
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2b_write_regressions() {
    sqlgen::set_schema("aip_v3_r2b");
    let s = sqlgen::schema();
    let f = facts();
    let mut db = connect().await;
    for st in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    db.batch_execute(&SEED.replace("S.", &format!("{s}."))).await.expect("seed");
    let k = Knobs::default();
    let mut fails: Vec<String> = vec![];
    let mut check = |name: &str, got: String, want: &str| {
        if got != want {
            fails.push(format!("{name}: 기대 {want}, 실제 {got}"));
        }
    };

    // R2B-09 nullable 필드에 null 대입
    check("null 대입", short(apply(&mut db, &f, &req("MemberAlarm.clearTitle", &[1, 3]), &who(1), &k).await), "changed[1, 3] unchanged[]");
    check("null 대입 재시도", short(apply(&mut db, &f, &req("MemberAlarm.clearTitle", &[1]), &who(1), &k).await), "changed[] unchanged[1]");
    // R2B-10 from과 to가 겹쳐도 이미 목표 상태면 unchanged
    check("겹치는 전이", short(apply(&mut db, &f, &req("MemberAlarm.touch", &[1]), &who(1), &k).await), "changed[] unchanged[1]");
    // R2B-04 where 대상은 from 충족 행만. 이미 읽은 알림만 고르면 대상 없음
    let read_true = json!({ "apply": "MemberAlarm.read", "target": { "where": [{ "field": "isChecked", "op": "eq", "value": true }] } });
    check("where 이미 목표 상태", short(apply(&mut db, &f, &read_true, &who(1), &k).await), "changed[] unchanged[]");

    // R2B-11 다른 트랜잭션이 잠근 숨은 행: 기다리지 않고 없는 행과 같은 결과
    let locker = connect().await;
    locker.batch_execute(&format!("BEGIN; SELECT id FROM {s}.recruitment WHERE id = 101 FOR UPDATE")).await.unwrap();
    let st = Instant::now();
    let r = short(apply(&mut db, &f, &req("Recruitment.close", &[101]), &who(1), &k).await);
    check("잠긴 숨은 행", format!("{r} wait<500ms={}", st.elapsed().as_millis() < 500), "MISSING_TARGET wait<500ms=true");
    locker.batch_execute("ROLLBACK").await.unwrap();

    // R2B-07 잠금 대기가 기한을 넘으면 DEADLINE_EXCEEDED
    locker.batch_execute(&format!("BEGIN; SELECT id FROM {s}.member_alarm WHERE id = 2 FOR UPDATE")).await.unwrap();
    let r = short(apply(&mut db, &f, &req("MemberAlarm.read", &[2]), &who(1), &k).await);
    check("잠금 대기 기한", r, "DEADLINE_EXCEEDED");
    locker.batch_execute("ROLLBACK").await.unwrap();

    // R2B-06 쓰기 판정이 읽은 권한 행(ClubMember)은 커밋까지 잠긴다. 회수는 진행 중 쓰기 뒤에 반영된다.
    let (closed_first, revoke_waited) = revoke_race(&f, false).await;
    check("권한 회수 경합: 마감 성공", closed_first, "changed[103] unchanged[]");
    check("권한 회수 경합: 회수가 기다림", revoke_waited.to_string(), "true");
    let fin: String = db.query_one(format!("SELECT status FROM {s}.recruitment WHERE id = 103").as_str(), &[]).await.unwrap().get(0);
    let gone: i64 =
        db.query_one(format!("SELECT count(*) FROM {s}.club_member WHERE club_id = 14 AND member_id = 1").as_str(), &[]).await.unwrap().get(0);
    check("최종 상태", format!("{fin} membership={gone}"), "CLOSED membership=0");

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

/// T1: 관리자가 모집 103 마감(판정 뒤 600ms 멈춤). T2: 그 사이 같은 관리자의 ClubMember 행 삭제.
async fn revoke_race(f: &Value, skip_policy_lock: bool) -> (String, bool) {
    let s = sqlgen::schema();
    let f1 = f.clone();
    let t1 = tokio::spawn(async move {
        let mut db = connect().await;
        let k = Knobs { pause_after_lock_ms: 600, skip_row_lock: false, skip_policy_lock, pause_before_commit_ms: 0 };
        short(apply(&mut db, &f1, &req("Recruitment.close", &[103]), &who(1), &k).await)
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let revoker = connect().await;
    let st = Instant::now();
    revoker.batch_execute(&format!("DELETE FROM {s}.club_member WHERE club_id = 14 AND member_id = 1")).await.unwrap();
    let waited = st.elapsed().as_millis() >= 300;
    (t1.await.unwrap(), waited)
}
