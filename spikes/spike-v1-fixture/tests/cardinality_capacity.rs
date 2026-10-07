use serde_json::json;
use spike_v1_fixture::{load_str, Form};

const BASE: &str = include_str!("../fixture/recruitment.aip");

#[test]
fn per_reference_at_most_two_is_a_supported_server_invariant() {
    let source = BASE.replacen("atMost 1 where status = PUBLISHED", "atMost 2 where status = PUBLISHED", 1);
    let output = load_str(&source, Form::A).unwrap_or_else(|diagnostics| panic!("{diagnostics:?}"));
    let enforcement = &output.execution["resources"]["Recruitment"]["invariants"]["atMostOnePublished"]["enforcement"];

    assert_eq!(enforcement["kind"], json!("lockedCountCheck"));
    assert_eq!(enforcement["max"], json!(2));
}
