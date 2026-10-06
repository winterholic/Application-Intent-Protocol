use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller, Reject};
use spike_v2_read::{connect, execute, sqlgen};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const HPY: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.h.py");
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts_of(src: &str, f: Form) -> Value {
    load_str(src, f).unwrap_or_else(|e| panic!("facts 실패 {e:?}")).execution
}
fn facts() -> Value {
    facts_of(A, Form::A)
}
fn variant(old: &str, new: &str) -> Value {
    assert_eq!(A.matches(old).count(), 1, "변형 대상 `{old}`");
    facts_of(&A.replacen(old, new, 1), Form::A)
}
fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}

fn list_req() -> Value {
    json!({
        "read": "Recruitment",
        "select": ["id", "title", "periodEnd", "bookmarkCount", "internalNote", { "club": { "select": ["id", "name", "logo"] } }],
        "sort": [{ "field": "periodEnd", "dir": "asc" }],
        "limit": 20
    })
}

fn code(r: Result<spike_v2_read::plan::Plan, Reject>) -> &'static str {
    match r {
        Ok(_) => "OK",
        Err(e) => e.code,
    }
}

#[test]
fn planner_rejects_without_db() {
    let f = facts();
    let c = who(Some(1));
    let mut req = list_req();
    type Case = (&'static str, Box<dyn Fn(&mut Value)>, &'static str);
    let cases: Vec<Case> = vec![
        // 열린 계약은 화면 요청만 바꿔도 된다(RK-01)
        ("logo만 빼도 됨", Box::new(|r| r["select"][5] = json!({ "club": { "select": ["id", "name"] } })), "OK"),
        ("닫힌 필드: club.school", Box::new(|r| r["select"][5] = json!({ "club": { "select": ["id", "school"] } })), "FIELD_NOT_EXPOSED"),
        ("닫힌 필드: status", Box::new(|r| r["select"] = json!(["id", "status"])), "FIELD_NOT_EXPOSED"),
        // select 권한이 filter/sort/traverse/aggregate를 자동 허용하지 않음(SY-3)
        (
            "선택 가능 internalNote로 filter",
            Box::new(|r| r["filter"] = json!([{ "field": "internalNote", "op": "eq", "value": "x" }])),
            "FILTER_NOT_ALLOWED",
        ),
        ("선택 가능 title로 sort", Box::new(|r| r["sort"] = json!([{ "field": "title" }])), "SORT_NOT_ALLOWED"),
        ("집계 bookmarkCount로 sort", Box::new(|r| r["sort"] = json!([{ "field": "bookmarkCount" }])), "SORT_NOT_ALLOWED"),
        (
            "허용 안 된 연산 periodEnd.eq",
            Box::new(|r| r["filter"] = json!([{ "field": "periodEnd", "op": "eq", "value": NOW }])),
            "FILTER_NOT_ALLOWED",
        ),
        ("계약에 없는 관계", Box::new(|r| r["select"] = json!(["id", { "school": { "select": ["id"] } }])), "TRAVERSE_NOT_ALLOWED"),
        (
            "관계 안 관계",
            Box::new(|r| r["select"][5] = json!({ "club": { "select": ["id", { "school": { "select": ["id"] } }] } })),
            "DEPTH_EXCEEDED",
        ),
        ("budget rows 초과", Box::new(|r| r["limit"] = json!(51)), "ROWS_EXCEEDED"),
        ("limit 0", Box::new(|r| r["limit"] = json!(0)), "BAD_VALUE"),
        ("Time 아닌 값", Box::new(|r| r["filter"] = json!([{ "field": "periodEnd", "op": "gte", "value": "'; DROP TABLE x; --" }])), "BAD_VALUE"),
        ("필드 이름 주입", Box::new(|r| r["filter"] = json!([{ "field": "periodEnd; DROP", "op": "gte", "value": NOW }])), "FILTER_NOT_ALLOWED"),
        ("모르는 요청 키", Box::new(|r| r["where"] = json!("1=1")), "UNKNOWN_KEY"),
        ("모르는 filter 키", Box::new(|r| r["filter"] = json!([{ "field": "periodEnd", "op": "gte", "value": NOW, "raw": true }])), "UNKNOWN_KEY"),
    ];
    let mut fails = vec![];
    for (name, m, want) in &cases {
        let mut r = req.clone();
        m(&mut r);
        let got = code(plan_read(&f, &r, &c));
        if got != *want {
            fails.push(format!("{name}: 기대 {want}, 실제 {got}"));
        }
    }
    req["read"] = json!("Club");
    if code(plan_read(&f, &req, &c)) != "NOT_ROOT_QUERYABLE" {
        fails.push("Club 루트 조회가 거부되지 않음".into());
    }
    if code(plan_read(&f, &json!({ "read": "RecruitmentBookmark", "select": ["member"] }), &c)) != "NOT_EXPOSED" {
        fails.push("북마크 개인 행 조회가 거부되지 않음".into());
    }
    if code(plan_read(&variant("cost 1000", "cost 50"), &list_req(), &c)) != "COST_EXCEEDED" {
        fails.push("cost 50 변형이 거부되지 않음".into());
    }
    // depth 1 + traverse 정의는 planner까지 오지 않고 V1 의미 검사에서 거부된다.
    if load_str(&A.replacen("depth 2;", "depth 1;", 1), Form::A).is_ok() {
        fails.push("depth 1 + traverse 정의가 의미 검사를 통과".into());
    }
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
    eprintln!("planner rejection cases: {}", cases.len() + 4);
}

