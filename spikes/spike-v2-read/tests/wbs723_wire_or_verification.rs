use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn facts() -> Value {
    let source = SOURCE.replacen(
        "filter periodEnd.gte, periodEnd.lte",
        "filter periodEnd.gte, periodEnd.lte, title.contains",
        1,
    );
    load_str(&source, Form::A).expect("compile the test definition").execution
}

#[test]
fn read_wire_accepts_or_disjunction_filter() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let valid = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"field": "title", "op": "contains", "value": "cat"}]
    });
    assert!(plan_read(&facts, &valid, &caller).is_ok());

    let disjunction = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "title", "op": "contains", "value": "dog"}
        ]}]
    });
    let plan = match plan_read(&facts, &disjunction, &caller) {
        Ok(p) => p,
        Err(e) => panic!("OR disjunction should be accepted: {e:?}"),
    };
    let sql = plan.sql;
    assert!(
        sql.contains("strpos(") && sql.contains(" OR "),
        "expected OR condition in SQL: {sql}"
    );
    assert!(plan.params.contains(&Some("cat".to_string())));
    assert!(plan.params.contains(&Some("dog".to_string())));
}

#[test]
fn read_wire_rejects_unexposed_field_in_or() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let unexposed = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "views", "op": "gt", "value": 10}
        ]}]
    });
    let err = match plan_read(&facts, &unexposed, &caller) {
        Err(e) => e,
        Ok(_) => panic!("unexposed field in OR must be rejected"),
    };
    assert_eq!(err.code, "WIRE.FILTER_NOT_ALLOWED");
}

#[test]
fn read_wire_rejects_policy_redacted_field_in_or() {
    let mut facts = facts();
    // Simulate facts that bypassed sema to verify defense-in-depth runtime check
    facts["resources"]["Recruitment"]["exposeRead"]["filter"]
        .as_array_mut()
        .unwrap()
        .push(json!("internalNote.contains"));
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let redacted = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "internalNote", "op": "contains", "value": "secret"}
        ]}]
    });
    let err = match plan_read(&facts, &redacted, &caller) {
        Err(e) => e,
        Ok(_) => panic!("policy field in OR must be rejected"),
    };
    assert_eq!(err.code, "WIRE.POLICY_FIELD_NOT_FILTERABLE");
}

#[test]
fn read_wire_rejects_empty_or_array() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let empty_or = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": []}]
    });
    let err = match plan_read(&facts, &empty_or, &caller) {
        Err(e) => e,
        Ok(_) => panic!("empty OR array must be rejected"),
    };
    assert_eq!(err.code, "WIRE.BAD_VALUE");
}

#[test]
fn read_wire_or_preserves_row_read_policy() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let disjunction = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "title", "op": "contains", "value": "dog"}
        ]}]
    });
    let plan = match plan_read(&facts, &disjunction, &caller) {
        Ok(p) => p,
        Err(e) => panic!("valid OR filter should pass: {e:?}"),
    };
    assert!(plan.sql.contains("WHERE ("), "WHERE clause must wrap conditions: {}", plan.sql);
    assert!(plan.sql.contains(") AND ("), "Row policy must be AND-combined with filters: {}", plan.sql);
}

#[test]
fn public_read_planner_accepts_a_predeclared_count_aggregate() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let request = json!({"aggregate": "Apply.approvedCount", "input": {"clubId": "10"}});
    let plan = plan_read(&facts, &request, &caller).unwrap_or_else(|error| panic!("{error:?}"));
    assert!(plan.sql.to_ascii_lowercase().contains("count(*)"), "{}", plan.sql);
    assert_eq!(plan.output_type["value"]["ty"], "Int");
}

#[test]
fn read_wire_or_across_multiple_fields() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let disjunction = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "periodEnd", "op": "gte", "value": "2026-10-01T00:00:00Z"}
        ]}]
    });
    let plan = match plan_read(&facts, &disjunction, &caller) {
        Ok(p) => p,
        Err(e) => panic!("multiple fields in OR should pass: {e:?}"),
    };
    let sql = plan.sql;
    assert!(sql.contains("strpos(") && sql.contains(" >= ") && sql.contains(" OR "));
    assert!(plan.params.contains(&Some("cat".to_string())));
    assert!(plan.params.contains(&Some("2026-10-01T00:00:00+00:00".to_string())));
}

#[test]
fn read_wire_or_with_and_combination() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let combination = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [
            {"field": "periodEnd", "op": "gte", "value": "2026-10-01T00:00:00Z"},
            {"or": [
                {"field": "title", "op": "contains", "value": "cat"},
                {"field": "title", "op": "contains", "value": "dog"}
            ]}
        ]
    });
    let plan = match plan_read(&facts, &combination, &caller) {
        Ok(p) => p,
        Err(e) => panic!("AND-OR combination should pass: {e:?}"),
    };
    let sql = plan.sql;
    assert!(sql.contains(" >= ") && sql.contains("strpos(") && sql.contains(" OR "));
}

#[test]
fn read_wire_rejects_nested_or() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let nested = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"or": [{"field": "title", "op": "contains", "value": "cat"}]}
        ]}]
    });
    let err = match plan_read(&facts, &nested, &caller) {
        Err(e) => e,
        Ok(_) => panic!("nested OR must be rejected"),
    };
    assert_eq!(err.code, "WIRE.BAD_REQUEST");
}

