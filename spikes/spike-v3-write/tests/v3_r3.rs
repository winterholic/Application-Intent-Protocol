//! Codex r3(plan-docs/reviews/codex-v3-r3.md) R3-01·R3-03·R3-05 재발 방지.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::{Duration, Instant};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts(row_read_true: bool) -> Value {
    let mut s = A.replacen(
        "  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }",
        "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }",
        1,
    );
    if row_read_true {
        s = s.replacen("  rows read when managerOf(actor, recruitment.club)", "  rows read when true", 1);
    }
    let apply_add = "  transition approve { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE
    create ClubMember { club = recruitment.club; member = member; role = MEMBER } }
  expose apply approve { target id; bulk maxRows 100; sameScope recruitment.club }
  transition approveOnly { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE }
  expose apply approveOnly { target id; bulk maxRows 100; sameScope recruitment.club }
  transition retract { allow managerOf(actor, recruitment.club); from status = APPROVE; to status = REJECT }
  expose apply retract { target id; bulk maxRows 100 }
  expose compose { bulk maxRows 100; sameScope recruitment.club; transitions approveOnly; create ClubMember from member, recruitment.club }
";
    s = s.replacen("  expose aggregate approvedCount\n", &format!("  expose aggregate approvedCount\n{apply_add}"), 1);
    s = s.replacen(
        "  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n",
        "  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n  unique club, member\n  expose create { allow managerOf(actor, club) and role = MEMBER; fields club, member, role }\n  check hasApproval when approvedApplicant(this)\n",
        1,
    );
    s.push_str(
        "predicate approvedApplicant(cm: ClubMember) = exists Apply where member = cm.member and recruitment.club = cm.club and status = APPROVE\n",
    );
    load_str(&s, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

const SEED: &str = "
TRUNCATE S.aip_outbox, S.apply, S.recruitment_bookmark, S.recruitment, S.club_member, S.club, S.member, S.school RESTART IDENTITY CASCADE;
INSERT INTO S.school (id, name) VALUES (1, 'A대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (5, 1), (7, 1), (8, 1), (9, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1), (11, 'B', NULL, 1);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 8, 'ADMIN'), (11, 8, 'ADMIN');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL), (101, 'B', '2026-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL);
INSERT INTO S.apply (id, recruitment_id, member_id, status) VALUES
 (300, 100, 5, 'PENDING'), (302, 101, 7, 'PENDING'), (304, 100, 7, 'REJECT'), (305, 100, 9, 'APPROVE');
";

fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: NOW.into() }
}
fn code<T>(r: Result<T, spike_v2_read::plan::Reject>) -> String {
    r.map(|_| "OK".to_string()).unwrap_or_else(|e| e.code.to_string())
}
fn ids(v: &[&str]) -> Value {
    json!(v)
}

async fn all3(db: &mut tokio_postgres::Client, f: &Value, actor: i64, t: &[&str]) -> String {
    let k = Knobs::default();
    let w2 = code(apply(db, f, &json!({ "apply": "Apply.approve", "target": { "ids": ids(t) } }), &who(actor), &k).await);
    let w0 = code(bundle(db, f, &json!({ "atomic": [{ "apply": "Apply.approveOnly", "target": { "ids": ids(t) } }] }), &who(actor), &k).await);
    let w1 = code(
        compose(db, f, &json!({ "compose": "Apply", "targets": { "ids": ids(t) }, "steps": [{ "transition": "approveOnly" }] }), &who(actor), &k)
            .await,
    );
    format!("{w2}/{w0}/{w1}")
}

