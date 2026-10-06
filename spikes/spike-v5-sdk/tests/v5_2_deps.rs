//! V5-2 캐시 무효화 태그(RK-09). 읽기 deps가 결과에 영향을 주는 모든 resource를 포함하는지(건전성) DB 변경으로 확인한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  expose read { select id, isChecked; filter isChecked.eq; budget { rows 100; depth 1; deadline 1s; cost 100 } }
}
";
const SEED: &str = "
TRUNCATE S.member_alarm, S.apply, S.recruitment_bookmark, S.recruitment, S.club_member, S.club, S.member, S.school RESTART IDENTITY CASCADE;
INSERT INTO S.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (12, '연합', NULL, NULL);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'A-note'), (102, '연합 모집', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, NULL);
INSERT INTO S.recruitment_bookmark (recruitment_id, member_id) VALUES (100, 2);
INSERT INTO S.apply (id, recruitment_id, status) VALUES (200, 100, 'APPROVE');
INSERT INTO S.member_alarm (id, member_id, is_checked) VALUES (1, 1, false);
";

#[tokio::test(flavor = "current_thread")]
async fn v5_2_read_deps_are_sound() {
    sqlgen::set_schema("aip_v5");
    let s = sqlgen::schema();
    let f: Value = load_str(&format!("{A}\n{ALARM}"), Form::A).unwrap().execution;
    let mut db = connect().await;
    for st in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&st).await.unwrap();
    }
    let seed = SEED.replace("S.", &format!("{s}."));
    let caller = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let q = json!({ "read": "Recruitment", "select": ["id", "title", "internalNote", "bookmarkCount", { "club": { "select": ["name", "logo"] } }], "sort": [{ "field": "id" }] });
    let plan = plan_read(&f, &q, &caller).unwrap();
    let deps = plan.deps.clone();
    eprintln!("deps = {deps:?}");
    // 각 resource 하나만 바꾸는 쓰기. 결과가 바뀌면 그 resource는 deps에 있어야 한다.
    let mutations: Vec<(&str, String)> = vec![
        ("Recruitment", format!("UPDATE {s}.recruitment SET title = '바뀐 제목' WHERE id = 100")),
        ("Club", format!("UPDATE {s}.club SET logo = 'b.png' WHERE id = 10")),
        ("RecruitmentBookmark", format!("INSERT INTO {s}.recruitment_bookmark (recruitment_id, member_id) VALUES (100, 1)")),
        ("ClubMember", format!("DELETE FROM {s}.club_member WHERE club_id = 10 AND member_id = 1")),
        ("Member", format!("UPDATE {s}.member SET school_id = 2 WHERE id = 1")),
        ("School", format!("UPDATE {s}.school SET name = 'A대학교' WHERE id = 1")),
        ("Apply", format!("UPDATE {s}.apply SET status = 'REJECT' WHERE id = 200")),
        ("MemberAlarm", format!("UPDATE {s}.member_alarm SET is_checked = true WHERE id = 1")),
    ];
    let mut fails = vec![];
    let mut table = vec![];
    for (res, sql) in &mutations {
        db.batch_execute(&seed).await.unwrap();
        let before = execute(&mut db, &plan).await.unwrap();
        db.batch_execute(sql).await.unwrap();
        let after = execute(&mut db, &plan).await.unwrap();
        let changed = before != after;
        let in_deps = deps.iter().any(|d| d == res);
        table.push(format!("{res}: 결과 변화 {changed}, deps 포함 {in_deps}"));
        if changed && !in_deps {
            fails.push(format!("{res} 변경이 결과를 바꿨는데 deps에 없음(캐시가 낡은 값을 보여 줌)"));
        }
    }
    eprintln!("{}", table.join("\n"));
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
