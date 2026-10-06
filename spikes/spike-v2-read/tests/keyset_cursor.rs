//! opt-in keyset cursor(`after`). 동률이 있어도 누락·중복이 없고, 행 정책을 우회하지 않으며, 값은 전부 바인딩된다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller, Plan, Reject};
use spike_v2_read::{connect, execute, sqlgen};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
const BUDGET: &str = "budget { rows 50; depth 2; deadline 2s; cost 1000 }";

fn open() -> Value {
    load_str(&A.replacen(BUDGET, "budget { rows 50; depth 2; deadline 2s; cost 1000; cursor }", 1), Form::A)
        .unwrap_or_else(|e| panic!("{e:?}"))
        .execution
}

fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}

fn code(r: Result<Plan, Reject>) -> &'static str {
    match r {
        Ok(_) => "OK",
        Err(e) => e.code,
    }
}

fn req(sort: Value, after: Value) -> Value {
    json!({ "read": "Recruitment", "select": ["id", "views", "periodEnd"], "sort": sort, "after": after, "limit": 2 })
}

#[test]
fn after_requires_budget_opt_in() {
    let base = load_str(A, Form::A).unwrap().execution;
    let r = req(json!([{ "field": "views" }]), json!({ "views": 1, "id": 1 }));
    assert_eq!(code(plan_read(&base, &r, &who(Some(1)))), "UNKNOWN_KEY");
    assert_eq!(code(plan_read(&open(), &r, &who(Some(1)))), "OK");
}

#[test]
fn after_keys_must_match_sort_fields_plus_id() {
    let f = open();
    let c = who(Some(1));
    let sort = json!([{ "field": "views", "dir": "desc" }]);
    for bad in [
        json!({ "id": 1 }),                                                  // sort 필드 누락
        json!({ "views": 1 }),                                               // id 누락
        json!({ "views": 1, "id": 1, "periodEnd": "2026-10-10T00:00:00Z" }), // sort에 없는 필드
        json!({ "views": 1, "id": 1, "title": "x" }),
        json!({ "views": 1, "id": 1, "internalNote": "x" }), // field read 정책 필드
        json!({}),
        json!(null),
        json!([1, 2]),
        json!("abc"),
    ] {
        assert_eq!(code(plan_read(&f, &req(sort.clone(), bad.clone()), &c)), "BAD_CURSOR", "{bad}");
    }
    // sort가 없으면 id만 받는다.
    let no_sort = json!({ "read": "Recruitment", "select": ["id"], "after": { "id": 5 } });
    assert_eq!(code(plan_read(&f, &no_sort, &c)), "OK");
    // sort에 id가 있으면 id 키는 한 번만, 방향은 sort를 따른다.
    let id_sort = json!({ "read": "Recruitment", "select": ["id"], "sort": [{ "field": "id", "dir": "desc" }], "after": { "id": 5 } });
    let p = plan_read(&f, &id_sort, &c).unwrap();
    assert!(p.sql.contains("t.id < $"), "{}", p.sql);
}

#[test]
fn after_values_are_type_checked_and_bound() {
    let f = open();
    let c = who(Some(1));
    let sort = json!([{ "field": "views" }, { "field": "periodEnd", "dir": "desc" }]);
    for bad in [
        json!({ "views": "a", "periodEnd": "2026-10-10T00:00:00Z", "id": 1 }),
        json!({ "views": null, "periodEnd": "2026-10-10T00:00:00Z", "id": 1 }),
        json!({ "views": 1.5, "periodEnd": "2026-10-10T00:00:00Z", "id": 1 }),
        json!({ "views": 1, "periodEnd": "yesterday", "id": 1 }),
        json!({ "views": 1, "periodEnd": "2026-10-10T00:00:00Z", "id": -1 }),
        json!({ "views": 1, "periodEnd": "2026-10-10T00:00:00Z", "id": "x'; DROP TABLE y; --" }),
    ] {
        assert_eq!(code(plan_read(&f, &req(sort.clone(), bad.clone()), &c)), "BAD_VALUE", "{bad}");
    }
    let p = plan_read(&f, &req(sort, json!({ "views": 777, "periodEnd": "2026-10-10T00:00:00Z", "id": 4242 })), &c).unwrap();
    // Time은 정규화되어 바인딩된다(+00:00).
    for v in ["777", "2026-10-10T00:00:00+00:00", "4242"] {
        assert!(p.params.contains(&Some(v.to_string())), "{v}: {:?}", p.params);
        assert!(!p.sql.contains(v), "값이 SQL에 들어감 {v}: {}", p.sql);
    }
}

