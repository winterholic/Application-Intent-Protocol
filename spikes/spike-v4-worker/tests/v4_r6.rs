//! Codex r6(plan-docs/reviews/codex-v34-r6.md) 쓰기 확장 반례 재발 방지.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v4_worker::write::invoke_write;
use spike_v4_worker::{Lang, Worker};
use std::time::Duration;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const CM: &str = "  fields { id: Id; club: Club; member: Member; role: ClubRole }
  rows read when member = actor or managerOf(actor, club)
  invariant oneAdmin per club deferred
  check keepsAdmin when hasAdmin(club)
  transition makeAdmin { allow isAdmin(actor, club); from role != ADMIN; to role = ADMIN }
  transition resignAdmin { allow member = actor; from role = ADMIN; to role = MEMBER }
  expose apply makeAdmin { target id; bulk maxRows 1 }
  expose apply resignAdmin { target id; bulk maxRows 1 }
  expose apply touch { target id; bulk maxRows 1 }
  transition touch { allow member = actor; from true; to role = MEMBER }
  extension write delegate { input { target: ClubMember.Id; self: ClubMember.Id } output { done: Bool } access ClubMember.makeAdmin, ClubMember.resignAdmin effect db deadline 1s implementation \"club.IMPL\" }
";
const TOP: &str = "limit oneAdmin on ClubMember = atMost 1 where role = ADMIN
predicate isAdmin(m: Member, c: Club) = exists ClubMember where club = c and member = m and role = ADMIN
predicate hasAdmin(c: Club) = exists ClubMember where club = c and role = ADMIN
";

fn facts(imp: &str) -> Value {
    let s = A.replacen("  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n", &CM.replace("IMPL", imp), 1);
    load_str(&format!("{s}\n{TOP}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}
const SEED: &str = "
TRUNCATE S.aip_outbox, S.apply, S.recruitment_bookmark, S.recruitment, S.club_member, S.club, S.member, S.school RESTART IDENTITY CASCADE;
INSERT INTO S.school (id, name) VALUES (1, 'A대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1);
INSERT INTO S.club_member (id, club_id, member_id, role) VALUES (1, 10, 1, 'ADMIN'), (2, 10, 2, 'MANAGER'), (3, 10, 3, 'MEMBER');
";
fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-10-04T00:00:00Z".into() }
}
fn short(r: Result<Value, spike_v2_read::plan::Reject>) -> String {
    match r {
        Ok(v) => format!("OK {v}"),
        Err(e) if e.code == "EXTENSION_ERROR" => format!("EXTENSION_ERROR({})", e.msg.rsplit(": ").next().unwrap_or("")),
        Err(e) => e.code.to_string(),
    }
}
async fn roles(db: &tokio_postgres::Client) -> String {
    let s = sqlgen::schema();
    db.query_one(format!("SELECT string_agg(id || ':' || role, ',' ORDER BY id) FROM {s}.club_member").as_str(), &[]).await.unwrap().get(0)
}

fn facts_d(imp: &str, deadline: &str) -> Value {
    let s = A.replacen(
        "  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n",
        &CM.replace("IMPL", imp).replace("deadline 1s", &format!("deadline {deadline}")),
        1,
    );
    load_str(&format!("{s}\n{TOP}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r6_write_extension() {
    sqlgen::set_schema("aip_v4_r6");
    let s = sqlgen::schema();
    let mut db = connect().await;
    for st in sqlgen::ddl(&facts("delegate")).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    let seed = SEED.replace("S.", &format!("{s}."));
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let done = "1:MEMBER,2:MANAGER,3:ADMIN";
    let input = json!({ "target": "3", "self": "1" });
    let mut fails = vec![];
    for lang in [Lang::Node, Lang::Python] {
        let mut w = Worker::start(lang, ext_dir).await;
        // F02: 커밋이 기한을 넘기면 성공이 아니라 결과 미확정. 실제로는 커밋됨.
        db.batch_execute(&seed).await.unwrap();
        db.batch_execute(&format!(
            "CREATE OR REPLACE FUNCTION {s}.slow() RETURNS trigger AS $$ BEGIN IF NEW.id = 1 THEN PERFORM pg_sleep(0.5); END IF; RETURN NULL; END $$ LANGUAGE plpgsql;
             DROP TRIGGER IF EXISTS slow_cm ON {s}.club_member;
             CREATE CONSTRAINT TRIGGER slow_cm AFTER UPDATE ON {s}.club_member DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.slow();"
        )).await.unwrap();
        let r = short(invoke_write(&mut db, &mut w, &facts("delegateAfter500"), "ClubMember.delegate", &input, &who(1)).await);
        tokio::time::sleep(Duration::from_millis(600)).await;
        let st = roles(&db).await;
        if r != "COMMIT_UNKNOWN" || st != done {
            fails.push(format!("{lang:?} F02 커밋 기한: {r} / {st}"));
        }
        db.batch_execute(&format!("DROP TRIGGER IF EXISTS slow_cm ON {s}.club_member")).await.unwrap();
        // F03: 기한이 지난 이전 호출의 늦은 ctx 쓰기가 다음 정상 호출을 실패시키지 않는다
        db.batch_execute(&seed).await.unwrap();
        let old = short(invoke_write(&mut db, &mut w, &facts_d("staleWriter", "50ms"), "ClubMember.delegate", &input, &who(1)).await);
        let next = short(invoke_write(&mut db, &mut w, &facts("delegateAfter350"), "ClubMember.delegate", &input, &who(1)).await);
        let st = roles(&db).await;
        if old != "DEADLINE_EXCEEDED" || next != "OK {\"done\":true}" || st != done {
            fails.push(format!("{lang:?} F03 이전 호출 간섭: {old} / {next} / {st}"));
        }
        // F05: ctx 쓰기 횟수 상한은 별도 확장 없이 정적 확인(write::MAX_CTX_CALLS = 20)
        w.stop().await;
    }
    // F04: Python에서 ctx 작업 대기를 취소해도 worker가 살아 있다
    let mut w = Worker::start(Lang::Python, ext_dir).await;
    db.batch_execute(&seed).await.unwrap();
    let locker = connect().await;
    locker.batch_execute(&format!("BEGIN; SELECT id FROM {s}.club_member WHERE id = 3 FOR UPDATE")).await.unwrap();
    let rel = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        locker.batch_execute("ROLLBACK").await.unwrap();
    });
    let c = short(invoke_write(&mut db, &mut w, &facts("cancelPending"), "ClubMember.delegate", &input, &who(1)).await);
    rel.await.unwrap();
    db.batch_execute(&seed).await.unwrap();
    let after = short(invoke_write(&mut db, &mut w, &facts("delegate"), "ClubMember.delegate", &input, &who(1)).await);
    eprintln!("F04 cancel={c} after={after}");
    if c == "WORKER_FAILED" || after != "OK {\"done\":true}" {
        fails.push(format!("F04 Python 취소: {c} / {after}"));
    }
    w.stop().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
