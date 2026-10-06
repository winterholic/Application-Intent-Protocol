//! V3-2: 아리아리 일괄 승인을 W2(서버 정의 전이)·W0(공개 동작 묶음)·W1(서버 선언 범위의 조합)로 같은 반례 표에 돌린다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::Duration;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

/// 세 후보가 서버에 써야 하는 정의. 줄 수 비교에 그대로 쓴다.
const W2_DEF: &str = "  transition approve {
    allow managerOf(actor, recruitment.club)
    from status = PENDING
    to status = APPROVE
    create ClubMember { club = recruitment.club; member = member; role = MEMBER }
    notify member \"apply.approved\"
  }
  expose apply approve { target id; bulk maxRows 100; sameScope recruitment.club }
";
const W0_DEF: &str = "  transition approveOnly {
    allow managerOf(actor, recruitment.club)
    from status = PENDING
    to status = APPROVE
  }
  expose apply approveOnly { target id; bulk maxRows 100; sameScope recruitment.club }
";
const W1_DEF: &str =
    "  expose compose { bulk maxRows 100; sameScope recruitment.club; transitions approveOnly; create ClubMember from member, recruitment.club }
";
// R3-02: W0·W1이 W2와 같은 업무(MEMBER 역할, 승인하면 반드시 회원)를 지키려면 아래가 더 필요하다.
const CM_CREATE: &str = "  expose create { allow managerOf(actor, club) and role = MEMBER; fields club, member, role }\n";
const CM_CHECK: &str = "  check hasApproval when approvedApplicant(this)\n";
const APPLY_CHECK: &str = "  check approvedHasMember when status != APPROVE or joined(this)\n";
const PRED: &str =
    "predicate approvedApplicant(cm: ClubMember) = exists Apply where member = cm.member and recruitment.club = cm.club and status = APPROVE
predicate joined(a: Apply) = exists ClubMember where member = a.member and club = a.recruitment.club
";

fn facts(with_check: bool) -> Value {
    let mut s = A.replacen(
        "  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }",
        "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }",
        1,
    );
    let apply_check = if with_check { APPLY_CHECK } else { "" };
    s = s.replacen("  expose aggregate approvedCount\n", &format!("  expose aggregate approvedCount\n{W2_DEF}{W0_DEF}{W1_DEF}{apply_check}"), 1);
    let cm = format!("  rows read when member = actor\n  unique club, member\n{CM_CREATE}{}", if with_check { CM_CHECK } else { "" });
    s = s.replacen(
        "  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n",
        &format!("  fields {{ club: Club; member: Member; role: ClubRole }}\n{cm}"),
        1,
    );
    s.push_str(PRED);
    load_str(&s, Form::A).unwrap_or_else(|e| panic!("facts {e:?}")).execution
}

const SEED: &str = "
TRUNCATE S.aip_outbox, S.apply, S.recruitment_bookmark, S.recruitment, S.club_member, S.club, S.member, S.school RESTART IDENTITY CASCADE;
INSERT INTO S.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (5, 1), (6, 1), (7, 2), (8, 1), (9, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 2);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'MANAGER'), (10, 8, 'ADMIN'), (11, 8, 'ADMIN');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL),
 (103, 'A 지난 모집', '2026-09-01T00:00:00Z', 'CLOSED', 0, 10, NULL);
INSERT INTO S.apply (id, recruitment_id, member_id, status) VALUES
 (300, 100, 5, 'PENDING'), (301, 100, 6, 'PENDING'), (302, 101, 7, 'PENDING'),
 (303, 100, 2, 'PENDING'), (304, 100, 7, 'REJECT'), (305, 103, 9, 'PENDING'), (306, 103, 5, 'PENDING');
";

#[derive(Clone, Copy, Debug, PartialEq)]
enum W {
    W2,
    W0,
    W1,
}

fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: NOW.into() }
}
fn sids(ids: &[i64]) -> Vec<String> {
    ids.iter().map(|x| x.to_string()).collect()
}

