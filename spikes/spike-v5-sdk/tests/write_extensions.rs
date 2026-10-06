use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::*;

const APP: &str = include_str!("../../../prototype/example/app.aip");

#[test]
fn write_projection_covers_public_contract_and_preserves_existing_read_family() {
    let mut facts = load_str(APP, Form::A).unwrap().execution;
    let write = json!({"kind":"write","effect":"db","input":[["id","Id<Event>"]],"output":[["id","Id<Event>"],["count","Int"]],"access":{"Event.mark":{}},"implementation":"event.run","deadlineMs":2000});
    let mut without_writes = facts.clone();
    without_writes["resources"]["Event"]["extensions"].as_object_mut().unwrap().retain(|_, extension| extension["kind"] != "write");
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        assert_eq!(contract_ts_with_all_extensions(&without_writes, wire), contract_ts_with_extensions(&without_writes, wire));
        assert_eq!(contract_fingerprint_with_all_extensions(&without_writes, wire), contract_fingerprint_with_extensions(&without_writes, wire));
        assert_eq!(
            contract_module_with_all_extensions(&without_writes, "../sdk/generic.ts", wire),
            contract_module_with_extensions(&without_writes, "../sdk/generic.ts", wire)
        );
    }
    facts["resources"]["Event"]["extensions"]["run"] = write;
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let old = contract_ts_with_extensions(&facts, wire);
        let types = contract_ts_with_all_extensions(&facts, wire);
        assert!(types.starts_with(&old));
        assert!(types.contains("export interface WriteExtensionContract"));
        assert!(types.contains("\"Event.run\""));
        assert!(types.contains("writeExtensionDescriptors"));
        assert!(types.contains("\"access\":[\"Event.mark\"]"));
        assert!(!types.contains("event.run"));
        let fp = contract_fingerprint_with_all_extensions(&facts, wire);
        let mut private = facts.clone();
        private["resources"]["Event"]["extensions"]["run"]["implementation"] = json!("private.module");
        private["resources"]["Event"]["extensions"]["run"]["deadlineMs"] = json!(3000);
        assert_eq!(fp, contract_fingerprint_with_all_extensions(&private, wire));
        private["resources"]["Event"]["extensions"]["run"]["output"] = json!([["id", "Id<Event>"], ["count", "Text"]]);
        assert_ne!(fp, contract_fingerprint_with_all_extensions(&private, wire));
        assert_eq!(old, contract_ts_with_extensions(&facts, wire));
    }
}
