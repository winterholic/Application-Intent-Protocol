//! 감사 결함 회귀: facts가 sema를 거치지 않고 들어와도 정책 필드 filter/sort를 계획에서 거부하고,
//! Int 범위를 DDL CHECK로 집행한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    plan::{plan_read, Caller},
    sqlgen,
};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() }
}

#[test]
fn plan_rejects_policy_field_filter_and_sort_even_if_facts_allow_it() {
    let mut facts = load_str(A, Form::A).unwrap().execution;
    let ex = &mut facts["resources"]["Recruitment"]["exposeRead"];
    ex["filter"].as_array_mut().unwrap().push(json!("internalNote.eq"));
    ex["sort"].as_array_mut().unwrap().push(json!("internalNote"));
    let f = json!({"read":"Recruitment","select":["id"],"filter":[{"field":"internalNote","op":"eq","value":"x"}]});
    assert_eq!(plan_read(&facts, &f, &caller()).err().unwrap().code, "POLICY_FIELD_NOT_FILTERABLE");
    let s = json!({"read":"Recruitment","select":["id"],"sort":[{"field":"internalNote","dir":"asc"}]});
    assert_eq!(plan_read(&facts, &s, &caller()).err().unwrap().code, "POLICY_FIELD_NOT_FILTERABLE");
    // 정책 없는 필드는 그대로 계획된다.
    let ok = json!({"read":"Recruitment","select":["id"],"sort":[{"field":"views","dir":"asc"}]});
    assert!(plan_read(&facts, &ok, &caller()).is_ok());
}

#[test]
fn int_range_becomes_ddl_check() {
    let facts = load_str(&A.replacen("views: Int", "views: Int(0..10)", 1), Form::A).unwrap().execution;
    let ddl = sqlgen::ddl(&facts).unwrap().join("\n");
    assert!(ddl.contains("views bigint CHECK (views BETWEEN 0 AND 10) NOT NULL"), "{ddl}");
    let none: Value = load_str(A, Form::A).unwrap().execution;
    assert!(!sqlgen::ddl(&none).unwrap().join("\n").contains("views BETWEEN"));
}