async fn state(db: &tokio_postgres::Client, s: &str) -> String {
    let a: String = db
        .query_one(format!("SELECT coalesce(string_agg(id || ':' || status, ',' ORDER BY id), '') FROM {s}.apply").as_str(), &[])
        .await
        .unwrap()
        .get(0);
    let m: String = db
        .query_one(
            format!("SELECT coalesce(string_agg(club_id || ':' || member_id, ',' ORDER BY club_id, member_id), '') FROM {s}.club_member").as_str(),
            &[],
        )
        .await
        .unwrap()
        .get(0);
    format!("apply[{a}] members[{m}]")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r3_regressions() {
    sqlgen::set_schema("aip_v3_r3");
    let s = sqlgen::schema();
    let f = facts(false);
    let mut db = connect().await;
    for st in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    let seed = SEED.replace("S.", &format!("{s}."));
    let mut fails: Vec<String> = vec![];

    // R3-01 복합 위반의 오류 우선순위가 세 후보에서 같다(권한·상태 판정이 같은 범위 검사보다 먼저)
    db.batch_execute(&seed).await.unwrap();
    let got = all3(&mut db, &f, 8, &["304", "302"]).await;
    if got != "INVALID_STATE/INVALID_STATE/INVALID_STATE" {
        fails.push(format!("R3-01 거절됨+다른 동아리: {got}"));
    }
    let ft = facts(true);
    db.batch_execute(&seed).await.unwrap();
    let got = all3(&mut db, &ft, 1, &["300", "302"]).await;
    if got != "FORBIDDEN/FORBIDDEN/FORBIDDEN" {
        fails.push(format!("R3-01 rowRead true + 권한 없는 동아리 섞임: {got}"));
    }

    // R3-03 커밋 검사의 근거 행(승인된 Apply 305)은 검사 뒤 커밋까지 잠긴다. 철회는 회원 생성 커밋 뒤에 반영된다.
    db.batch_execute(&seed).await.unwrap();
    let (lock_wait, final_state) = check_race(&f, false).await;
    if !(lock_wait >= 400) {
        fails.push(format!("R3-03 철회가 검사 근거 잠금을 기다리지 않음 {lock_wait}ms"));
    }
    // 사후조건 계약: 커밋 시점에는 근거가 있었고, 이후 철회는 별개 쓰기다. 회원은 남는다(전역 불변식 아님).
    if final_state != "apply[300:PENDING,302:PENDING,304:REJECT,305:REJECT] members[10:1,10:8,10:9,11:8]" {
        fails.push(format!("R3-03 최종 상태 {final_state}"));
    }

    // R3-05 커밋을 보낸 뒤 기한 초과는 COMMIT_UNKNOWN. 커밋 전 기한 부족은 rollback 후 DEADLINE_EXCEEDED
    db.batch_execute(&seed).await.unwrap();
    db.batch_execute(&format!(
        "CREATE FUNCTION {s}.slow() RETURNS trigger AS $$ BEGIN PERFORM pg_sleep(1.1); RETURN NULL; END $$ LANGUAGE plpgsql;
         CREATE CONSTRAINT TRIGGER slow_cm AFTER INSERT ON {s}.club_member DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.slow();"
    ))
    .await
    .unwrap();
    let k = Knobs { pause_after_lock_ms: 1200, ..Default::default() };
    let r = code(apply(&mut db, &f, &json!({ "apply": "Apply.approve", "target": { "ids": ["300"] } }), &who(1), &k).await);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let st = state(&db, s).await;
    if r != "COMMIT_UNKNOWN" || !st.contains("300:APPROVE") {
        fails.push(format!("R3-05 커밋 중 기한 초과: {r} / {st}"));
    }
    db.batch_execute(&seed).await.unwrap();
    let k = Knobs { pause_before_commit_ms: 1850, ..Default::default() };
    let r = code(apply(&mut db, &f, &json!({ "apply": "Apply.approve", "target": { "ids": ["300"] } }), &who(1), &k).await);
    let st = state(&db, s).await;
    if r != "DEADLINE_EXCEEDED" || st.contains("300:APPROVE") {
        fails.push(format!("R3-05 커밋 전 기한 부족: {r} / {st}"));
    }

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

/// T1: 운영진 1이 승인된 지원 305(회원 9)를 근거로 W0 회원 생성(검사 뒤 커밋 전 600ms 멈춤). T2: 그 사이 305 철회.
async fn check_race(f: &Value, skip_policy_lock: bool) -> (u128, String) {
    let s = sqlgen::schema();
    let f1 = f.clone();
    let t1 = tokio::spawn(async move {
        let mut db = connect().await;
        let k = Knobs { pause_before_commit_ms: 600, skip_policy_lock, ..Default::default() };
        let req = json!({ "atomic": [{ "create": "ClubMember", "values": { "club": "10", "member": "9", "role": "MEMBER" } }] });
        code(bundle(&mut db, &f1, &req, &who(1), &k).await)
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut db = connect().await;
    let st = Instant::now();
    let r2 = code(apply(&mut db, f, &json!({ "apply": "Apply.retract", "target": { "ids": ["305"] } }), &who(1), &Knobs::default()).await);
    let waited = st.elapsed().as_millis();
    let r1 = t1.await.unwrap();
    assert_eq!((r1.as_str(), r2.as_str()), ("OK", "OK"), "check_race 결과");
    (waited, state(&db, s).await)
}
