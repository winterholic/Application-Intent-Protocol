//! 읽기 패턴 연산자: Ref eq, in(값 배열), isNull. 허용 목록·값 바인딩·행 정책 유지를 확인한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::{plan_read, plan_read_with_wire, Caller, Plan, MAX_IN_VALUES};
use spike_v2_read::{connect, execute, sqlgen};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

/// Recruitment와 Club(루트 조회용 budget 포함)에 새 연산자를 허용한 정의.
fn facts() -> Value {
    // Ref 필터는 대상 행 정책을 따른다. School은 fixture에서 행 정책이 없어(denyAll) 열어 준다.
    let src = A
        .replacen("fields { id: Id; name: Text }\n}", "fields { id: Id; name: Text }\n  rows read when true\n}", 1)
        .replacen("filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, periodEnd.lte, club.eq, club.in, status.in, title.in", 1)
        .replacen(
            "expose read { select id, name, logo }",
            "expose read { select id, name, logo; filter school.eq, school.in, school.isNull, logo.isNull; budget { rows 50; depth 1; deadline 2s; cost 1000 } }",
            1,
        );
    load_str(&src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}

fn rec(op_field: &str, op: &str, value: Value) -> Value {
    let _ = op_field;
    json!({ "read": "Recruitment", "select": ["id"], "filter": [{ "field": op_field, "op": op, "value": value }] })
}

fn club(field: &str, op: &str, value: Value) -> Value {
    json!({ "read": "Club", "select": ["id"], "filter": [{ "field": field, "op": op, "value": value }] })
}

fn code(r: Result<Plan, spike_v2_read::plan::Reject>) -> &'static str {
    match r {
        Ok(_) => "OK",
        Err(e) => e.code,
    }
}

#[test]
fn operators_outside_the_definition_allow_list_are_rejected() {
    // 기본 fixture는 club.eq / status.in / school.isNull 어느 것도 열지 않았다.
    let base = load_str(A, Form::A).unwrap().execution;
    let c = who(Some(1));
    assert_eq!(code(plan_read(&base, &rec("club", "eq", json!(10)), &c)), "FILTER_NOT_ALLOWED");
    assert_eq!(code(plan_read(&base, &rec("status", "in", json!(["PUBLISHED"])), &c)), "FILTER_NOT_ALLOWED");
    // 새 facts에서도 목록에 없는 (필드, 연산) 조합은 거부: title.eq, status.eq, club.isNull, school.gte
    let f = facts();
    assert_eq!(code(plan_read(&f, &rec("title", "eq", json!("x")), &c)), "FILTER_NOT_ALLOWED");
    assert_eq!(code(plan_read(&f, &rec("status", "eq", json!("PUBLISHED")), &c)), "FILTER_NOT_ALLOWED");
    assert_eq!(code(plan_read(&f, &rec("club", "isNull", json!(true)), &c)), "FILTER_NOT_ALLOWED");
    assert_eq!(code(plan_read(&f, &club("name", "in", json!(["a"])), &c)), "FILTER_NOT_ALLOWED");
    // in 허용이 eq·isNull 허용을 뜻하지 않는다
    assert_eq!(code(plan_read(&f, &club("school", "gte", json!(1)), &c)), "FILTER_NOT_ALLOWED");
    assert_eq!(code(plan_read(&f, &club("logo", "in", json!(["a"])), &c)), "FILTER_NOT_ALLOWED");
}

#[test]
fn ref_eq_follows_the_id_wire_and_rejects_bad_values() {
    let f = facts();
    let c = who(Some(1));
    for (wire, ok, bad) in [
        (
            IdWire::Legacy,
            vec![json!(10), json!("10")],
            vec![json!("abc"), json!(-1), json!(null), json!(1.5), json!([10]), json!(true), json!("10 OR 1=1")],
        ),
        (IdWire::SafeNumber, vec![json!(10)], vec![json!("10"), json!(-1), json!(null)]),
        (IdWire::DecimalString, vec![json!("10"), json!("0")], vec![json!(10), json!("010"), json!(""), json!("-1"), json!("1e3"), json!(null)]),
    ] {
        for v in ok {
            let p = plan_read_with_wire(&f, &rec("club", "eq", v.clone()), &c, wire).unwrap_or_else(|e| panic!("{wire:?} {v}: {e:?}"));
            assert!(p.params.contains(&Some(v.as_str().map_or_else(|| v.to_string(), str::to_string))), "{:?}", p.params);
            assert!(p.sql.contains("::text::bigint"), "{}", p.sql);
        }
        for v in bad {
            assert_eq!(code(plan_read_with_wire(&f, &rec("club", "eq", v.clone()), &c, wire)), "BAD_VALUE", "{wire:?} {v}");
        }
    }
}

