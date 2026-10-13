//! Delegated decision CD-6 (2026-10-13): the generated contract carries the exposed creates and composes, with the same
//! size caps the server enforces, and stays silent (so existing contract fingerprints do not move) when a definition
//! exposes neither.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_ts_with_apply};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Project {
  fields { id: Id; owner: Member; name: Text(1..50); archived: Bool }
  rows read when owner = actor
  transition archive { allow owner = actor; from archived = false; to archived = true }
  expose apply archive { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 2; transitions archive; create Log from owner, name }
}
resource Log {
  fields { id: Id; owner: Member; name: Text(1..50); note: Text? }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, name, note }
}
resource Link {
  fields { project: Project; member: Member }
  rows read when member = actor
  expose create { allow member = actor; fields project, member }
}
"#;

fn facts(spec: &str) -> serde_json::Value {
    load_str(spec, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

#[test]
fn creates_and_composes_are_typed_in_the_contract() {
    let ts = contract_ts_with_apply(&facts(SPEC), IdWire::DecimalString);
    for expected in [
        "export const bundleLimits = { maxOps: 20, maxRows: 1000, maxSteps: 10 } as const;",
        "export interface CreateContract {",
        "\"Log\": { idOutput: Id<\"Log\">; fields: { name: string; note: string | null; owner: Id<\"Member\">; }; optional: \"note\" };",
        // a table without an id column cannot hand an id back
        "\"Link\": { idOutput: null;",
        "export interface ComposeContract {",
        "\"Project\": { idInput: Id<\"Project\">; idOutput: Id<\"Project\">; maxRows: 2; transitions: \"archive\"; self: false;",
        "optional: \"note\"; sources: { \"owner\": Id<\"Member\">; \"name\": string; }; constants: \"name\" | \"note\" }",
    ] {
        assert!(ts.contains(expected), "missing `{expected}` in:\n{ts}");
    }
    let legacy = contract_ts_with_apply(&facts(SPEC), IdWire::Legacy);
    assert!(legacy.contains("fields: { name: string; note: string | null; owner: string; }"), "{legacy}");
}

#[test]
fn a_definition_without_create_or_compose_keeps_its_fingerprint() {
    let plain = "resource Member { fields { id: Id } }\nactor Member\nresource Box {\n  fields { id: Id; owner: Member; open: Bool }\n  rows read when owner = actor\n  transition close { allow owner = actor; from open = true; to open = false }\n  expose apply close { target id; bulk maxRows 3 }\n}\n";
    let ts = contract_ts_with_apply(&facts(plain), IdWire::DecimalString);
    assert!(!ts.contains("CreateContract") && !ts.contains("ComposeContract") && !ts.contains("bundleLimits"), "{ts}");
    let with = contract_fingerprint_with_apply(&facts(SPEC), IdWire::DecimalString);
    assert_ne!(with, contract_fingerprint_with_apply(&facts(plain), IdWire::DecimalString));
}