#[test]
fn read_wire_rejects_exceeded_or_list() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let mut or_items = vec![];
    for i in 0..11 {
        or_items.push(json!({"field": "title", "op": "contains", "value": format!("test{i}")}));
    }
    let exceeded = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": or_items}]
    });
    let err = match plan_read(&facts, &exceeded, &caller) {
        Err(e) => e,
        Ok(_) => panic!("exceeded OR conditions must be rejected"),
    };
    assert_eq!(err.code, "WIRE.OR_LIST_EXCEEDED");
}

#[test]
fn read_wire_rejects_duplicate_or_items() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let dup = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "title", "op": "contains", "value": "cat"}
        ]}]
    });
    let err = match plan_read(&facts, &dup, &caller) {
        Err(e) => e,
        Ok(_) => panic!("duplicate OR item must be rejected"),
    };
    assert_eq!(err.code, "WIRE.DUPLICATE");
}

#[test]
fn read_wire_or_prevents_sql_injection() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let injection_input = "cat' OR '1'='1' --";
    let disjunction = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": injection_input},
            {"field": "title", "op": "contains", "value": "dog"}
        ]}]
    });
    let plan = match plan_read(&facts, &disjunction, &caller) {
        Ok(p) => p,
        Err(e) => panic!("injection payload should be safely bound as param: {e:?}"),
    };
    // The malicious string must be inside params, not in the raw SQL
    assert!(!plan.sql.contains("1'='1"), "SQL must not interpolate injection input: {}", plan.sql);
    assert!(plan.params.contains(&Some(injection_input.to_string())));
}

#[test]
fn read_wire_or_with_mysql_dialect() {
    let facts = facts();
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let disjunction = json!({
        "read": "Recruitment",
        "select": ["id"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "cat"},
            {"field": "title", "op": "contains", "value": "dog"}
        ]}]
    });
    let plan = spike_v2_read::plan::plan_read_in(
        spike_v2_read::sqlgen::Dialect::MySql,
        &facts,
        &disjunction,
        &caller,
        spike_v2_read::id_wire::IdWire::Legacy,
    ).expect("MySQL OR plan");
    assert!(
        plan.sql.contains("LOCATE(") && plan.sql.contains(" OR "),
        "MySQL must use LOCATE for contains in OR: {}", plan.sql
    );
}

const FACEREVIEW_SOURCE: &str = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");

fn facereview_facts() -> Value {
    load_str(FACEREVIEW_SOURCE, Form::A).expect("compile facereview.aip").execution
}

#[test]
fn facereview_video_or_search_planner_mysql() {
    let facts = facereview_facts();
    let caller = Caller { actor_id: None, now: "2026-10-18T00:00:00Z".into() };
    let query = json!({
        "read": "Video",
        "select": ["id", "title", "channel_name"],
        "filter": [{"or": [
            {"field": "title", "op": "contains", "value": "고양이"},
            {"field": "channel_name", "op": "contains", "value": "쿡채널"}
        ]}],
        "sort": [{"field": "created_at", "dir": "desc"}],
        "limit": 10,
        "offset": 0
    });
    let plan = spike_v2_read::plan::plan_read_in(
        spike_v2_read::sqlgen::Dialect::MySql,
        &facts,
        &query,
        &caller,
        spike_v2_read::id_wire::IdWire::Legacy,
    ).expect("FaceReview Video OR search plan");

    let sql = plan.sql;
    assert!(sql.contains("LOCATE("), "MySQL LOCATE must be used: {sql}");
    assert!(sql.contains(" OR "), "OR disjunction must be generated: {sql}");
    assert!(sql.contains("is_deleted"), "Video row read policy must be included: {sql}");
    assert!(plan.params.contains(&Some("고양이".to_string())));
    assert!(plan.params.contains(&Some("쿡채널".to_string())));
}

#[test]
fn facereview_video_total_count_aggregate_mysql() {
    let facts = facereview_facts();
    // 1. Anonymous guest caller
    let guest = Caller { actor_id: None, now: "2026-10-18T00:00:00Z".into() };
    let req = json!({"aggregate": "Video.totalCount", "input": {}});
    let plan = spike_v2_read::plan::plan_read_in(
        spike_v2_read::sqlgen::Dialect::MySql,
        &facts,
        &req,
        &guest,
        spike_v2_read::id_wire::IdWire::Legacy,
    ).expect("guest Video totalCount aggregate");
    assert!(plan.sql.to_ascii_lowercase().contains("count(*)"), "count(*) must be present: {}", plan.sql);
    assert_eq!(plan.output_type["value"]["ty"], "Int");

    // 2. Authenticated active member caller
    let member = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let plan = spike_v2_read::plan::plan_read_in(
        spike_v2_read::sqlgen::Dialect::MySql,
        &facts,
        &req,
        &member,
        spike_v2_read::id_wire::IdWire::Legacy,
    ).expect("member Video totalCount aggregate");
    assert!(plan.sql.to_ascii_lowercase().contains("count(*)"), "count(*) must be present: {}", plan.sql);
}