#[test]
fn after_cannot_combine_with_offset_and_is_cheap() {
    let f = load_str(&A.replacen(BUDGET, "budget { rows 50; depth 2; deadline 2s; cost 1000; offset 100; cursor }", 1), Form::A).unwrap().execution;
    let c = who(Some(1));
    let mut r = req(json!([{ "field": "views" }]), json!({ "views": 1, "id": 1 }));
    r["offset"] = json!(1);
    assert_eq!(code(plan_read(&f, &r, &c)), "CURSOR_WITH_OFFSET");
    // 비용은 건너뛰는 행을 세지 않는다: cursor 읽기는 같은 limit의 offset 0 읽기와 같다.
    let a = plan_read(&f, &req(json!([{ "field": "views" }]), json!({ "views": 1, "id": 1 })), &c).unwrap().cost;
    let b = plan_read(&f, &json!({ "read": "Recruitment", "select": ["id", "views", "periodEnd"], "sort": [{ "field": "views" }], "limit": 2 }), &c)
        .unwrap()
        .cost;
    assert_eq!(a, b);
}

#[test]
fn defensive_checks_when_facts_skip_sema() {
    let c = who(Some(1));
    let ok = json!({ "views": 1, "id": 1 });
    let req = |sort: Value, after: Value| json!({ "read": "Recruitment", "select": ["id"], "sort": sort, "after": after });
    // select에 없는 sort 필드(정의 검사를 건너뛴 facts)
    let mut f = open();
    f["resources"]["Recruitment"]["exposeRead"]["select"].as_object_mut().unwrap().remove("views");
    assert_eq!(code(plan_read(&f, &req(json!([{ "field": "views" }]), ok.clone()), &c)), "CURSOR_FIELD_NOT_ALLOWED");
    // id가 select에 없음
    let mut f = open();
    f["resources"]["Recruitment"]["exposeRead"]["select"].as_object_mut().unwrap().remove("id");
    let mut r = req(json!([{ "field": "views" }]), ok.clone());
    r["select"] = json!(["title"]);
    assert_eq!(code(plan_read(&f, &r, &c)), "CURSOR_FIELD_NOT_ALLOWED");
    // field read 정책이 있는 필드를 sort에 억지로 올림
    let mut f = open();
    f["resources"]["Recruitment"]["exposeRead"]["sort"].as_array_mut().unwrap().push(json!("internalNote"));
    let r = req(json!([{ "field": "internalNote" }]), json!({ "internalNote": "x", "id": 1 }));
    assert_eq!(code(plan_read(&f, &r, &c)), "POLICY_FIELD_NOT_FILTERABLE");
    // nullable 필드를 sort에 억지로 올리고 select kind를 field로 속임
    let mut f = open();
    f["resources"]["Recruitment"]["fieldRead"].as_object_mut().unwrap().remove("internalNote");
    f["resources"]["Recruitment"]["exposeRead"]["sort"].as_array_mut().unwrap().push(json!("internalNote"));
    f["resources"]["Recruitment"]["exposeRead"]["select"]["internalNote"] = json!("field");
    assert_eq!(
        code(plan_read(&f, &req(json!([{ "field": "internalNote" }]), json!({ "internalNote": "x", "id": 1 })), &c)),
        "CURSOR_FIELD_NOT_ALLOWED"
    );
}

#[test]
fn sql_shape_expands_mixed_directions() {
    let f = open();
    let c = who(Some(1));
    let sort = json!([{ "field": "views", "dir": "desc" }, { "field": "periodEnd", "dir": "asc" }]);
    let p = plan_read(&f, &req(sort, json!({ "views": 5, "periodEnd": "2026-10-10T00:00:00Z", "id": 100 })), &c).unwrap();
    assert!(p.sql.contains("t.views < $"), "{}", p.sql);
    assert!(p.sql.contains("t.views = $") && p.sql.contains("t.period_end > $"), "{}", p.sql);
    assert!(p.sql.contains("t.id > $"), "{}", p.sql);
    assert!(p.sql.contains("ORDER BY t.views DESC, t.period_end ASC, t.id ASC"), "{}", p.sql);
}

