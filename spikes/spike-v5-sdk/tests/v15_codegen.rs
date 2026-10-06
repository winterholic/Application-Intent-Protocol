use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_fingerprint_with_wire, contract_module_with_apply, contract_ts_with_apply};

const SOURCE: &str = include_str!("../../../prototype/example/app.aip");
const IMPORT: &str = "../sdk/generic.ts";

fn facts(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|diags| panic!("V15 fixture parse failed: {diags:?}")).execution
}

fn replace_once(src: &str, old: &str, new: &str) -> String {
    assert_eq!(src.matches(old).count(), 1, "V15 mutation target must occur exactly once: {old}");
    src.replacen(old, new, 1)
}

#[test]
fn where_contract_uses_v15_scalar_types_without_expanding_read_projection() {
    let src = replace_once(SOURCE, "title.eq, ", "");
    let src =
        replace_once(&src, "title: Text; link: Url; phase: Phase; date: Time", "title: Text; privateNote: Text; link: Url; phase: Phase; date: Time");
    let f = facts(&src);
    let types = contract_ts_with_apply(&f, IdWire::DecimalString);
    let read = types.split("export interface ApplyContract").next().expect("read Contract projection");
    let apply = types.split("export interface ApplyContract").nth(1).expect("ApplyContract projection");

    assert!(read.contains("link: string;"), "Url remains a string in the read contract");
    assert!(read.contains("phase: \"READY\" | \"DONE\";"), "Enum variants remain a literal union in the read contract");
    assert!(read.contains("date: string;"), "Time remains a string in the read contract");
    assert!(read.contains("title: string;"), "title remains publicly readable");
    assert!(!types.contains("privateNote:"), "a field absent from public select/filter stays out of both projections");

    assert!(apply.contains("\"Event.mark\""));
    assert!(apply.contains("maxRows: 20;"));
    assert!(apply.contains("targets: \"id\" | \"where\";"));
    assert!(apply.contains("link: string;"), "Url eq is executable as a text value");
    assert!(apply.contains("phase: \"READY\" | \"DONE\";"), "Enum where is restricted to declared variants");
    assert!(apply.contains("date: string;"), "Time eq is represented as a string input");
    assert!(!apply.contains("title:"), "title.prefix alone is not an apply where value");
}

#[test]
fn adding_a_public_enum_member_changes_read_and_apply_fingerprints() {
    let src = replace_once(SOURCE, "enum Phase { READY, DONE }", "enum Phase { READY, DONE, PAUSED }");
    let base = facts(SOURCE);
    let changed = facts(&src);
    assert_ne!(base["enums"]["Phase"], changed["enums"]["Phase"]);

    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        assert_ne!(
            contract_fingerprint_with_wire(&base, wire),
            contract_fingerprint_with_wire(&changed, wire),
            "a public read enum change must change the read contract fingerprint"
        );
        assert_ne!(
            contract_fingerprint_with_apply(&base, wire),
            contract_fingerprint_with_apply(&changed, wire),
            "a public where enum change must change the combined contract fingerprint"
        );
        assert!(contract_ts_with_apply(&changed, wire).contains("\"READY\" | \"DONE\" | \"PAUSED\";"));
    }
}

#[test]
fn public_action_target_bulk_where_and_value_changes_change_fingerprint() {
    let base = facts(SOURCE);
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let fingerprint = contract_fingerprint_with_apply(&base, wire);

        let mut action_removed = base.clone();
        action_removed["resources"]["Event"]["exposeApply"].as_object_mut().unwrap().remove("mark");
        assert_ne!(contract_fingerprint_with_apply(&action_removed, wire), fingerprint, "public action set affects the fingerprint");

        let mut target_changed = base.clone();
        target_changed["resources"]["Event"]["exposeApply"]["mark"]["target"] = serde_json::json!(["id"]);
        assert_ne!(contract_fingerprint_with_apply(&target_changed, wire), fingerprint, "target forms affect the fingerprint");

        let mut bulk_changed = base.clone();
        bulk_changed["resources"]["Event"]["exposeApply"]["mark"]["bulkMaxRows"] = serde_json::json!(19);
        assert_ne!(contract_fingerprint_with_apply(&bulk_changed, wire), fingerprint, "bulk limit affects the fingerprint");

        let mut where_changed = base.clone();
        where_changed["resources"]["Event"]["exposeRead"]["filter"] =
            serde_json::json!(["id.eq", "title.eq", "title.prefix", "phase.eq", "date.eq", "date.gte"]);
        assert_ne!(contract_fingerprint_with_apply(&where_changed, wire), fingerprint, "eq where allowlist affects the fingerprint");

        let mut value_changed = base.clone();
        value_changed["resources"]["Event"]["fields"]["phase"]["ty"] = serde_json::json!("Text");
        assert_ne!(contract_fingerprint_with_apply(&value_changed, wire), fingerprint, "where value type affects the fingerprint");
    }
}

#[test]
fn docs_and_internal_policy_do_not_change_binding_fingerprint() {
    let with_docs = replace_once(
        SOURCE,
        "  expose apply mark { target id, where; bulk maxRows 20 }",
        "  expose apply mark { target id, where; bulk maxRows 20 }\n  docs { summary \"문서 변경\"; visibility internal }",
    );
    let with_policy = replace_once(SOURCE, "allow member = actor", "allow member != actor");
    let base = facts(SOURCE);
    let docs = facts(&with_docs);
    let policy = facts(&with_policy);
    assert_eq!(base, docs, "resource docs belong to metadata, not execution facts");
    assert_ne!(base["resources"]["Event"]["transitions"]["mark"]["allow"], policy["resources"]["Event"]["transitions"]["mark"]["allow"]);

    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let fingerprint = contract_fingerprint_with_apply(&base, wire);
        assert_eq!(contract_fingerprint_with_apply(&docs, wire), fingerprint);
        assert_eq!(contract_fingerprint_with_apply(&policy, wire), fingerprint);
        let module = contract_module_with_apply(&base, IMPORT, wire);
        assert!(module.contains("ApplyBinding<Contract, ApplyContract>"));
        assert!(module.contains(&format!("fingerprint: \"{fingerprint}\"")));
        assert!(module.contains(&format!("idWire: \"{}\"", wire.label())));
    }
}
