//! V4-3 쓰기 확장: 위임을 JS/Python 확장으로. 커밋은 서버가 조건을 모두 확인한 뒤에만 한다.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v4_3_write_extension() {
    sqlgen::set_schema("aip_v4_write");
    let s = sqlgen::schema();
    let mut db = connect().await;
    for st in sqlgen::ddl(&facts("delegate")).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    let seed = SEED.replace("S.", &format!("{s}."));
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let start = "1:ADMIN,2:MANAGER,3:MEMBER";
    let input = json!({ "target": "3", "self": "1" });
    let mut fails = vec![];
    let mut table = vec![];
    for lang in [Lang::Node, Lang::Python] {
        let mut w = Worker::start(lang, ext_dir).await;
        let cases: Vec<(&str, &str, i64, &str, &str)> = vec![
            ("정상 위임", "delegate", 1, "OK {\"done\":true}", "1:MEMBER,2:MANAGER,3:ADMIN"),
            ("관리자 아닌 운영진", "delegate", 2, "EXTENSION_ERROR(FORBIDDEN)", start),
            ("확장이 중간에 실패", "delegateThrow", 1, "EXTENSION_ERROR(EXTENSION_ERROR)", start),
            ("ctx 오류를 삼키고 성공 반환", "delegateSwallow", 1, "EXTENSION_ERROR(FORBIDDEN)", start),
            ("임명만(관리자 2명)", "onlyPromote", 1, "INVARIANT_VIOLATED", start),
            ("계약 밖 쓰기", "undeclared", 1, "EXTENSION_ERROR(ACCESS_NOT_DECLARED)", start),
        ];
        for (name, imp, actor, want, want_roles) in &cases {
            db.batch_execute(&seed).await.unwrap();
            let got = short(invoke_write(&mut db, &mut w, &facts(imp), "ClubMember.delegate", &input, &who(*actor)).await);
            let r = roles(&db).await;
            table.push(format!("{lang:?} | {name} | {got} | {r}"));
            if got != *want || r != *want_roles {
                fails.push(format!("{lang:?} {name}: 기대 {want} / {want_roles}, 실제 {got} / {r}"));
            }
        }
        // 기한 초과: 첫 쓰기 뒤 멈춤 → rollback. 그 뒤 붙잡아 둔 ctx로 늦은 쓰기 → 거부, 아무것도 안 바뀜
        db.batch_execute(&seed).await.unwrap();
        let slow = short(invoke_write(&mut db, &mut w, &facts("delegateSlow"), "ClubMember.delegate", &input, &who(1)).await);
        let r1 = roles(&db).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        let late = short(invoke_write(&mut db, &mut w, &facts("lateWrite"), "ClubMember.delegate", &input, &who(1)).await);
        let r2 = roles(&db).await;
        table.push(format!("{lang:?} | 기한 초과 | {slow} | {r1}"));
        table.push(format!("{lang:?} | 끝난 호출의 ctx로 늦은 쓰기 | {late} | {r2}"));
        if slow != "DEADLINE_EXCEEDED" || r1 != start || late != "EXTENSION_ERROR(TOKEN_EXPIRED)" || r2 != start {
            fails.push(format!("{lang:?} 기한/늦은 쓰기: {slow} {r1} / {late} {r2}"));
        }
        w.stop().await;
    }
    eprintln!("{}", table.join("\n"));
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
