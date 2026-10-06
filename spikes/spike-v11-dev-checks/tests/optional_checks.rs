use serde_json::{json, Value};
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v5_sdk::contract_ts;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ETS: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.e.ts");
const EPY: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.e.py");
const HTS: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.h.ts");
const HPY: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.h.py");
const ALL: [Form; 5] = [Form::A, Form::ETs, Form::EPy, Form::HTs, Form::HPy];
const NOW: &str = "2026-10-04T00:00:00Z";

fn fixture(form: Form) -> &'static str {
    match form {
        Form::A => A,
        Form::ETs => ETS,
        Form::EPy => EPY,
        Form::HTs => HTS,
        Form::HPy => HPY,
    }
}

fn checked(src: &str, form: Form, options: Value) -> spike_v11_dev_checks::Checked {
    load_checked(src, form, &options).unwrap_or_else(|ds| panic!("{form:?}: {ds:?}"))
}

fn diag_codes(result: Result<spike_v11_dev_checks::Checked, Vec<spike_v1_fixture::diag::Diag>>) -> Vec<&'static str> {
    match result {
        Ok(_) => vec![],
        Err(ds) => ds.iter().map(|d| d.code).collect(),
    }
}

fn docs_syntax(form: Form) -> (&'static str, &'static str) {
    match form {
        Form::A | Form::ETs | Form::EPy => {
            ("docs { summary \"모집 정보\"; visibility internal }", "docs { summary \"새 설명\"; visibility internal }")
        }
        Form::HTs => ("docs: { summary: \"모집 정보\", visibility: \"internal\" },", "docs: { summary: \"새 설명\", visibility: \"internal\" },"),
        Form::HPy => (
            "\"docs\": {\"summary\": \"모집 정보\", \"visibility\": \"internal\"},",
            "\"docs\": {\"summary\": \"새 설명\", \"visibility\": \"internal\"},",
        ),
    }
}

fn replace_once(src: &str, old: &str, new: &str) -> String {
    assert_eq!(src.matches(old).count(), 1, "변형 대상이 원문에 정확히 한 번 있어야 함: {old}");
    src.replacen(old, new, 1)
}

fn read_request() -> Value {
    json!({
        "read": "Recruitment",
        "select": ["id", "title", "periodEnd", "bookmarkCount", "internalNote", {"club": {"select": ["id", "name", "logo"]}}],
        "sort": [{"field": "periodEnd", "dir": "asc"}],
        "limit": 20
    })
}

fn plan_code(facts: &Value, request: &Value) -> &'static str {
    match plan_read(facts, request, &Caller { actor_id: Some(1), now: NOW.into() }) {
        Ok(_) => "OK",
        Err(e) => e.code,
    }
}

#[test]
fn all_five_forms_default_to_off_and_preserve_v1_and_v5_outputs() {
    for form in ALL {
        let src = fixture(form);
        let default = checked(src, form, json!({}));
        let explicit_off = checked(src, form, json!({"devChecks": false}));
        let on = checked(src, form, json!({"devChecks": true}));
        assert!(default.advisories.is_empty(), "{form:?}: 기본값은 off");
        assert!(explicit_off.advisories.is_empty(), "{form:?}: off는 조언 없음");
        assert!(on.advisories.is_empty(), "{form:?}: 기본 fixture에는 미참조 predicate 없음");
        for candidate in [&explicit_off, &on] {
            assert_eq!(digest(&candidate.output.execution), digest(&default.output.execution), "{form:?}: facts 변경");
            assert_eq!(contract_ts(&candidate.output.execution), contract_ts(&default.output.execution), "{form:?}: V5 TS 계약 변경");
        }
    }
}

