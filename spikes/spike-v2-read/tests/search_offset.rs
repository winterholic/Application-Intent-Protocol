//! 부분일치 `contains`와 opt-in `offset`. 허용 목록·값 바인딩·행 정책 유지·상한을 확인한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller, Plan, Reject, MAX_CONTAINS_CHARS};
use spike_v2_read::{connect, execute, sqlgen};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
const FILTER: &str = "filter periodEnd.gte, periodEnd.lte";
const BUDGET: &str = "budget { rows 50; depth 2; deadline 2s; cost 1000 }";

fn load(filter: &str, budget: &str) -> Value {
    load_str(&A.replacen(FILTER, filter, 1).replacen(BUDGET, budget, 1), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

/// contains와 offset(최대 100)을 모두 연 정의.
fn open() -> Value {
    load("filter periodEnd.gte, periodEnd.lte, title.contains", "budget { rows 50; depth 2; deadline 2s; cost 1000; offset 100 }")
}

fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}

fn contains(v: Value) -> Value {
    json!({ "read": "Recruitment", "select": ["id"], "filter": [{ "field": "title", "op": "contains", "value": v }] })
}

fn paged(offset: Value) -> Value {
    json!({ "read": "Recruitment", "select": ["id"], "offset": offset })
}

fn code(r: Result<Plan, Reject>) -> &'static str {
    match r {
        Ok(_) => "OK",
        Err(e) => e.code,
    }
}

#[test]
fn contains_requires_declaration() {
    let c = who(Some(1));
    // 선언 없는 정의(기본 fixture)에서는 거부, 선언이 있어도 다른 필드·연산 조합은 거부
    let base = load_str(A, Form::A).unwrap().execution;
    assert_eq!(code(plan_read(&base, &contains(json!("a")), &c)), "FILTER_NOT_ALLOWED");
    let f = open();
    assert_eq!(code(plan_read(&f, &contains(json!("a")), &c)), "OK");
    let other = json!({ "read": "Recruitment", "select": ["id"], "filter": [{ "field": "title", "op": "icontains", "value": "a" }] });
    assert_eq!(code(plan_read(&f, &other, &c)), "FILTER_NOT_ALLOWED");
    let prefix = json!({ "read": "Recruitment", "select": ["id"], "filter": [{ "field": "title", "op": "prefix", "value": "a" }] });
    assert_eq!(code(plan_read(&f, &prefix, &c)), "FILTER_NOT_ALLOWED");
}

#[test]
fn contains_value_rules() {
    let f = open();
    let c = who(Some(1));
    assert_eq!(MAX_CONTAINS_CHARS, 200);
    // 빈 문자열은 모든 행과 같아 필터가 아니다. 거부한다.
    assert_eq!(code(plan_read(&f, &contains(json!("")), &c)), "BAD_VALUE");
    for bad in [json!(null), json!(1), json!(true), json!(["a"]), json!({"a":1}), json!("a\u{0}b")] {
        assert_eq!(code(plan_read(&f, &contains(bad.clone()), &c)), "BAD_VALUE", "{bad}");
    }
    // 길이는 문자 수로 센다(바이트 아님).
    assert_eq!(code(plan_read(&f, &contains(json!("가".repeat(200))), &c)), "OK");
    assert_eq!(code(plan_read(&f, &contains(json!("가".repeat(201))), &c)), "VALUE_TOO_LONG");
    assert_eq!(code(plan_read(&f, &contains(json!("a".repeat(201))), &c)), "VALUE_TOO_LONG");
    // 공백만 있는 값은 리터럴이다(trim하지 않는다)
    assert_eq!(code(plan_read(&f, &contains(json!(" ")), &c)), "OK");
}

#[test]
fn contains_binds_literal_and_never_uses_like() {
    let f = open();
    let c = who(Some(1));
    for v in ["100%", "a_b", "\\", "x'); DROP TABLE y; --", "%", "_"] {
        let p = plan_read(&f, &contains(json!(v)), &c).unwrap();
        assert!(p.params.contains(&Some(v.to_string())), "{:?}", p.params);
        assert!(p.sql.contains("strpos("), "{}", p.sql);
        assert!(!p.sql.to_uppercase().contains("LIKE"), "{}", p.sql);
        assert!(!p.sql.contains(v) || v.len() <= 1, "값이 SQL에 들어감: {}", p.sql);
        assert!(!p.sql.contains("DROP"), "{}", p.sql);
    }
}

