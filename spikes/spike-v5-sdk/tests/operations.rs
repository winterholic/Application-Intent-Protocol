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

#[test]
fn python_job_states_match_server_execution_states_exactly() {
    let jobs_source = include_str!("../../../crates/aip-execution/src/jobs.rs");
    let state_line = jobs_source
        .lines()
        .find(|line| line.contains("const STATES:"))
        .expect("crates/aip-execution/src/jobs.rs must define STATES");
    let (_, array_part) = state_line.split_once('=').expect("should have = in STATES assignment");
    let bracket_start = array_part.find('[').unwrap();
    let bracket_end = array_part.rfind(']').unwrap();
    let inner = &array_part[bracket_start + 1..bracket_end];
    let server_states: Vec<&str> = inner
        .split(',')
        .map(|s| s.trim().trim_matches('"'))
        .filter(|s| !s.is_empty())
        .collect();

    assert_eq!(server_states, vec!["QUEUED", "RUNNING", "CANCEL_REQUESTED", "SUCCEEDED", "FAILED", "CANCELLED"]);

    let facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    let python_code = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);

    let expected_job_state = format!(
        "JobState = Literal[{}]",
        server_states.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ")
    );
    assert!(
        python_code.contains(&expected_job_state),
        "Generated python JobState must match server execution states exactly:\nExpected: {expected_job_state}\nFound:\n{python_code}"
    );
}

#[test]
fn python_codegen_handles_principal_id_and_generates_principal_id_newtype() {
    let facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    let code = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
    assert!(code.contains("principalId = NewType(\"principalId\", str)"), "Must define principalId NewType:\n{code}");
    assert!(code.contains("\"owner\": principalId"), "LengthOperationInput must reference principalId:\n{code}");
}

#[test]
#[should_panic(expected = "unsupported or unknown type")]
fn python_codegen_panics_on_unknown_type() {
    let mut facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    facts["operations"]["length"]["input"][0][1] = serde_json::json!("UnknownCustomType");
    let _ = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
}

#[test]
#[should_panic(expected = "unsupported or unknown resource reference")]
fn python_codegen_panics_on_unknown_resource_ref() {
    let mut facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    facts["operations"]["length"]["input"][0][1] = serde_json::json!("Id<NonExistentResource>");
    let _ = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
}

#[test]
#[should_panic(expected = "unknown enum")]
fn python_codegen_panics_on_unknown_enum() {
    let mut facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    facts["operations"]["length"]["input"][0][1] = serde_json::json!("Enum<NonExistentEnum>");
    let _ = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
}

#[test]
#[should_panic(expected = "unsupported reference to principal without principal actor")]
fn python_codegen_panics_on_principal_ref_without_principal_actor() {
    let mut facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    facts["actor"] = serde_json::json!("none");
    facts["actorMode"] = serde_json::json!("none");
    facts["operations"]["length"]["input"][0][1] = serde_json::json!("Ref<principal>");
    let _ = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
}
