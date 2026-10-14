use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_ts_with_apply};

const SOURCE: &str = "resource Member { fields { id: Id } } actor Member resource Note { fields { id: Id; owner: Member; title: Text(1..8); content: Text?; version: Int } rows read when owner = actor transition edit { from true; allow owner = actor; patch title, content; expect version; to version = version + 1 } expose apply edit { target id; bulk maxRows 1 } }";
#[test]
fn generated_patch_types_and_public_validation_change_the_contract() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    let ts = contract_ts_with_apply(&facts, IdWire::DecimalString);
    assert!(ts.contains("values: {"), "{ts}");
    assert!(ts.contains("content?: string | null;"), "{ts}");
    assert!(ts.contains("title?: string;"), "{ts}");
    assert!(ts.contains("expect: {"), "{ts}");
    assert!(ts.contains("version: number;"), "{ts}");
    let hash = contract_fingerprint_with_apply(&facts, IdWire::DecimalString);
    for source in [SOURCE.replace("1..8", "1..9"), SOURCE.replace("patch title, content", "patch title"), SOURCE.replace("expect version;", "")] {
        let changed = load_str(&source, Form::A).unwrap().execution;
        assert_ne!(hash, contract_fingerprint_with_apply(&changed, IdWire::DecimalString));
    }
    let changed = load_str(&SOURCE.replace("allow owner = actor", "allow false"), Form::A).unwrap().execution;
    assert_eq!(hash, contract_fingerprint_with_apply(&changed, IdWire::DecimalString));
}

#[test]
fn python_input_types_accept_definition_fields_that_are_python_keywords() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let facts = load_str(&SOURCE.replace("title", "class"), Form::A).unwrap().execution;
    let generated = spike_v5_sdk::contract_module_python(&facts, IdWire::DecimalString);
    let mut child = Command::new("python3")
        .args(["-c", "import sys; compile(sys.stdin.read(), '<binding>', 'exec')"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(generated.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}
