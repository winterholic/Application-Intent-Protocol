//! V3-3: 아리아리 관리자 위임(entrustAdmin)을 W2(서버 정의 전이 + 갱신 효과)와 W0(공개 전이 묶음)로 비교한다.
//! W1은 compose에 selfRow(대상과 같은 범위의 요청자 자신 행)를 더해 표현한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::Duration;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

const COMMON: &str = "  rows read when member = actor or managerOf(actor, club)
  invariant oneAdmin per club deferred
  check keepsAdmin when hasAdmin(club)
";
const W2_DEF: &str = "  transition entrust {
    allow isAdmin(actor, club) and member != actor
    from role != ADMIN
    to role = ADMIN
    update ClubMember where club = club and member = actor { role = MEMBER }
  }
  expose apply entrust { target id; bulk maxRows 1 }
";
const W0_DEF: &str = "  transition makeAdmin { allow isAdmin(actor, club); from role != ADMIN; to role = ADMIN }
  transition resignAdmin { allow member = actor; from role = ADMIN; to role = MEMBER }
  expose apply makeAdmin { target id; bulk maxRows 1 }
  expose apply resignAdmin { target id; bulk maxRows 1 }
";
const W1_DEF: &str = "  expose compose { bulk maxRows 1; sameScope club; transitions makeAdmin, resignAdmin; selfRow member by club }
";
const TOP: &str = "limit oneAdmin on ClubMember = atMost 1 where role = ADMIN
predicate isAdmin(m: Member, c: Club) = exists ClubMember where club = c and member = m and role = ADMIN
predicate hasAdmin(c: Club) = exists ClubMember where club = c and role = ADMIN
";

