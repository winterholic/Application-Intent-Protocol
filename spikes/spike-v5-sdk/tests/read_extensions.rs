use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::*;
const APP: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
#[test]
fn read_extensions_add_public_types_and_descriptors_without_changing_old_contracts() {
    let facts = load_str(APP, Form::A).unwrap().execution;
    for wire in [IdWire::SafeNumber, IdWire::DecimalString, IdWire::Legacy] {
        let old = contract_ts_with_apply(&facts, wire);
        let new = contract_ts_with_extensions(&facts, wire);
        assert!(new.contains("export interface ExtensionContract"), "missing public extension contract");
        assert!(new.contains("\"Recruitment.stats\""));
        assert!(new.contains("approvedApplicants: number"));
        assert!(new.starts_with(&old));
        assert_eq!(contract_ts_with_apply(&facts, wire), old);
        let module = contract_module_with_extensions(&facts, "test-sdk", wire);
        assert!(module.contains("ExtensionBinding<Contract, ApplyContract, ExtensionContract>"));
        assert!(module.contains("extensions:"));
        let mut private = facts.clone();
        private["resources"]["Recruitment"]["extensions"]["stats"]["implementation"] = json!("different.fn");
        assert_eq!(contract_fingerprint_with_extensions(&facts, wire), contract_fingerprint_with_extensions(&private, wire));
        private["resources"]["Recruitment"]["extensions"]["stats"]["output"] = json!([["approvedApplicants", "Text"]]);
        assert_ne!(contract_fingerprint_with_extensions(&facts, wire), contract_fingerprint_with_extensions(&private, wire));
    }
}
#[test]
fn no_read_extensions_still_emit_public_read_descriptors() {
    let mut facts = load_str(APP, Form::A).unwrap().execution;
    facts["resources"]["Recruitment"]["extensions"] = json!({});
    facts["resources"]["Recruitment"]["fields"]["privateExtra"] = json!({"ty":"Text"});
    for wire in [IdWire::SafeNumber, IdWire::DecimalString, IdWire::Legacy] {
        let module = contract_module_with_extensions(&facts, "test-sdk", wire);
        assert!(module.contains("readDescriptors:"));
        assert!(module.contains("\"bookmarkCount\""));
        assert!(!module.contains("privateExtra"));
        assert_ne!(contract_fingerprint_with_extensions(&facts, wire), contract_fingerprint_with_apply(&facts, wire));
    }
}

#[test]
fn descriptors_preserve_nullable_enums_and_exclude_write_extensions() {
    let mut facts = load_str(APP, Form::A).unwrap().execution;
    let ext = &mut facts["resources"]["Recruitment"]["extensions"]["stats"];
    ext["output"] = json!([["role", "Enum<ClubRole>?"], ["id", "Id<Club>?"]]);
    let types = contract_ts_with_extensions(&facts, IdWire::DecimalString);
    assert!(types.contains("role: \"ADMIN\" | \"MANAGER\" | \"MEMBER\" | null"));
    assert!(types.contains("id: Id<\"Club\"> | null"));
    assert!(types.contains("\"nullable\":true"));
    assert!(types.contains("\"values\":[\"ADMIN\",\"MANAGER\",\"MEMBER\"]"));
    let fp = contract_fingerprint_with_extensions(&facts, IdWire::DecimalString);
    facts["enums"]["ClubRole"].as_array_mut().unwrap().push(json!("OWNER"));
    assert_ne!(fp, contract_fingerprint_with_extensions(&facts, IdWire::DecimalString));
    facts["resources"]["Recruitment"]["extensions"]["stats"]["kind"] = json!("write");
    let module = contract_module_with_extensions(&facts, "sdk", IdWire::DecimalString);
    assert!(!module.contains("Recruitment.stats"));
    assert!(module.contains("readDescriptors:"));
}

#[test]
fn public_read_projection_excludes_internal_traverse_execution_markers() {
    let facts = load_str(APP, Form::A).unwrap().execution;
    let mut internal = facts.clone();
    internal["resources"]["Recruitment"]["exposeRead"]["traverse"]["club"]["reapply"] = json!(["internal-change"]);
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let module = contract_module_with_extensions(&facts, "sdk", wire);
        assert!(!module.contains("reapply"));
        assert_eq!(contract_fingerprint_with_extensions(&facts, wire), contract_fingerprint_with_extensions(&internal, wire));
    }
}