#[test]
fn in_filter_bounds_dedupes_and_binds_every_element() {
    let f = facts();
    let c = who(Some(1));
    assert_eq!(MAX_IN_VALUES, 50);
    let req = |v: Value| rec("status", "in", v);
    // 빈 배열, 배열 아님, 상한 초과, 잘못된 원소는 SQL 전에 거부
    assert_eq!(code(plan_read(&f, &req(json!([])), &c)), "BAD_VALUE");
    assert_eq!(code(plan_read(&f, &req(json!("PUBLISHED")), &c)), "BAD_VALUE");
    assert_eq!(code(plan_read(&f, &req(json!(null)), &c)), "BAD_VALUE");
    assert_eq!(code(plan_read(&f, &req(json!(["PUBLISHED", "NOPE"])), &c)), "BAD_VALUE");
    assert_eq!(code(plan_read(&f, &req(json!(["PUBLISHED", null])), &c)), "BAD_VALUE");
    assert_eq!(code(plan_read(&f, &req(json!([["PUBLISHED"]])), &c)), "BAD_VALUE");
    // 상한 초과는 중복 제거 전 길이로 센다. 요청 크기 자체를 제한하기 위해서다.
    let many: Vec<Value> = (0..=MAX_IN_VALUES).map(|_| json!("PUBLISHED")).collect();
    assert_eq!(code(plan_read(&f, &req(Value::Array(many)), &c)), "IN_LIST_EXCEEDED");
    let titles: Vec<Value> = (0..MAX_IN_VALUES).map(|i| json!(format!("t{i}"))).collect();
    let p = plan_read(&f, &rec("title", "in", Value::Array(titles)), &c).unwrap();
    assert_eq!(p.params.iter().filter(|x| x.as_ref().is_some_and(|s| s.starts_with('t'))).count(), 50);
    let titles: Vec<Value> = (0..=MAX_IN_VALUES).map(|i| json!(format!("t{i}"))).collect();
    assert_eq!(code(plan_read(&f, &rec("title", "in", Value::Array(titles)), &c)), "IN_LIST_EXCEEDED");
    // 중복은 값 하나로 합친다(첫 등장 순서). Id는 정규화한 값 기준이다.
    let p = plan_read(&f, &req(json!(["DRAFT", "CLOSED", "DRAFT"])), &c).unwrap();
    assert_eq!(p.params.iter().filter(|x| x.as_deref() == Some("DRAFT")).count(), 1);
    let p = plan_read(&f, &rec("club", "in", json!([10, "10", 11])), &c).unwrap();
    assert_eq!(p.params.iter().filter(|x| x.as_deref() == Some("10")).count(), 1);
    // 값은 SQL 문자열에 들어가지 않는다. Text 원소의 따옴표·세미콜론·콤마·중괄호도 그대로 한 원소다.
    let nasty = "a'); DROP TABLE x; --";
    let p = plan_read(&f, &rec("title", "in", json!([nasty, "b,c", "{d}"])), &c).unwrap();
    assert!(!p.sql.contains("DROP") && !p.sql.contains("b,c"), "{}", p.sql);
    for want in [nasty, "b,c", "{d}"] {
        assert!(p.params.contains(&Some(want.to_string())));
    }
    assert!(p.sql.contains("::text::text"));
    // NUL 문자는 기존 Text 검사에서 거부
    assert_eq!(code(plan_read(&f, &rec("title", "in", json!(["a\u{0}b"])), &c)), "BAD_VALUE");
}

#[test]
fn safe_number_wire_rejects_ids_beyond_js_safe_range() {
    let f = facts();
    let c = who(Some(1));
    let big = json!(9_007_199_254_740_992_i64);
    assert_eq!(code(plan_read_with_wire(&f, &rec("club", "eq", big.clone()), &c, IdWire::SafeNumber)), "ID_OUT_OF_RANGE");
    assert_eq!(code(plan_read_with_wire(&f, &rec("club", "in", json!([10, big])), &c, IdWire::SafeNumber)), "ID_OUT_OF_RANGE");
}