/// 정직한 호출자가 각 후보로 "이 지원들을 승인"을 표현한 요청.
fn approve_req(w: W, ids: &[i64]) -> Value {
    // W0 호출자는 회원 생성 값(동아리·회원 id)을 스스로 알아서 보내야 한다. seed 값을 그대로 쓴다.
    let member_of = |id: i64| match id {
        300 => 5,
        301 => 6,
        302 => 7,
        303 => 2,
        304 => 7,
        305 => 9,
        306 => 5,
        _ => 999,
    };
    let club_of = |id: i64| if id == 302 { 11 } else { 10 };
    match w {
        W::W2 => json!({ "apply": "Apply.approve", "target": { "ids": sids(ids) } }),
        W::W0 => {
            let mut ops = vec![json!({ "apply": "Apply.approveOnly", "target": { "ids": sids(ids) } })];
            for id in ids {
                ops.push(json!({ "create": "ClubMember", "values": { "club": club_of(*id).to_string(), "member": member_of(*id).to_string(), "role": "MEMBER" } }));
            }
            json!({ "atomic": ops })
        }
        W::W1 => json!({ "compose": "Apply", "targets": { "ids": sids(ids) }, "steps": [
            { "transition": "approveOnly" },
            { "create": "ClubMember", "values": { "club": { "item": "recruitment.club" }, "member": { "item": "member" }, "role": { "const": "MEMBER" } } }
        ] }),
    }
}

async fn run(db: &mut tokio_postgres::Client, f: &Value, w: W, req: &Value, actor: i64) -> String {
    let k = Knobs::default();
    let r = match w {
        W::W2 => apply(db, f, req, &who(actor), &k).await.map(|a| format!("{:?}", a.changed)),
        W::W0 => bundle(db, f, req, &who(actor), &k)
            .await
            .map(|b| format!("{:?}+{}", b.applied.first().map(|a| a.changed.clone()).unwrap_or_default(), b.created)),
        W::W1 => compose(db, f, req, &who(actor), &k)
            .await
            .map(|b| format!("{:?}+{}", b.applied.first().map(|a| a.changed.clone()).unwrap_or_default(), b.created)),
    };
    match r {
        Ok(s) => format!("OK {s}"),
        Err(e) => e.code.to_string(),
    }
}