#[test]
fn request_values_never_enter_sql_text() {
    let f = facts();
    let mut r = list_req();
    let probe = "2026-10-09T00:00:00Z";
    r["filter"] = json!([{ "field": "periodEnd", "op": "gte", "value": probe }]);
    let p = plan_read(&f, &r, &who(Some(1))).unwrap();
    assert!(!p.sql.contains(probe), "요청 값이 SQL 문자열에 들어감");
    let bound = "2026-10-09T00:00:00+00:00";
    assert!(!p.sql.contains(bound), "정규화한 값도 SQL 문자열에 들어가면 안 됨");
    assert!(p.params.iter().any(|x| x.as_deref() == Some(bound)));
    assert!(!p.sql.contains("PUBLISHED"), "enum 값도 매개변수여야 함");
}

#[test]
fn same_plan_from_a_and_h_python_facts() {
    let a = plan_read(&facts(), &list_req(), &who(Some(1))).unwrap();
    let h = plan_read(&facts_of(HPY, Form::HPy), &list_req(), &who(Some(1))).unwrap();
    assert_eq!(a.sql, h.sql);
    assert_eq!(a.params, h.params);
}

const SEED: &str = "
INSERT INTO aip_v2_spike.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO aip_v2_spike.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (4, NULL);
INSERT INTO aip_v2_spike.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', NULL, 2), (12, '연합', 'u.png', NULL), (13, 'A2동아리', NULL, 1);
INSERT INTO aip_v2_spike.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (13, 2, 'MEMBER'), (11, 3, 'ADMIN');
INSERT INTO aip_v2_spike.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'A-note'),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, 'B-note'),
 (102, '연합 모집', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, 'U-note'),
 (103, 'A 초안', '2026-10-20T00:00:00Z', 'DRAFT', 0, 10, NULL),
 (104, 'A2 마감', '2026-10-20T00:00:00Z', 'CLOSED', 0, 13, NULL),
 (105, 'A2 만료', '2026-10-01T00:00:00Z', 'PUBLISHED', 0, 13, NULL);
INSERT INTO aip_v2_spike.recruitment_bookmark (recruitment_id, member_id) VALUES (100, 1), (100, 2), (100, 3), (102, 4), (101, 3);
INSERT INTO aip_v2_spike.apply (id, recruitment_id, status) VALUES (200, 100, 'APPROVE'), (201, 100, 'PENDING'), (202, 103, 'APPROVE'), (203, 101, 'APPROVE');
";

