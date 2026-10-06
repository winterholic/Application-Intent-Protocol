use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::sqlgen::{Ctx, Env};

#[test]
fn accepted_definition_depth_is_executable() {
    let mut policy = json!({"lit":true});
    for _ in 0..63 {
        policy = json!({"not":policy});
    }
    let facts = json!({});
    let mut context = Ctx::new(&facts, None, "");
    assert!(context.cond(&policy, &Env::default()).is_ok(), "64 policy frames fit the definition budget");
}

#[test]
fn runtime_predicate_expansion_has_a_cumulative_work_limit() {
    let mut facts = json!({"predicates":{"p0":{"params":[],"body":{"lit":true}}}});
    for n in 1..=14 {
        let call = json!({"call":format!("p{}",n-1),"args":[]});
        facts["predicates"][format!("p{n}")] = json!({"params":[],"body":{"and":[call.clone(),call]}});
    }
    let call = json!({"call":"p14","args":[]});
    let policy = json!({"and":[call.clone(),call]});
    let mut context = Ctx::new(&facts, None, "");
    let error = context.cond(&policy, &Env::default()).expect_err("small source with repeated calls must not generate unbounded SQL");
    assert!(error.contains("작업량"), "{error}");
}

#[test]
fn deeply_chained_reference_paths_are_rejected_before_building_nested_sql() {
    let facts = json!({"actor":"Member","resources":{"Member":{"fields":{"parent":{"ty":"Ref<Member>?"}}}}});
    let path = json!({"path":{"root":"actor","segs":vec!["parent";10_000]}});
    let mut context = Ctx::new(&facts, Some(1), "");
    let error = context.path(&path, &Env::default()).expect_err("reference path must be bounded before SQL construction");
    assert!(error.contains("경로"), "{error}");
}

#[test]
fn raw_facts_cannot_supply_sql_comparison_text() {
    let facts = json!({});
    let expression = json!({"cmp":"= 1); SELECT pg_sleep(5); SELECT (1 =","l":{"lit":1},"r":{"lit":1}});
    let mut context = Ctx::new(&facts, None, "");
    assert!(context.cond(&expression, &Env::default()).is_err(), "operator must be one of the validated protocol comparisons");
}

#[test]
fn predicate_diamond_cannot_amplify_source_literal_payloads() {
    let base = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
    let active = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
    assert_eq!(base.matches(active).count(), 1);
    let payload = "x".repeat(128 * 1024);
    let definitions = std::iter::once(format!("predicate p0() = \"{payload}\" = \"{payload}\""))
        .chain((1..=6).map(|n| format!("predicate p{n}() = p{}() and p{}()", n - 1, n - 1)))
        .collect::<Vec<_>>()
        .join("\n");
    let source = base.replacen(active, &format!("predicate active(r: Recruitment) = p6()\n{definitions}"), 1);
    assert!(source.len() < 1 << 20, "source must stay below V1's 1 MiB ceiling: {} bytes", source.len());
    let facts = load_str(&source, Form::A).expect("the V1-bounded policy definition is valid").execution;
    let policy = json!({"call":"p6","args":[]});
    let mut context = Ctx::new(&facts, None, "");
    let error = context.cond(&policy, &Env::default()).expect_err("expanded payload copies must be bounded");
    assert!(error.contains("데이터"), "expected payload-data budget error, got {error}");
}
