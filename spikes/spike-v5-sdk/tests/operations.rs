use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_all_extensions, contract_module_with_all_extensions};

#[test]
fn operation_contracts_are_typed_discoverable_and_exclude_private_policy_and_implementation() {
    let facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let generated = contract_module_with_all_extensions(&facts, "@aip/client", wire);
        assert!(generated.contains("export interface OperationContract"), "{generated}");
        assert!(generated.contains("operationDescriptors"));
        assert!(generated.contains("operations: operationDescriptors"));
        assert!(!generated.contains("compute.length"));
        assert!(!generated.contains("actor"));
        assert!(generated.contains("deadlineMs"));
        let mut changed = facts.clone();
        changed["operations"]["length"]["output"][0][1] = serde_json::json!("Text");
        assert_ne!(contract_fingerprint_with_all_extensions(&facts, wire), contract_fingerprint_with_all_extensions(&changed, wire));
        changed = facts.clone();
        changed["operations"]["length"]["implementation"] = serde_json::json!("internal.different");
        assert_eq!(contract_fingerprint_with_all_extensions(&facts, wire), contract_fingerprint_with_all_extensions(&changed, wire));
    }
}