const SEED: &str = "
INSERT INTO SCHEMA.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO SCHEMA.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2);
INSERT INTO SCHEMA.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', NULL, 2), (12, '연합', 'u.png', NULL),
 (104, 'c4', NULL, 1), (105, 'c5', NULL, 1), (106, 'c6', NULL, 1), (107, 'c7', NULL, 1), (108, 'c8', NULL, 1), (109, 'c9', NULL, NULL), (110, 'c10', NULL, 1), (111, 'c11', NULL, 1), (112, 'c12', NULL, 1), (113, 'c13', NULL, 1);
INSERT INTO SCHEMA.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');
INSERT INTO SCHEMA.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'a', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'secret'),
 (101, 'b', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, NULL),
 (102, 'c', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, NULL),
 (103, 'd', '2026-10-20T00:00:00Z', 'DRAFT', 0, 10, NULL),
 (104, 'e', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 104, NULL),
 (105, 'f', '2026-10-01T00:00:00Z', 'PUBLISHED', 0, 105, NULL),
 (106, 'g', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 106, NULL),
 (107, 'h', '2026-10-10T00:00:00Z', 'PUBLISHED', 9, 107, NULL),
 (108, 'i', '2026-10-11T00:00:00Z', 'PUBLISHED', 5, 108, NULL),
 (109, 'j', '2026-10-11T00:00:00Z', 'PUBLISHED', 5, 109, NULL),
 (110, 'k', '2026-10-09T00:00:00Z', 'PUBLISHED', 2, 110, NULL),
 (111, 'l', '2026-10-09T00:00:00Z', 'PUBLISHED', 2, 111, NULL),
 (112, 'm', '2026-10-09T00:00:00Z', 'PUBLISHED', 2, 112, NULL),
 (113, 'n', '2026-10-10T00:00:00Z', 'PUBLISHED', 1, 113, NULL);
";

async fn rows(db: &mut tokio_postgres::Client, f: &Value, actor: Option<i64>, r: Value) -> Vec<Value> {
    let p = plan_read(f, &r, &who(actor)).unwrap_or_else(|e| panic!("{r}: {e:?}"));
    execute(db, &p).await.unwrap_or_else(|e| panic!("{r}: {e:?}"))
}

fn ids(v: &[Value]) -> Vec<i64> {
    v.iter().map(|r| r["id"].as_i64().unwrap()).collect()
}

/// 마지막 행에서 sort 필드와 id를 꺼내 다음 after를 만든다. 호출자가 할 수 있는 일과 같다.
fn next_after(last: &Value, sort: &[(&str, &str)]) -> Value {
    let mut m = serde_json::Map::new();
    for (f, _) in sort {
        m.insert((*f).into(), last[*f].clone());
    }
    m.insert("id".into(), last["id"].clone());
    Value::Object(m)
}

async fn walk(db: &mut tokio_postgres::Client, f: &Value, actor: Option<i64>, sort: &[(&str, &str)], limit: i64) -> Vec<i64> {
    let sort_json: Vec<Value> = sort.iter().map(|(f, d)| json!({ "field": f, "dir": d })).collect();
    let mut out = vec![];
    let mut after: Option<Value> = None;
    for _ in 0..50 {
        let mut r = json!({ "read": "Recruitment", "select": ["id", "views", "periodEnd"], "sort": sort_json, "limit": limit });
        if let Some(a) = &after {
            r["after"] = a.clone();
        }
        let page = rows(db, f, actor, r).await;
        if page.is_empty() {
            return out;
        }
        assert!(page.len() as i64 <= limit);
        after = Some(next_after(page.last().unwrap(), sort));
        out.extend(ids(&page));
    }
    panic!("페이지 순회가 끝나지 않음");
}