#[test]
fn in_filter_on_ref_follows_wire_and_unknown_op_shapes_stay_rejected() {
    let f = facts();
    let c = who(Some(1));
    assert_eq!(code(plan_read_with_wire(&f, &rec("club", "in", json!(["10", "11"])), &c, IdWire::DecimalString)), "OK");
    assert_eq!(code(plan_read_with_wire(&f, &rec("club", "in", json!([10, 11])), &c, IdWire::DecimalString)), "BAD_VALUE");
    assert_eq!(code(plan_read_with_wire(&f, &rec("club", "in", json!([10, 11])), &c, IdWire::SafeNumber)), "OK");
    assert_eq!(code(plan_read_with_wire(&f, &rec("club", "in", json!([10, "x"])), &c, IdWire::SafeNumber)), "BAD_VALUE");
    // 스칼라 연산에 배열을 넣을 수 없고, in에 스칼라를 넣을 수 없다
    assert_eq!(code(plan_read(&f, &rec("club", "eq", json!([10, 11])), &c)), "BAD_VALUE");
    assert_eq!(code(plan_read(&f, &rec("club", "in", json!(10)), &c)), "BAD_VALUE");
    // 같은 (필드, 연산) 중복 금지는 그대로
    let mut r = rec("club", "in", json!([10]));
    r["filter"] = json!([{"field":"club","op":"in","value":[10]},{"field":"club","op":"in","value":[11]}]);
    assert_eq!(code(plan_read(&f, &r, &c)), "DUPLICATE");
}

#[test]
fn is_null_takes_only_a_bool_and_binds_it() {
    let f = facts();
    let c = who(Some(1));
    for bad in [json!("true"), json!(1), json!(null), json!([true]), json!({"a":1})] {
        assert_eq!(code(plan_read(&f, &club("school", "isNull", bad.clone()), &c)), "BAD_VALUE", "{bad}");
    }
    for (v, s) in [(true, "true"), (false, "false")] {
        let p = plan_read(&f, &club("school", "isNull", json!(v)), &c).unwrap();
        assert!(p.params.contains(&Some(s.into())), "{:?}", p.params);
        assert!(p.sql.contains("IS NULL") && p.sql.contains("::text::boolean"), "{}", p.sql);
        assert!(!p.sql.contains(s), "bool 값이 SQL 문자열에 들어감: {}", p.sql);
    }
}

const SEED: &str = "
INSERT INTO SCHEMA.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO SCHEMA.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (4, NULL);
INSERT INTO SCHEMA.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', NULL, 2), (12, '연합', 'u.png', NULL), (13, 'A2동아리', NULL, 1);
INSERT INTO SCHEMA.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (13, 2, 'MEMBER'), (11, 3, 'ADMIN');
INSERT INTO SCHEMA.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'A-note'),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, 'B-note'),
 (102, '연합 모집', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, 'U-note'),
 (103, 'A 초안', '2026-10-20T00:00:00Z', 'DRAFT', 0, 10, NULL),
 (104, 'A2 마감', '2026-10-20T00:00:00Z', 'CLOSED', 0, 13, NULL),
 (105, 'A2 만료', '2026-10-01T00:00:00Z', 'PUBLISHED', 0, 13, NULL);
";

fn ids(rows: &[Value]) -> Vec<i64> {
    rows.iter().map(|r| r["id"].as_i64().unwrap()).collect()
}

async fn run(db: &mut tokio_postgres::Client, f: &Value, actor: Option<i64>, req: Value) -> Vec<i64> {
    let p = plan_read(f, &req, &who(actor)).unwrap_or_else(|e| panic!("{req}: {e:?}"));
    ids(&execute(db, &p).await.unwrap_or_else(|e| panic!("{req}: {e:?}")))
}