#[test]
fn contains_on_policy_field_is_rejected_even_if_facts_skip_sema() {
    let mut f = open();
    f["resources"]["Recruitment"]["exposeRead"]["filter"].as_array_mut().unwrap().push(json!("internalNote.contains"));
    let req = json!({ "read": "Recruitment", "select": ["id"], "filter": [{ "field": "internalNote", "op": "contains", "value": "x" }] });
    assert_eq!(code(plan_read(&f, &req, &who(Some(1)))), "POLICY_FIELD_NOT_FILTERABLE");
}

#[test]
fn offset_rejected_without_budget_opt_in() {
    let base = load_str(A, Form::A).unwrap().execution;
    for v in [json!(0), json!(10), json!("a")] {
        assert_eq!(code(plan_read(&base, &paged(v.clone()), &who(Some(1)))), "UNKNOWN_KEY", "{v}");
    }
}

#[test]
fn offset_value_rules_and_binding() {
    let f = open();
    let c = who(Some(1));
    for ok in [json!(0), json!(1), json!(100)] {
        assert_eq!(code(plan_read(&f, &paged(ok.clone()), &c)), "OK", "{ok}");
    }
    for bad in [json!(-1), json!(1.5), json!("5"), json!(null), json!(true), json!([1]), json!(1e3)] {
        assert_eq!(code(plan_read(&f, &paged(bad.clone()), &c)), "BAD_VALUE", "{bad}");
    }
    for over in [json!(101), json!(i64::MAX)] {
        assert_eq!(code(plan_read(&f, &paged(over.clone()), &c)), "OFFSET_EXCEEDED", "{over}");
    }
    // 값은 매개변수다. SQL에 숫자가 새지 않는다. offset을 안 보내면 OFFSET 절도 없다.
    let p = plan_read(&f, &paged(json!(77)), &c).unwrap();
    assert!(p.params.contains(&Some("77".into())), "{:?}", p.params);
    assert!(p.sql.contains("OFFSET $") && !p.sql.contains("77"), "{}", p.sql);
    let none = plan_read(&f, &json!({ "read": "Recruitment", "select": ["id"] }), &c).unwrap();
    assert!(!none.sql.contains("OFFSET"), "{}", none.sql);
}

#[test]
fn offset_order_is_deterministic_and_cost_counts_skipped_rows() {
    let f = open();
    let c = who(Some(1));
    // 사용자 정렬에 id 타이브레이커가 항상 뒤에 붙는다.
    let req = json!({ "read": "Recruitment", "select": ["id"], "sort": [{ "field": "views", "dir": "desc" }], "offset": 5 });
    let p = plan_read(&f, &req, &c).unwrap();
    assert!(p.sql.contains("ORDER BY t.views DESC, t.id ASC LIMIT"), "{}", p.sql);
    // 건너뛰는 행 수도 비용이다: 같은 limit에서 offset이 크면 cost가 크다.
    let a = plan_read(&f, &paged(json!(0)), &c).unwrap().cost;
    let b = plan_read(&f, &paged(json!(100)), &c).unwrap().cost;
    assert_eq!(b - a, 100);
    // offset 상한이 budget cost보다 크게 선언돼 있으면 깊은 offset은 COST_EXCEEDED로 막힌다.
    let deep = load("filter periodEnd.gte", "budget { rows 50; depth 2; deadline 2s; cost 1000; offset 5000 }");
    assert_eq!(code(plan_read(&deep, &paged(json!(900)), &c)), "OK");
    assert_eq!(code(plan_read(&deep, &paged(json!(960)), &c)), "COST_EXCEEDED");
    assert_eq!(code(plan_read(&deep, &paged(json!(5001)), &c)), "OFFSET_EXCEEDED");
}

const SEED: &str = "
INSERT INTO SCHEMA.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO SCHEMA.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2);
INSERT INTO SCHEMA.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', NULL, 2), (12, '연합', 'u.png', NULL), (13, 'C동아리', NULL, 1), (14, 'D동아리', NULL, 1), (15, 'E동아리', NULL, 1);
INSERT INTO SCHEMA.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');
INSERT INTO SCHEMA.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'secret'),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, NULL),
 (102, '연합 모집', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, NULL),
 (103, 'A 초안 모집', '2026-10-20T00:00:00Z', 'DRAFT', 0, 10, NULL),
 (105, 'A 만료 모집', '2026-10-01T00:00:00Z', 'PUBLISHED', 0, 15, NULL),
 (106, 'sale 50% off', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 13, NULL),
 (107, 'sale 50x off a_b', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 14, NULL);
";