#[tokio::test(flavor = "current_thread")]
async fn db_keyset_walk_equals_full_list_with_ties_and_never_leaks_hidden_rows() {
    sqlgen::set_schema("aip_keyset_cursor");
    let f = open();
    let mut db = connect().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL 실패 {stmt}: {e}"));
    }
    db.batch_execute(&SEED.replace("SCHEMA", sqlgen::schema())).await.expect("seed");
    let mut fails: Vec<String> = vec![];

    // 사용자 1(학교 1)이 보는 행: 초안(103), 다른 학교 전용 동아리(101)는 정책으로 숨겨진다.
    let combos: Vec<Vec<(&str, &str)>> = vec![
        vec![],
        vec![("id", "desc")],
        vec![("views", "asc")],
        vec![("views", "desc")],
        vec![("periodEnd", "desc")],
        vec![("views", "desc"), ("periodEnd", "asc")],
        vec![("views", "asc"), ("periodEnd", "desc")],
        vec![("periodEnd", "asc"), ("views", "asc")],
        vec![("periodEnd", "desc"), ("views", "desc"), ("id", "asc")],
    ];
    for actor in [Some(1), Some(3), None] {
        for sort in &combos {
            let sort_json: Vec<Value> = sort.iter().map(|(f, d)| json!({ "field": f, "dir": d })).collect();
            let full = ids(&rows(&mut db, &f, actor, json!({ "read": "Recruitment", "select": ["id"], "sort": sort_json })).await);
            let mut dedup = full.clone();
            dedup.sort();
            dedup.dedup();
            if dedup.len() != full.len() {
                fails.push(format!("전체 목록 자체에 중복 {actor:?} {sort:?}"));
            }
            for limit in 1..=3 {
                let got = walk(&mut db, &f, actor, sort, limit).await;
                if got != full {
                    fails.push(format!("순회 불일치 actor={actor:?} sort={sort:?} limit={limit}: 기대 {full:?}, 실제 {got:?}"));
                }
            }
        }
    }
    // 동률이 실제로 있는지 확인한다(검증이 공허하지 않게). views=5 행이 여러 개, periodEnd 동률도 있다.
    let tied = ids(&rows(&mut db, &f, Some(1), json!({ "read": "Recruitment", "select": ["id"], "sort": [{ "field": "views" }] })).await);
    if tied.iter().filter(|i| [100, 104, 106, 108].contains(i)).count() != 4 {
        fails.push(format!("동률 시드 확인 실패 {tied:?}"));
    }

    // 정책 우회 불가: 숨은 행(초안 103, 학교 2 전용 101)의 값으로 경계를 잡아도 결과에는 보이는 행만 나온다.
    let hidden = |after: Value| json!({ "read": "Recruitment", "select": ["id"], "sort": [{ "field": "views" }], "after": after });
    let got = ids(&rows(&mut db, &f, Some(1), hidden(json!({ "views": 0, "id": 103 }))).await);
    if got.contains(&103) || got.contains(&101) {
        fails.push(format!("숨은 행 노출 {got:?}"));
    }
    // 경계가 존재하지 않는 행(id 9999)이든 숨은 행이든 결과는 값에만 달려 있다. 존재 여부를 구별할 수 없다.
    let hidden_a = ids(&rows(&mut db, &f, Some(1), hidden(json!({ "views": 0, "id": 103 }))).await);
    let ghost_a = ids(&rows(&mut db, &f, Some(1), hidden(json!({ "views": 0, "id": 9999 }))).await);
    let hidden_b = ids(&rows(&mut db, &f, Some(1), hidden(json!({ "views": 1, "id": 101 }))).await);
    let ghost_b = ids(&rows(&mut db, &f, Some(1), hidden(json!({ "views": 1, "id": 9998 }))).await);
    // 같은 (views, 위치) 구분: id가 경계보다 큰 동률 행만 달라지므로 id 대소 기준으로 동일해야 한다.
    let expect = |b: i64, v: i64| -> Vec<i64> {
        let all = [(113, 1), (110, 2), (111, 2), (112, 2), (100, 5), (104, 5), (106, 5), (108, 5), (109, 5), (102, 9), (107, 9)];
        all.iter().filter(|(i, vw)| *vw > v || (*vw == v && *i > b)).map(|(i, _)| *i).collect()
    };
    if hidden_a != expect(103, 0) || ghost_a != expect(9999, 0) {
        fails.push(format!("경계 비교 불일치 a: {hidden_a:?} {ghost_a:?}"));
    }
    if hidden_b != expect(101, 1) || ghost_b != expect(9998, 1) {
        fails.push(format!("경계 비교 불일치 b: {hidden_b:?} {ghost_b:?}"));
    }
    // 사용자 3(학교 2)에게는 101이 보이고 같은 경계가 그 행을 건너뛴다.
    let u3 = ids(&rows(&mut db, &f, Some(3), hidden(json!({ "views": 0, "id": 103 }))).await);
    if !u3.contains(&101) {
        fails.push(format!("사용자 3은 101을 봐야 함 {u3:?}"));
    }

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