#[test]
fn docs_removal_edit_move_and_span_shift_leave_execution_and_v5_contract_unchanged() {
    for form in ALL {
        let src = fixture(form);
        let base = checked(src, form, json!({}));
        let (old, edited) = docs_syntax(form);
        let removed_src = replace_once(src, old, "");
        let edited_src = replace_once(src, old, edited);
        let removed = checked(&removed_src, form, json!({}));
        let edited = checked(&edited_src, form, json!({}));
        for (label, candidate) in [("removed", &removed), ("edited", &edited)] {
            assert_eq!(digest(&candidate.output.execution), digest(&base.output.execution), "{form:?} docs {label}: execution");
            assert_eq!(contract_ts(&candidate.output.execution), contract_ts(&base.output.execution), "{form:?} docs {label}: TS");
            assert_ne!(candidate.output.metadata, base.output.metadata, "{form:?} docs {label}: metadata anchor");
        }

        let shifted_src = replace_once(src, old, &format!("\n\n{old}"));
        let shifted = checked(&shifted_src, form, json!({}));
        assert_eq!(digest(&shifted.output.execution), digest(&base.output.execution), "{form:?} docs span 이동: execution");
        assert_eq!(contract_ts(&shifted.output.execution), contract_ts(&base.output.execution), "{form:?} docs span 이동: TS");
        assert_eq!(shifted.output.metadata, base.output.metadata, "{form:?} span 이동은 metadata 값을 바꾸지 않아야 함");
        assert_ne!(shifted.output.spans, base.output.spans, "{form:?} span 이동이 anchor 위치에 반영되어야 함");
    }
}

#[test]
fn moving_docs_changes_only_the_metadata_anchor_not_execution_or_contract() {
    let original = A;
    let docs = "docs { summary \"모집 정보\"; visibility internal }";
    let without_recruitment_docs = replace_once(original, docs, "");
    let moved_src =
        replace_once(&without_recruitment_docs, "resource Club {", "resource Club {\n  docs { summary \"모집 정보\"; visibility internal }");
    let before = checked(original, Form::A, json!({}));
    let after = checked(&moved_src, Form::A, json!({}));
    assert_eq!(digest(&after.output.execution), digest(&before.output.execution));
    assert_eq!(contract_ts(&after.output.execution), contract_ts(&before.output.execution));
    assert!(before.output.metadata.get("resource:Recruitment").is_some());
    assert!(after.output.metadata.get("resource:Recruitment").is_none());
    assert!(after.output.metadata.get("resource:Club").is_some());
    assert_ne!(after.output.spans, before.output.spans);
}

#[test]
fn only_direct_execution_calls_suppress_unused_predicate_advisories() {
    let with_unused = format!("{A}\npredicate orphan(m: Member) = m.id = m.id\n");
    let off = checked(&with_unused, Form::A, json!({"devChecks": false}));
    let on = checked(&with_unused, Form::A, json!({"devChecks": true}));
    assert!(checked(&with_unused, Form::A, json!({})).advisories.is_empty());
    assert!(off.advisories.is_empty());
    assert_eq!(on.advisories.len(), 1);
    assert_eq!(on.advisories[0].code, "ADVISORY_UNUSED_PREDICATE");
    assert_eq!(on.advisories[0].anchor, "predicate:orphan");
    assert_eq!(digest(&off.output.execution), digest(&on.output.execution));
    assert_eq!(contract_ts(&off.output.execution), contract_ts(&on.output.execution));
    assert!(on.output.spans.get("predicate:orphan").is_none(), "predicate 조언은 별도 source span을 만들지 않음");

    let docs_mention = replace_once(
        &with_unused,
        "docs { summary \"모집 정보\"; visibility internal }",
        "docs { summary \"orphan predicate\"; visibility internal }",
    );
    let mentioned = checked(&docs_mention, Form::A, json!({"devChecks": true}));
    assert_eq!(mentioned.advisories.len(), 1, "docs의 텍스트는 typed execution 호출로 세지 않음");
    assert_eq!(mentioned.advisories[0].anchor, "predicate:orphan");
}

#[test]
fn direct_call_rule_does_not_claim_transitive_reachability() {
    let source = format!("{A}\npredicate orphan(m: Member) = m.id = m.id\npredicate isolatedCaller(m: Member) = orphan(m)\n");
    let on = checked(&source, Form::A, json!({"devChecks": true}));
    assert_eq!(on.advisories.iter().map(|a| a.anchor.as_str()).collect::<Vec<_>>(), ["predicate:isolatedCaller"]);
}

