use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_fingerprint_with_wire, contract_module_with_apply, contract_ts_with_apply};

const SOURCE: &str = include_str!("../../spike-v6-transport/fixture/v14.aip");
fn facts() -> Value {
    load_str(SOURCE, Form::A).unwrap().execution
}

#[test]
fn apply_projection_includes_write_only_public_actions_and_real_targets() {
    let f = facts();
    let types = contract_ts_with_apply(&f, IdWire::DecimalString);
    assert!(types.contains("export interface ApplyContract"));
    assert!(types.contains("\"WriteOnly.mark\""));
    assert!(!types.contains("\"Inbox.hidden\""));
    assert!(types.contains("idOutput: Id<\"WriteOnly\">"));
    assert!(types.contains("maxRows: 20"));
    assert!(types.contains("targets: \"where\""));
    assert!(!types.split("export interface ApplyContract").nth(1).unwrap().contains("gte"));
}

#[test]
fn public_write_changes_change_candidate_fingerprint_even_without_read_changes() {
    let f = facts();
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let fp = contract_fingerprint_with_apply(&f, wire);
        assert_ne!(fp, contract_fingerprint_with_wire(&f, wire));
        for mutate in ["action", "target", "bulk", "where", "value"] {
            let mut changed = f.clone();
            match mutate {
                "action" => {
                    changed["resources"]["WriteOnly"]["exposeApply"].as_object_mut().unwrap().remove("mark");
                }
                "target" => changed["resources"]["Inbox"]["exposeApply"]["mark"]["target"] = serde_json::json!(["id"]),
                "bulk" => changed["resources"]["WriteOnly"]["exposeApply"]["mark"]["bulkMaxRows"] = serde_json::json!(1),
                "where" => changed["resources"]["WhereOnly"]["exposeRead"]["filter"] = serde_json::json!([]),
                "value" => changed["resources"]["WhereOnly"]["fields"]["title"]["ty"] = serde_json::json!("Int"),
                _ => unreachable!(),
            }
            assert_ne!(contract_fingerprint_with_apply(&changed, wire), fp, "public {mutate} must affect fingerprint");
        }
    }
}

#[test]
fn internal_policy_and_docs_stay_outside_public_apply_projection() {
    let f = facts();
    let mut changed = f.clone();
    changed["resources"]["WriteOnly"]["transitions"]["mark"]["allow"] = serde_json::json!({"literal": false});
    changed["docs"] = serde_json::json!({"description":"this text grants no access"});
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        assert_eq!(contract_fingerprint_with_apply(&changed, wire), contract_fingerprint_with_apply(&f, wire));
        let module = contract_module_with_apply(&f, "../sdk/generic.ts", wire);
        assert!(module.contains("ApplyBinding<Contract, ApplyContract>"));
        assert!(module.contains(&format!("fingerprint: \"{}\"", contract_fingerprint_with_apply(&f, wire))));
        assert!(module.contains(&format!("idWire: \"{}\"", wire.label())));
    }
}

#[test]
fn where_projection_tracks_the_shared_scalar_executor_support() {
    let source = SOURCE
        .replace("count: Int; title: Text", "count: Int; title: Text; date: Time")
        .replace("count.gte, title.eq", "count.gte, title.eq, date.eq");
    let f = load_str(&source, Form::A).unwrap().execution;
    let types = contract_ts_with_apply(&f, IdWire::DecimalString);
    let apply = types.split("export interface ApplyContract").nth(1).unwrap_or("");
    assert!(apply.contains("date: string;"), "V15 shared parser executes Time where values");
}