fn ids(rows: &[Value]) -> Vec<i64> {
    rows.iter().map(|r| r["id"].as_i64().unwrap()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn db_vertical_slice() {
    let f = facts();
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL 실패 {stmt}: {e}"));
    }
    db.batch_execute(SEED).await.expect("seed");
    let mut fails: Vec<String> = vec![];
    macro_rules! check {
        ($c:expr, $($m:tt)*) => { if !$c { fails.push(format!($($m)*)); } };
    }

    // EQ-02 행 정책: 학교·익명·상태·기한
    for (actor, want) in [(Some(1), vec![102, 100]), (Some(2), vec![102, 100]), (Some(3), vec![102, 101]), (Some(4), vec![102]), (None, vec![102])] {
        let p = plan_read(&f, &list_req(), &who(actor)).unwrap();
        let rows = execute(&mut db, &p).await.unwrap();
        check!(ids(&rows) == want, "actor {actor:?}: 기대 {want:?}, 실제 {:?}", ids(&rows));
    }

    let p = plan_read(&f, &list_req(), &who(Some(1))).unwrap();
    let rows = execute(&mut db, &p).await.unwrap();
    let by = |id: i64| rows.iter().find(|r| r["id"] == json!(id)).cloned().unwrap_or(Value::Null);
    // EQ-05 내부 메모: 관리자인 동아리 행만
    check!(by(100)["internalNote"] == json!("A-note"), "관리자 internalNote {:?}", by(100)["internalNote"]);
    check!(by(102)["internalNote"].is_null(), "타 동아리 internalNote 노출 {:?}", by(102)["internalNote"]);
    // EQ-06 북마크 총수는 개인 행 정책과 무관한 전체 count
    check!(by(100)["bookmarkCount"] == json!(3), "bookmarkCount(100) {:?}", by(100)["bookmarkCount"]);
    check!(by(102)["bookmarkCount"] == json!(1), "bookmarkCount(102) {:?}", by(102)["bookmarkCount"]);
    // EQ-04 관계 선택과 출력 타입
    check!(by(100)["club"] == json!({ "id": 10, "name": "A동아리", "logo": "a.png" }), "club {:?}", by(100)["club"]);
    check!(p.output_type["rows"]["internalNote"]["redactable"] == true, "출력 타입 redactable");
    check!(p.output_type["rows"]["club"]["nullable"] == true, "출력 타입 club nullable");
    let p2 = plan_read(&f, &list_req(), &who(Some(2))).unwrap();
    let r2 = execute(&mut db, &p2).await.unwrap();
    check!(r2.iter().all(|r| r["internalNote"].is_null()), "비관리자에게 internalNote 노출");

    // 필터·정렬
    let mut fr = list_req();
    fr["filter"] = json!([{ "field": "periodEnd", "op": "gte", "value": "2026-10-09T00:00:00Z" }]);
    let rows = execute(&mut db, &plan_read(&f, &fr, &who(Some(1))).unwrap()).await.unwrap();
    check!(ids(&rows) == vec![100], "filter gte {:?}", ids(&rows));
    let mut sr = list_req();
    sr["sort"] = json!([{ "field": "views", "dir": "desc" }]);
    let rows = execute(&mut db, &plan_read(&f, &sr, &who(Some(1))).unwrap()).await.unwrap();
    check!(ids(&rows) == vec![102, 100], "sort views desc {:?}", ids(&rows));

    // EQ-04 대상 정책 재적용: Club 행 정책을 같은 학교만으로 좁힌 변형에서 연합 동아리는 null
    let strict = variant("rows read when school = null or school = actor.school", "rows read when school = actor.school");
    let rows = execute(&mut db, &plan_read(&strict, &list_req(), &who(Some(1))).unwrap()).await.unwrap();
    let u = rows.iter().find(|r| r["id"] == json!(102)).cloned().unwrap_or(Value::Null);
    check!(u["club"].is_null() && !u.is_null(), "엄격 Club 정책에서 연합 club {:?}", u);

    // V1-R1 행 정책 누락은 전부 거부: Club 행 정책을 지운 변형에서 관계는 모두 null
    let nopol = variant("  rows read when school = null or school = actor.school\n", "");
    let rows = execute(&mut db, &plan_read(&nopol, &list_req(), &who(Some(1))).unwrap()).await.unwrap();
    check!(!rows.is_empty() && rows.iter().all(|r| r["club"].is_null()), "행 정책 없는 Club이 노출됨 {rows:?}");

    // EQ-10 단독 집계: 관리자만, 없는 동아리와 남의 동아리는 같은 결과
    for (actor, club, want) in [
        (Some(1), "10", Ok(json!(2))),
        (Some(3), "11", Ok(json!(1))),
        (Some(2), "10", Err("ACCESS_DENIED")),
        (Some(1), "11", Err("ACCESS_DENIED")),
        (Some(1), "999", Err("ACCESS_DENIED")),
        (None, "10", Err("ACCESS_DENIED")),
    ] {
        let q = json!({ "aggregate": "Apply.approvedCount", "input": { "clubId": club } });
        let p = plan_read(&f, &q, &who(actor)).unwrap();
        // 거부 변환은 테스트가 아니라 실행기(execute)가 한다(R2B-01).
        let got = execute(&mut db, &p).await.map(|v| v[0].clone()).map_err(|e| e.code);
        check!(got == want, "approvedCount actor {actor:?} club {club}: 기대 {want:?}, 실제 {got:?}");
    }
    let bad = json!({ "aggregate": "Apply.approvedCount", "input": { "clubId": "10 OR 1=1" } });
    check!(code(plan_read(&f, &bad, &who(Some(1)))) == "BAD_VALUE", "집계 입력 주입이 거부되지 않음");
    let other = json!({ "aggregate": "Recruitment.bookmarkCount", "input": {} });
    check!(code(plan_read(&f, &other, &who(Some(1)))) == "NOT_EXPOSED", "노출 안 된 집계 직접 호출");

    // EQ-09 불변식 DB 집행(V1-R4)
    let dup = db
        .batch_execute("INSERT INTO aip_v2_spike.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES (106, 'A 두번째', '2026-10-30T00:00:00Z', 'PUBLISHED', 0, 10, NULL)")
        .await;
    check!(dup.as_ref().err().and_then(|e| e.code()).map(|c| c.code()) == Some("23505"), "같은 동아리 두 번째 게시가 막히지 않음: {dup:?}");
    let ok_draft = db
        .batch_execute("INSERT INTO aip_v2_spike.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES (107, 'A 두번째 초안', '2026-10-30T00:00:00Z', 'DRAFT', 0, 10, NULL)")
        .await;
    check!(ok_draft.is_ok(), "초안 추가가 불변식에 막힘: {ok_draft:?}");

    // 읽기 전용 트랜잭션과 기한
    db.batch_execute("INSERT INTO aip_v2_spike.recruitment_bookmark (recruitment_id, member_id) SELECT 105, 1 FROM generate_series(1, 400000)")
        .await
        .expect("부하 seed");
    let slow = variant("deadline 2s;", "deadline 1ms;");
    let r = execute(&mut db, &plan_read(&slow, &list_req(), &who(Some(1))).unwrap()).await;
    check!(matches!(&r, Err(e) if e.code == "DEADLINE_EXCEEDED"), "1ms 기한 변형: {:?}", r.as_ref().map(|v| v.len()));
    let r = execute(&mut db, &plan_read(&f, &list_req(), &who(Some(1))).unwrap()).await;
    check!(r.is_ok(), "기본 2s 기한에서 실패: {:?}", r.err());

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
    eprintln!("db scenarios ok");
}
