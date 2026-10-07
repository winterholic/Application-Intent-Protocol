use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{plan_read_with_wire, Caller},
};

#[test]
fn prefix_limits_count_characters_and_reject_invalid_values_before_sql() {
    let source = include_str!("../../../prototype/example/app.aip");
    let facts = load_str(source, Form::A).expect("definition").execution;
    let caller = Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() };
    for wire in [IdWire::Legacy, IdWire::SafeNumber, IdWire::DecimalString] {
        for (value, expected) in [
            (json!("가".repeat(200)), None),
            (json!("가".repeat(201)), Some("VALUE_TOO_LONG")),
            (json!("a".repeat(200)), None),
            (json!("a".repeat(201)), Some("VALUE_TOO_LONG")),
            (json!(""), None),
            (json!("bad\u{0000}value"), Some("BAD_VALUE")),
            (json!(null), Some("BAD_VALUE")),
            (json!(["x"]), Some("BAD_VALUE")),
        ] {
            let request = json!({"read":"Event","select":["id"],"filter":[{"field":"title","op":"prefix","value":value}]});
            match (plan_read_with_wire(&facts, &request, &caller, wire), expected) {
                (Ok(plan), None) => assert!(plan.params.contains(&value.as_str().map(str::to_string))),
                (Err(error), Some(code)) => assert_eq!(error.code, code, "{wire:?}: {value}"),
                (Ok(_), Some(code)) => panic!("{wire:?}: expected {code}, accepted {value}"),
                (Err(error), None) => panic!("{wire:?}: rejected {value}: {error:?}"),
            }
        }
    }
}
