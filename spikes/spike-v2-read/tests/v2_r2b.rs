//! Codex r2b(plan-docs/reviews/codex-v2v3-r2b.md) 읽기 쪽 반례 재발 방지.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  expose read { select id, isChecked; filter isChecked.eq; budget { rows 100; depth 1; deadline 1s; cost 100 } }
}
access managerSomewhere(a: Member) = exists ClubMember where member = a and role in (ADMIN, MANAGER)
";

fn facts(old: &str, new: &str) -> Value {
    assert!(old.is_empty() || A.matches(old).count() == 1, "{old}");
    let src = if old.is_empty() { A.to_string() } else { A.replacen(old, new, 1) };
    load_str(&format!("{src}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}
fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}
fn code(r: Result<Vec<Value>, spike_v2_read::plan::Reject>) -> String {
    match r {
        Ok(v) => format!("OK{}", v.len()),
        Err(e) => e.code.to_string(),
    }
}

const SEED: &str = "
INSERT INTO S.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (4, NULL);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', NULL, 2), (12, '연합', 'u.png', NULL);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (11, 3, 'ADMIN');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'A-note'),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, 'B-note'),
 (102, '연합 모집', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, 'U-note');
INSERT INTO S.recruitment_bookmark (recruitment_id, member_id) VALUES (100, 1), (100, 2), (100, 3), (102, 4);
INSERT INTO S.member_alarm (id, member_id, is_checked) VALUES (1, 1, false), (2, 1, true), (3, 2, false);
";

#[tokio::test(flavor = "current_thread")]
async fn r2b_read_regressions() {
    sqlgen::set_schema("aip_v2_r2b");
    let base = facts("", "");
    let mut db = connect().await;
    for s in sqlgen::ddl(&base).unwrap() {
        db.batch_execute(&s).await.unwrap_or_else(|e| panic!("DDL {s}: {e}"));
    }
    db.batch_execute(&SEED.replace("S.", &format!("{}.", sqlgen::schema()))).await.expect("seed");
    let mut fails: Vec<String> = vec![];
    let mut check = |name: &str, got: String, want: &str| {
        if got != want {
            fails.push(format!("{name}: 기대 {want}, 실제 {got}"));
        }
    };
    let ids_req = json!({ "read": "Recruitment", "select": ["id"], "limit": 20 });

    // R2B-02 단독 Bool 조건
    let t = facts("rows read when active(this) and\n    (club.school = null or club.school = actor.school)", "rows read when true");
    check("rows true", code(execute(&mut db, &plan_read(&t, &ids_req, &who(None)).unwrap()).await), "OK3");
    let fz = facts("rows read when active(this) and\n    (club.school = null or club.school = actor.school)", "rows read when false");
    check("rows false", code(execute(&mut db, &plan_read(&fz, &ids_req, &who(Some(1))).unwrap()).await), "OK0");

    // R2B-03 요청 구조 중복과 출력 크기
    let dup = json!({ "read": "Recruitment", "select": ["id", "id"], "limit": 1 });
    check("select 중복", plan_read(&base, &dup, &who(Some(1))).err().map(|e| e.code.to_string()).unwrap_or("OK".into()), "DUPLICATE");
    let dupf = json!({ "read": "Recruitment", "select": ["id"], "limit": 1,
        "filter": [{ "field": "periodEnd", "op": "gte", "value": NOW }, { "field": "periodEnd", "op": "gte", "value": NOW }] });
    check("filter 중복", plan_read(&base, &dupf, &who(Some(1))).err().map(|e| e.code.to_string()).unwrap_or("OK".into()), "DUPLICATE");
    let dups = json!({ "read": "Recruitment", "select": ["id"], "limit": 1, "sort": [{ "field": "id" }, { "field": "id" }] });
    check("sort 중복", plan_read(&base, &dups, &who(Some(1))).err().map(|e| e.code.to_string()).unwrap_or("OK".into()), "DUPLICATE");
    db.batch_execute(&format!("UPDATE {}.recruitment SET internal_note = repeat('x', 1048576) WHERE id = 100", sqlgen::schema())).await.unwrap();
    let big = json!({ "read": "Recruitment", "select": ["internalNote"], "limit": 1, "filter": [{ "field": "periodEnd", "op": "gte", "value": "2026-10-09T00:00:00Z" }] });
    check("출력 상한", code(execute(&mut db, &plan_read(&base, &big, &who(Some(1))).unwrap()).await), "OUTPUT_TOO_LARGE");

    // R2B-05 읽기 Bool filter
    let unread = json!({ "read": "MemberAlarm", "select": ["id"], "filter": [{ "field": "isChecked", "op": "eq", "value": false }] });
    check("Bool filter", code(execute(&mut db, &plan_read(&base, &unread, &who(Some(1))).unwrap()).await), "OK1");

    // R2B-08 행별 집계 guard sourceAccess
    let g = facts("sourceAccess fixedTotalOfVisibleRecruitment", "sourceAccess managerSomewhere(actor)");
    let bc = json!({ "read": "Recruitment", "select": ["id", "bookmarkCount"], "sort": [{ "field": "id" }], "limit": 20 });
    let rows = execute(&mut db, &plan_read(&g, &bc, &who(Some(2))).unwrap()).await.unwrap();
    check("guard 거짓이면 null", format!("{:?}", rows.iter().map(|r| r["bookmarkCount"].clone()).collect::<Vec<_>>()), "[Null, Null]");
    let rows = execute(&mut db, &plan_read(&g, &bc, &who(Some(1))).unwrap()).await.unwrap();
    check("guard 참이면 count", format!("{:?}", rows.iter().map(|r| r["bookmarkCount"].clone()).collect::<Vec<_>>()), "[Number(3), Number(1)]");
    let rows = execute(&mut db, &plan_read(&base, &bc, &who(Some(2))).unwrap()).await.unwrap();
    check("totalOfVisible는 그대로", format!("{:?}", rows.iter().map(|r| r["bookmarkCount"].clone()).collect::<Vec<_>>()), "[Number(3), Number(1)]");

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
