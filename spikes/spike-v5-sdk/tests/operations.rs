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

/// The values a client sees in `JobRun.state`. They are fixed here on purpose: the server's compiled `STATES` arrays are pinned to this
/// same list by unit tests in `crates/aip-execution/src/jobs.rs` and `crates/aip-migrate/src/mysql_jobs.rs`, so the generated Python
/// type, this list and both servers cannot drift apart without one of those tests failing. Nothing here parses Rust source text.
const WIRE_JOB_STATES: [&str; 6] = ["QUEUED", "RUNNING", "CANCEL_REQUESTED", "SUCCEEDED", "FAILED", "CANCELLED"];

#[test]
fn python_job_states_match_the_fixed_wire_values() {
    let facts = load_str(include_str!("../../spike-v6-transport/tests/fixtures/operations.aip"), Form::A).unwrap().execution;
    let python_code = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);

    let declaration = python_code.lines().find(|line| line.starts_with("JobState = Literal[")).expect("generated Python must declare JobState");
    let members = declaration.strip_prefix("JobState = Literal[").and_then(|rest| rest.strip_suffix(']')).expect("JobState is a Literal[...]");
    let generated: Vec<&str> = members.split(", ").map(|member| member.trim_matches('"')).collect();

    let generated_set: std::collections::BTreeSet<&str> = generated.iter().copied().collect();
    assert_eq!(generated_set.len(), generated.len(), "JobState must not repeat a state: {declaration}");
    assert_eq!(generated_set, WIRE_JOB_STATES.into_iter().collect(), "generated Python JobState differs from the wire states: {declaration}");
    assert_eq!(generated, WIRE_JOB_STATES, "state order is part of the generated text");
    // The server stores a queued job as the plain string QUEUED; a constant name must never reach the wire.
    assert!(generated.contains(&"QUEUED") && generated.iter().all(|state| !state.starts_with("JOB_STATE_")), "{declaration}");
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