#[test]
fn unknown_docs_and_invalid_options_are_errors() {
    let unknown_docs = replace_once(
        A,
        "docs { summary \"모집 정보\"; visibility internal }",
        "docs { summary \"모집 정보\"; visibility internal; audience public }",
    );
    for options in [json!({}), json!({"devChecks": true})] {
        assert!(diag_codes(load_checked(&unknown_docs, Form::A, &options)).contains(&"PARSE_UNKNOWN_KEY"));
    }

    assert!(diag_codes(load_checked(A, Form::A, &json!({"unknown": true}))).contains(&"UNKNOWN_OPTION"));
    for options in [json!({"devChecks": "true"}), json!({"devChecks": null}), json!({"devChecks": 1}), json!(null), json!([]), json!("on")] {
        assert!(diag_codes(load_checked(A, Form::A, &options)).contains(&"BAD_OPTION"), "허용하지 않은 옵션 형식: {options}");
    }
}

#[test]
fn required_definition_type_errors_fail_with_development_checks_off_or_on() {
    let broken = replace_once(A, "title: Text(1..100)", "title: MissingType");
    for options in [json!({}), json!({"devChecks": false}), json!({"devChecks": true})] {
        assert!(diag_codes(load_checked(&broken, Form::A, &options)).contains(&"UNKNOWN_TYPE"));
    }
}

#[test]
fn raw_read_rejections_are_identical_with_development_checks_off_or_on() {
    let facts = checked(A, Form::A, json!({})).output.execution;
    let cost_src = replace_once(A, "cost 1000", "cost 50");
    let cases: [(&str, &str, Value, &str); 8] = [
        ("closed field", A, json!({"select": ["id", "status"]}), "FIELD_NOT_EXPOSED"),
        ("bad filter type", A, json!({"filter": [{"field": "periodEnd", "op": "gte", "value": 7}]}), "BAD_VALUE"),
        ("private filter", A, json!({"filter": [{"field": "internalNote", "op": "eq", "value": "A-only"}]}), "FILTER_NOT_ALLOWED"),
        ("closed operator", A, json!({"filter": [{"field": "periodEnd", "op": "eq", "value": NOW}]}), "FILTER_NOT_ALLOWED"),
        ("closed sort", A, json!({"sort": [{"field": "title", "dir": "asc"}]}), "SORT_NOT_ALLOWED"),
        ("row budget", A, json!({"limit": 51}), "ROWS_EXCEEDED"),
        ("relation depth", A, json!({"select": ["id", {"club": {"select": ["id", {"school": {"select": ["id"]}}]}}]}), "DEPTH_EXCEEDED"),
        ("cost budget", &cost_src, json!({}), "COST_EXCEEDED"),
    ];

    for (name, source, patch, expected) in cases {
        let mut request = read_request();
        if let Some(object) = patch.as_object() {
            for (key, value) in object {
                request[key] = value.clone();
            }
        }
        let selected_facts = checked(source, Form::A, json!({})).output.execution;
        let off = checked(source, Form::A, json!({"devChecks": false}));
        let on = checked(source, Form::A, json!({"devChecks": true}));
        assert_eq!(plan_code(&selected_facts, &request), expected, "{name} 기준 거부");
        assert_eq!(digest(&off.output.execution), digest(&on.output.execution), "{name}: devChecks가 실행 facts를 바꿈");
        assert_eq!(plan_code(&off.output.execution, &request), expected, "{name} off");
        assert_eq!(plan_code(&on.output.execution, &request), expected, "{name} on");
    }

    let mut runtime_option = read_request();
    runtime_option["devChecks"] = json!(false);
    assert_eq!(plan_code(&facts, &runtime_option), "UNKNOWN_KEY", "개발 옵션은 raw runtime 요청에서 거부되어야 함");
}