async fn run(db: &mut tokio_postgres::Client, f: &Value, actor: Option<i64>, req: Value) -> Vec<i64> {
    let p = plan_read(f, &req, &who(actor)).unwrap_or_else(|e| panic!("{req}: {e:?}"));
    execute(db, &p).await.unwrap_or_else(|e| panic!("{req}: {e:?}")).iter().map(|r| r["id"].as_i64().unwrap()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn db_contains_and_offset_never_bypass_row_policy() {
    sqlgen::set_schema("aip_search_offset");
    let f = open();
    let mut db = connect().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL 실패 {stmt}: {e}"));
    }
    db.batch_execute(&SEED.replace("SCHEMA", sqlgen::schema())).await.expect("seed");
    let mut fails: Vec<String> = vec![];
    macro_rules! check {
        ($name:expr, $got:expr, $want:expr) => {
            let got = $got;
            if got != $want {
                fails.push(format!("{}: 기대 {:?}, 실제 {:?}", $name, $want, got));
            }
        };
    }
    // 행 정책: 사용자 1(학교 1)은 100,102,106,107만 본다. 초안(103)·만료(105)·다른 학교(101)는 값이 맞아도 안 나온다.
    check!("전체", run(&mut db, &f, Some(1), json!({"read":"Recruitment","select":["id"]})).await, vec![100, 102, 106, 107]);
    check!("contains 모집", run(&mut db, &f, Some(1), contains(json!("모집"))).await, vec![100, 102]);
    check!("contains 초안(정책으로 숨김)", run(&mut db, &f, Some(1), contains(json!("초안"))).await, Vec::<i64>::new());
    check!("contains 만료(정책으로 숨김)", run(&mut db, &f, Some(1), contains(json!("만료"))).await, Vec::<i64>::new());
    check!("contains B(다른 학교)", run(&mut db, &f, Some(1), contains(json!("B 모집"))).await, Vec::<i64>::new());
    check!("contains B 학교2 사용자", run(&mut db, &f, Some(3), contains(json!("B 모집"))).await, vec![101]);
    check!("contains 대소문자 구분", run(&mut db, &f, Some(1), contains(json!("SALE"))).await, Vec::<i64>::new());
    // 와일드카드는 리터럴: `%`는 `50%`에만, `_`는 `a_b`에만 일치한다(LIKE였다면 전부/한 글자 일치).
    check!("contains %", run(&mut db, &f, Some(1), contains(json!("%"))).await, vec![106]);
    check!("contains 50%", run(&mut db, &f, Some(1), contains(json!("50%"))).await, vec![106]);
    check!("contains _", run(&mut db, &f, Some(1), contains(json!("_"))).await, vec![107]);
    check!("contains 50_", run(&mut db, &f, Some(1), contains(json!("50_"))).await, Vec::<i64>::new());
    check!("contains 중간", run(&mut db, &f, Some(1), contains(json!("ale 5"))).await, vec![106, 107]);
    check!("contains 전체 일치", run(&mut db, &f, Some(1), contains(json!("A 모집"))).await, vec![100]);
    check!("contains 따옴표", run(&mut db, &f, Some(1), contains(json!("x'; --"))).await, Vec::<i64>::new());

    // offset: 정렬 periodEnd asc → 102(10-08), 그다음 10-10 동률 100,106,107은 id로 고정
    let page = |limit: i64, off: i64| json!({"read":"Recruitment","select":["id"],"sort":[{"field":"periodEnd"}],"limit":limit,"offset":off});
    check!("offset 0", run(&mut db, &f, Some(1), page(2, 0)).await, vec![102, 100]);
    check!("offset 2", run(&mut db, &f, Some(1), page(2, 2)).await, vec![106, 107]);
    check!("offset 4(끝 지남)", run(&mut db, &f, Some(1), page(2, 4)).await, Vec::<i64>::new());
    check!("offset 3 limit 5", run(&mut db, &f, Some(1), page(5, 3)).await, vec![107]);
    let mut one_by_one = vec![];
    for off in 0..5 {
        one_by_one.extend(run(&mut db, &f, Some(1), page(1, off)).await);
    }
    check!("limit 1 페이지 합 = 전체(중복·누락 없음)", one_by_one, vec![102, 100, 106, 107]);
    // 정책 우회 불가: 학교 2 사용자의 offset 0은 자기 행(101,102)만, offset이 정책으로 숨겨진 행을 건너뛰어 노출하지 않는다.
    check!("학교2 offset 0", run(&mut db, &f, Some(3), page(5, 0)).await, vec![102, 101]);
    check!("학교2 offset 1", run(&mut db, &f, Some(3), page(5, 1)).await, vec![101]);
    // contains + offset 결합
    let both = json!({"read":"Recruitment","select":["id"],"filter":[{"field":"title","op":"contains","value":"sale"}],"limit":1,"offset":1});
    check!("contains+offset", run(&mut db, &f, Some(1), both).await, vec![107]);

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