#[tokio::test(flavor = "current_thread")]
async fn db_filters_return_expected_rows_and_never_bypass_row_policy() {
    sqlgen::set_schema("aip_read_patterns");
    let f = facts();
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

    // Ref eq: 같은 학교 사용자 1. 행 정책(PUBLISHED·기한·학교)이 먼저 적용된다.
    check!("club.eq 10", run(&mut db, &f, Some(1), rec("club", "eq", json!(10))).await, vec![100]);
    check!("club.eq 10 (digit string)", run(&mut db, &f, Some(1), rec("club", "eq", json!("10"))).await, vec![100]);
    check!("club.eq 11 다른 학교 동아리는 정책으로 안 보임", run(&mut db, &f, Some(1), rec("club", "eq", json!(11))).await, Vec::<i64>::new());
    check!("club.eq 11 학교 2 사용자", run(&mut db, &f, Some(3), rec("club", "eq", json!(11))).await, vec![101]);
    check!("club.eq 13 만료·마감은 안 보임", run(&mut db, &f, Some(1), rec("club", "eq", json!(13))).await, Vec::<i64>::new());
    check!("club.eq 999 없는 동아리", run(&mut db, &f, Some(1), rec("club", "eq", json!(999))).await, Vec::<i64>::new());
    check!("club.eq 익명 연합", run(&mut db, &f, None, rec("club", "eq", json!(12))).await, vec![102]);
    check!("club.eq 익명 학교 동아리", run(&mut db, &f, None, rec("club", "eq", json!(10))).await, Vec::<i64>::new());

    // in: 정책상 안 보이는 행(초안·마감·만료·다른 학교)은 값 목록에 넣어도 나오지 않는다
    check!("status.in 전부", run(&mut db, &f, Some(1), rec("status", "in", json!(["DRAFT", "CLOSED", "PUBLISHED"]))).await, vec![100, 102]);
    check!("status.in DRAFT", run(&mut db, &f, Some(1), rec("status", "in", json!(["DRAFT"]))).await, Vec::<i64>::new());
    check!("club.in 10,11,12", run(&mut db, &f, Some(1), rec("club", "in", json!([10, 11, 12]))).await, vec![100, 102]);
    check!("club.in 학교2 사용자", run(&mut db, &f, Some(3), rec("club", "in", json!([10, 11, 12]))).await, vec![101, 102]);
    check!("club.in 중복", run(&mut db, &f, Some(1), rec("club", "in", json!([10, "10", 10]))).await, vec![100]);
    check!("title.in", run(&mut db, &f, Some(1), rec("title", "in", json!(["A 모집", "A 초안", "B 모집"]))).await, vec![100]);
    check!("title.in 따옴표", run(&mut db, &f, Some(1), rec("title", "in", json!(["x'; --", "A 모집"]))).await, vec![100]);
    // 여러 필터는 AND
    let mut both = rec("club", "in", json!([10, 12]));
    both["filter"].as_array_mut().unwrap().push(json!({"field":"status","op":"in","value":["PUBLISHED"]}));
    check!("club.in AND status.in", run(&mut db, &f, Some(1), both).await, vec![100, 102]);

    // Club 루트 조회: 학교 1 사용자는 10,12,13 만 본다(11은 행 정책)
    check!("Club 전체", run(&mut db, &f, Some(1), json!({"read":"Club","select":["id"]})).await, vec![10, 12, 13]);
    check!("school.isNull true", run(&mut db, &f, Some(1), club("school", "isNull", json!(true))).await, vec![12]);
    check!("school.isNull false", run(&mut db, &f, Some(1), club("school", "isNull", json!(false))).await, vec![10, 13]);
    check!("school.isNull false 학교2 사용자", run(&mut db, &f, Some(3), club("school", "isNull", json!(false))).await, vec![11]);
    check!("logo.isNull true", run(&mut db, &f, Some(1), club("logo", "isNull", json!(true))).await, vec![13]);
    check!("logo.isNull false", run(&mut db, &f, Some(1), club("logo", "isNull", json!(false))).await, vec![10, 12]);
    // 정책 우회 불가: 다른 학교(2) 동아리를 school.eq/in 으로 불러도 학교 1 사용자에게는 비어 있다
    check!("school.eq 2", run(&mut db, &f, Some(1), club("school", "eq", json!(2))).await, Vec::<i64>::new());
    check!("school.in 1,2", run(&mut db, &f, Some(1), club("school", "in", json!([1, 2]))).await, vec![10, 13]);
    check!("school.isNull false 익명", run(&mut db, &f, None, club("school", "isNull", json!(false))).await, Vec::<i64>::new());
    check!("school.isNull true 익명", run(&mut db, &f, None, club("school", "isNull", json!(true))).await, vec![12]);
    check!("school.in 익명", run(&mut db, &f, None, club("school", "in", json!([1, 2]))).await, Vec::<i64>::new());

    // 정책 변형: Club 행 정책을 같은 학교만으로 좁히면 Club 루트 조회에서도 연합이 사라진다(필터가 정책을 대체하지 않음)
    let strict_src = A
        .replacen("fields { id: Id; name: Text }\n}", "fields { id: Id; name: Text }\n  rows read when true\n}", 1)
        .replacen("rows read when school = null or school = actor.school", "rows read when school = actor.school", 1)
        .replacen(
            "expose read { select id, name, logo }",
            "expose read { select id, name, logo; filter school.isNull; budget { rows 50; depth 1; deadline 2s; cost 1000 } }",
            1,
        );
    let strict = load_str(&strict_src, Form::A).unwrap().execution;
    check!("strict school.isNull true", run(&mut db, &strict, Some(1), club("school", "isNull", json!(true))).await, Vec::<i64>::new());

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