async fn state(db: &tokio_postgres::Client) -> String {
    let s = sqlgen::schema();
    let q = |sql: String| sql;
    let approved: String = db
        .query_one(q(format!("SELECT coalesce(string_agg(id::text, ',' ORDER BY id), '') FROM {s}.apply WHERE status = 'APPROVE'")).as_str(), &[])
        .await
        .unwrap()
        .get(0);
    let members: String = db
        .query_one(q(format!("SELECT coalesce(string_agg(club_id || ':' || member_id, ',' ORDER BY club_id, member_id), '') FROM {s}.club_member WHERE role = 'MEMBER'")).as_str(), &[])
        .await
        .unwrap()
        .get(0);
    let outbox: i64 = db.query_one(q(format!("SELECT count(*) FROM {s}.aip_outbox")).as_str(), &[]).await.unwrap().get(0);
    format!("approved[{approved}] members[{members}] outbox{outbox}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v3_2_approval_w0_w1_w2() {
    sqlgen::set_schema("aip_v3_2");
    let s = sqlgen::schema();
    let fc = facts(true);
    let fn_ = facts(false);
    let mut db = connect().await;
    for st in sqlgen::ddl(&fc).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    let seed = SEED.replace("S.", &format!("{s}."));
    let mut fails: Vec<String> = vec![];
    let mut table: Vec<String> = vec![];

    // 공통 반례 표: (이름, actor, 대상, 기대 결과, 기대 DB 상태(W2), 기대 DB 상태(W0·W1))
    let base = "approved[] members[10:2] outbox0";
    type Case = (&'static str, i64, Vec<i64>, &'static str, &'static str, &'static str);
    let cases: Vec<Case> = vec![
        (
            "정상 승인",
            1,
            vec![300, 301],
            "OK",
            "approved[300,301] members[10:2,10:5,10:6] outbox2",
            "approved[300,301] members[10:2,10:5,10:6] outbox0",
        ),
        (
            "같은 동아리 다른 모집",
            1,
            vec![300, 305],
            "OK",
            "approved[300,305] members[10:2,10:5,10:9] outbox2",
            "approved[300,305] members[10:2,10:5,10:9] outbox0",
        ),
        ("다른 동아리 섞기(양쪽 운영진)", 8, vec![300, 302], "NOT_SAME_SCOPE", base, base),
        ("일반 회원이 승인", 2, vec![300], "MISSING_TARGET", base, base),
        ("일부 없는 id", 1, vec![300, 999], "MISSING_TARGET", base, base),
        ("이미 회원 포함", 1, vec![300, 303], "ALREADY_EXISTS", base, base),
        ("이전 상태 아님(거절됨)", 1, vec![304], "INVALID_STATE", base, base),
        ("빈 목록", 1, vec![], "BAD_REQUEST", base, base),
    ];
    for (name, actor, ids, want, st2, st01) in &cases {
        for w in [W::W2, W::W0, W::W1] {
            db.batch_execute(&seed).await.unwrap();
            let got = run(&mut db, &fc, w, &approve_req(w, ids), *actor).await;
            let st = state(&db).await;
            let want_st = if w == W::W2 { st2 } else { st01 };
            let ok = got.starts_with(want) && st == *want_st;
            table.push(format!("{name} | {w:?} | {got} | {st}"));
            if !ok {
                fails.push(format!("{name} {w:?}: 기대 {want} / {want_st}, 실제 {got} / {st}"));
            }
        }
    }

    // 후보별 반례: 호출자가 서버 의도와 다른 회원을 만들려는 정상 형식 요청
    let forged_w0 = json!({ "atomic": [
        { "apply": "Apply.approveOnly", "target": { "ids": ["300"] } },
        { "create": "ClubMember", "values": { "club": "10", "member": "9", "role": "MEMBER" } }
    ] });
    let create_only_w0 = json!({ "atomic": [{ "create": "ClubMember", "values": { "club": "10", "member": "9", "role": "MEMBER" } }] });
    let w1 = |steps: Value| json!({ "compose": "Apply", "targets": { "ids": ["300"] }, "steps": steps });
    let tr = json!({ "transition": "approveOnly" });
    let mk = |member: Value| json!({ "create": "ClubMember", "values": { "club": { "item": "recruitment.club" }, "member": member, "role": { "const": "MEMBER" } } });
    let specific: Vec<(&str, W, &Value, Value, i64, &str)> = vec![
        ("W0 승인+지원 안 한 회원 생성", W::W0, &fc, forged_w0.clone(), 1, "CHECK_FAILED"),
        ("W0 승인+지원 안 한 회원 생성(check 없음)", W::W0, &fn_, forged_w0, 1, "OK"),
        ("W0 회원 생성만", W::W0, &fc, create_only_w0.clone(), 1, "CHECK_FAILED"),
        ("W0 회원 생성만(check 없음)", W::W0, &fn_, create_only_w0.clone(), 1, "OK"),
        ("W0 일반 회원이 회원 생성", W::W0, &fn_, create_only_w0, 2, "FORBIDDEN"),
        ("W1 member를 상수로", W::W1, &fc, w1(json!([tr, mk(json!({ "const": "9" }))])), 1, "VALUE_SOURCE_NOT_ALLOWED"),
        ("W1 선언 안 된 출처 경로", W::W1, &fc, w1(json!([tr, mk(json!({ "item": "recruitment.club.school" }))])), 1, "VALUE_SOURCE_NOT_ALLOWED"),
        ("W1 허용 안 된 전이 단계", W::W1, &fc, w1(json!([{ "transition": "approve" }])), 1, "STEP_NOT_ALLOWED"),
        ("W1 승인 단계 빼고 생성만", W::W1, &fc, w1(json!([mk(json!({ "item": "member" }))])), 1, "CHECK_FAILED"),
        ("W1 승인 단계 빼고 생성만(check 없음)", W::W1, &fn_, w1(json!([mk(json!({ "item": "member" }))])), 1, "OK"),
        ("W1 생성 뒤 승인(순서 바꿈)", W::W1, &fc, w1(json!([mk(json!({ "item": "member" })), tr.clone()])), 1, "OK"),
        // R3-02: 승인만 하고 회원을 만들지 않음, 관리자 역할로 생성
        ("W1 승인만", W::W1, &fc, w1(json!([tr.clone()])), 1, "CHECK_FAILED"),
        ("W0 승인만", W::W0, &fc, json!({ "atomic": [{ "apply": "Apply.approveOnly", "target": { "ids": ["300"] } }] }), 1, "CHECK_FAILED"),
        (
            "W1 role ADMIN",
            W::W1,
            &fc,
            w1(
                json!([tr.clone(), { "create": "ClubMember", "values": { "club": { "item": "recruitment.club" }, "member": { "item": "member" }, "role": { "const": "ADMIN" } } }]),
            ),
            1,
            "FORBIDDEN",
        ),
        (
            "W0 role ADMIN",
            W::W0,
            &fc,
            json!({ "atomic": [
            { "apply": "Apply.approveOnly", "target": { "ids": ["300"] } },
            { "create": "ClubMember", "values": { "club": "10", "member": "5", "role": "ADMIN" } }] }),
            1,
            "FORBIDDEN",
        ),
    ];
    for (name, w, f, req, actor, want) in &specific {
        db.batch_execute(&seed).await.unwrap();
        let got = run(&mut db, f, *w, req, *actor).await;
        let st = state(&db).await;
        table.push(format!("{name} | {w:?} | {got} | {st}"));
        if !got.starts_with(want) {
            fails.push(format!("{name}: 기대 {want}, 실제 {got} / {st}"));
        }
    }

    // 동시성: 같은 지원 동시 승인, 다른 지원으로 같은 회원 동시 생성
    for (name, ids1, ids2, want2) in
        [("같은 지원 동시 승인", vec![300], vec![300], "INVALID_STATE"), ("같은 회원 다른 지원 동시 승인", vec![300], vec![306], "ALREADY_EXISTS")]
    {
        for w in [W::W2, W::W1] {
            db.batch_execute(&seed).await.unwrap();
            let (r1, r2) = race(&fc, w, &ids1, &ids2).await;
            let st = state(&db).await;
            table.push(format!("{name} | {w:?} | T1 {r1}, T2 {r2} | {st}"));
            // 안전 성질: 정확히 하나만 성공하고 다른 쪽은 기대 오류, 회원은 한 명. 어느 쪽이 이기는지는 타이밍에 따른다.
            let outcomes = [r1.as_str(), r2.as_str()];
            let one_ok = outcomes.iter().filter(|r| r.starts_with("OK")).count() == 1;
            let one_err = outcomes.iter().filter(|r| r.starts_with(want2)).count() == 1;
            if !one_ok || !one_err || st.matches("10:5").count() != 1 {
                fails.push(format!("{name} {w:?}: T1 {r1}, T2 {r2}, {st}"));
            }
        }
    }

    eprintln!("{}", table.join("\n"));
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

async fn race(f: &Value, w: W, ids1: &[i64], ids2: &[i64]) -> (String, String) {
    let (f1, f2) = (f.clone(), f.clone());
    let (q1, q2) = (approve_req(w, ids1), approve_req(w, ids2));
    let t1 = tokio::spawn(async move {
        let mut db = connect().await;
        let k = Knobs { pause_after_lock_ms: 400, ..Default::default() };
        let r = match w {
            W::W2 => apply(&mut db, &f1, &q1, &who(1), &k).await.map(|_| ()),
            _ => compose(&mut db, &f1, &q1, &who(1), &k).await.map(|_| ()),
        };
        r.map(|_| "OK".to_string()).unwrap_or_else(|e| e.code.to_string())
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut db = connect().await;
    let r2 = match w {
        W::W2 => apply(&mut db, &f2, &q2, &who(8), &Knobs::default()).await.map(|_| ()),
        _ => compose(&mut db, &f2, &q2, &who(8), &Knobs::default()).await.map(|_| ()),
    };
    (t1.await.unwrap(), r2.map(|_| "OK".to_string()).unwrap_or_else(|e| e.code.to_string()))
}