fn facts() -> Value {
    let s = A.replacen(
        "  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n",
        &format!("  fields {{ id: Id; club: Club; member: Member; role: ClubRole }}\n{COMMON}{W2_DEF}{W0_DEF}{W1_DEF}"),
        1,
    );
    load_str(&format!("{s}\n{TOP}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

const SEED: &str = "
TRUNCATE S.aip_outbox, S.apply, S.recruitment_bookmark, S.recruitment, S.club_member, S.club, S.member, S.school RESTART IDENTITY CASCADE;
INSERT INTO S.school (id, name) VALUES (1, 'A대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 1), (4, 1), (5, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1), (11, 'B', NULL, 1);
INSERT INTO S.club_member (id, club_id, member_id, role) VALUES
 (1, 10, 1, 'ADMIN'), (2, 10, 2, 'MANAGER'), (3, 10, 3, 'MEMBER'), (4, 11, 4, 'ADMIN'), (5, 11, 5, 'MEMBER');
";

fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: NOW.into() }
}
fn ids(v: &[i64]) -> Value {
    json!({ "ids": v.iter().map(|x| x.to_string()).collect::<Vec<_>>() })
}
fn w2(target: i64) -> Value {
    json!({ "apply": "ClubMember.entrust", "target": ids(&[target]) })
}
fn w1(target: i64, steps: Value) -> Value {
    json!({ "compose": "ClubMember", "targets": ids(&[target]), "steps": steps })
}
fn w1_std(target: i64) -> Value {
    w1(target, json!([{ "transition": "makeAdmin" }, { "transition": "resignAdmin", "on": "self" }]))
}
fn w0(ops: &[(&str, i64)]) -> Value {
    json!({ "atomic": ops.iter().map(|(t, id)| json!({ "apply": format!("ClubMember.{t}"), "target": ids(&[*id]) })).collect::<Vec<_>>() })
}
async fn roles(db: &tokio_postgres::Client) -> String {
    let s = sqlgen::schema();
    db.query_one(format!("SELECT string_agg(id || ':' || role, ',' ORDER BY id) FROM {s}.club_member").as_str(), &[]).await.unwrap().get(0)
}
fn code<T>(r: Result<T, spike_v2_read::plan::Reject>) -> String {
    r.map(|_| "OK".into()).unwrap_or_else(|e| e.code.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v3_3_admin_delegation() {
    sqlgen::set_schema("aip_v3_3");
    let s = sqlgen::schema();
    let f = facts();
    let mut db = connect().await;
    for st in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    let seed = SEED.replace("S.", &format!("{s}."));
    let k = Knobs::default();
    let start = "1:ADMIN,2:MANAGER,3:MEMBER,4:ADMIN,5:MEMBER";
    let mut fails: Vec<String> = vec![];
    let mut table: Vec<String> = vec![];
    // (이름, actor, W2 요청, W0 요청, 기대 W2, 기대 W0, 기대 최종 역할(성공 시))
    let cases: Vec<(&str, i64, Value, Value, &str, &str, &str)> = vec![
        ("정상 위임", 1, w2(3), w0(&[("makeAdmin", 3), ("resignAdmin", 1)]), "OK", "OK", "1:MEMBER,2:MANAGER,3:ADMIN,4:ADMIN,5:MEMBER"),
        ("자기 자신에게 위임", 1, w2(1), w0(&[("makeAdmin", 1), ("resignAdmin", 1)]), "FORBIDDEN", "INVALID_STATE", start),
        ("관리자 아닌 운영진", 2, w2(3), w0(&[("makeAdmin", 3), ("resignAdmin", 2)]), "FORBIDDEN", "FORBIDDEN", start),
        ("다른 동아리 회원에게", 1, w2(5), w0(&[("makeAdmin", 5), ("resignAdmin", 1)]), "MISSING_TARGET", "MISSING_TARGET", start),
        ("이미 관리자에게", 1, w2(4), w0(&[("makeAdmin", 4), ("resignAdmin", 1)]), "MISSING_TARGET", "MISSING_TARGET", start),
    ];
    for (name, actor, q2, q0, want2, want0, want_roles) in &cases {
        let target = q2["target"]["ids"][0].as_str().unwrap().parse::<i64>().unwrap();
        let q1 = w1_std(target);
        for (w, q, want) in [("W2", q2, want2), ("W0", q0, want0), ("W1", &q1, want0)] {
            db.batch_execute(&seed).await.unwrap();
            let got = match w {
                "W2" => code(apply(&mut db, &f, q, &who(*actor), &k).await),
                "W0" => code(bundle(&mut db, &f, q, &who(*actor), &k).await),
                _ => code(compose(&mut db, &f, q, &who(*actor), &k).await),
            };
            let r = roles(&db).await;
            let want_r = if *want == "OK" { *want_roles } else { start };
            table.push(format!("{name} | {w} | {got} | {r}"));
            if got != *want || r != want_r {
                fails.push(format!("{name} {w}: 기대 {want} / {want_r}, 실제 {got} / {r}"));
            }
        }
    }
    // W0 고유: 순서·생략
    let specific: Vec<(&str, Value, &str)> = vec![
        // 먼저 내려놓으면 actor는 더 이상 운영진이 아니라 대상 행이 보이지 않는다(V3-R1: 없는 대상과 같게).
        ("W0 내려놓고 임명(순서 바꿈)", w0(&[("resignAdmin", 1), ("makeAdmin", 3)]), "MISSING_TARGET"),
        ("W0 임명만(관리자 2명)", w0(&[("makeAdmin", 3)]), "INVARIANT_VIOLATED"),
        ("W0 내려놓기만(관리자 0명)", w0(&[("resignAdmin", 1)]), "CHECK_FAILED"),
    ];
    let w1_specific: Vec<(&str, Value, &str)> = vec![
        (
            "W1 내려놓고 임명(순서 바꿈)",
            w1(3, json!([{ "transition": "resignAdmin", "on": "self" }, { "transition": "makeAdmin" }])),
            "MISSING_TARGET",
        ),
        ("W1 임명만", w1(3, json!([{ "transition": "makeAdmin" }])), "INVARIANT_VIOLATED"),
        ("W1 내려놓기만", w1(3, json!([{ "transition": "resignAdmin", "on": "self" }])), "CHECK_FAILED"),
        ("W1 자기 행 대신 대상에 내려놓기", w1(3, json!([{ "transition": "makeAdmin" }, { "transition": "resignAdmin" }])), "FORBIDDEN"),
    ];
    for (name, q, want) in &w1_specific {
        db.batch_execute(&seed).await.unwrap();
        let got = code(compose(&mut db, &f, q, &who(1), &k).await);
        let r = roles(&db).await;
        table.push(format!("{name} | W1 | {got} | {r}"));
        if got != *want || r != start {
            fails.push(format!("{name}: 기대 {want}, 실제 {got} / {r}"));
        }
    }
    for (name, q, want) in &specific {
        db.batch_execute(&seed).await.unwrap();
        let got = code(bundle(&mut db, &f, q, &who(1), &k).await);
        let r = roles(&db).await;
        table.push(format!("{name} | W0 | {got} | {r}"));
        if got != *want || r != start {
            fails.push(format!("{name}: 기대 {want}, 실제 {got} / {r}"));
        }
    }
    // 위임 뒤 원래 관리자가 다시 위임
    db.batch_execute(&seed).await.unwrap();
    code(apply(&mut db, &f, &w2(3), &who(1), &k).await);
    let again = code(apply(&mut db, &f, &w2(2), &who(1), &k).await);
    table.push(format!("위임 뒤 원래 관리자가 재위임 | W2 | {again}"));
    if again != "MISSING_TARGET" {
        fails.push(format!("재위임: {again}"));
    }
    // 동시 위임: 같은 관리자가 2와 3에게 동시에. 하나만 성공하고 관리자는 한 명
    for w in ["W2", "W0", "W1"] {
        db.batch_execute(&seed).await.unwrap();
        let (r1, r2) = race(&f, w).await;
        let r = roles(&db).await;
        let admins10 = r.split(',').filter(|x| ["1:ADMIN", "2:ADMIN", "3:ADMIN"].contains(x)).count();
        table.push(format!("동시 위임 | {w} | T1 {r1}, T2 {r2} | {r}"));
        let oks = [&r1, &r2].iter().filter(|x| x.as_str() == "OK").count();
        if oks != 1 || admins10 != 1 {
            fails.push(format!("동시 위임 {w}: T1 {r1}, T2 {r2}, {r}"));
        }
    }
    eprintln!("{}", table.join("\n"));
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

async fn race(f: &Value, w: &'static str) -> (String, String) {
    let (f1, f2) = (f.clone(), f.clone());
    let t1 = tokio::spawn(async move {
        let mut db = connect().await;
        let k = Knobs { pause_after_lock_ms: 400, ..Default::default() };
        match w {
            "W2" => code(apply(&mut db, &f1, &w2(2), &who(1), &k).await),
            "W0" => code(bundle(&mut db, &f1, &w0(&[("makeAdmin", 2), ("resignAdmin", 1)]), &who(1), &k).await),
            _ => code(compose(&mut db, &f1, &w1_std(2), &who(1), &k).await),
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut db = connect().await;
    let r2 = match w {
        "W2" => code(apply(&mut db, &f2, &w2(3), &who(1), &Knobs::default()).await),
        "W0" => code(bundle(&mut db, &f2, &w0(&[("makeAdmin", 3), ("resignAdmin", 1)]), &who(1), &Knobs::default()).await),
        _ => code(compose(&mut db, &f2, &w1_std(3), &who(1), &Knobs::default()).await),
    };
    (t1.await.unwrap(), r2)
}
