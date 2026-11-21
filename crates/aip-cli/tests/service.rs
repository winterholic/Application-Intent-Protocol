#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;
use std::{collections::BTreeSet, process::Command};

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn product_cli_checks_the_caller_definition() {
    let definition = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prototype/example/app.aip");
    let output = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "check", definition]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["executionDigest"].as_str().unwrap().len(), 64);
}

#[test]
fn service_check_json_keeps_success_and_failure_diagnostic_contracts() {
    let definition = root().join("spikes/spike-v1-fixture/fixture/recruitment.aip");
    let success = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "check", definition.to_str().unwrap()]).output().unwrap();
    assert_eq!(success.status.code(), Some(0));
    let checked: Value = serde_json::from_slice(&success.stdout).unwrap();
    let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>();
    let key_set = |names: &[&str]| names.iter().map(|name| (*name).to_string()).collect::<BTreeSet<_>>();
    assert_eq!(keys(&checked), key_set(&["advisories", "executionDigest", "ok", "workers"]));
    assert_eq!(checked["ok"].as_bool(), Some(true));
    assert_eq!(checked["executionDigest"].as_str().map(str::len), Some(64));
    let advisories = checked["advisories"].as_array().unwrap();
    assert_eq!(advisories.len(), 1);
    assert_eq!(keys(&advisories[0]), key_set(&["anchor", "code", "message", "severity"]));
    assert_eq!(advisories[0]["severity"].as_str(), Some("warning"));
    assert_eq!(advisories[0]["code"].as_str(), Some("DEF.ADVISORY_PUBLIC_COUNT"));
    assert!(advisories[0]["anchor"].as_str().is_some_and(|v| v.starts_with("line:")));
    assert!(advisories[0]["message"].is_string());
    assert_eq!(checked["workers"]["required"].as_bool(), Some(true));
    assert!(checked["workers"]["extensions"].is_array());

    let dir = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(&definition).unwrap();
    let invalid = dir.path().join("invalid.aip");
    std::fs::write(&invalid, source.replacen("sort periodEnd, views, id", "sorts periodEnd, views, id", 1)).unwrap();
    let failure = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "check", invalid.to_str().unwrap()]).output().unwrap();
    assert_eq!(failure.status.code(), Some(1));
    assert!(failure.stdout.is_empty());
    let rejected: Value = serde_json::from_slice(&failure.stderr).unwrap();
    assert_eq!(keys(&rejected), key_set(&["code", "diagnostics", "ok"]));
    assert_eq!(rejected["ok"].as_bool(), Some(false));
    assert_eq!(rejected["code"].as_str(), Some("DEF.INVALID_DEFINITION"));
    let diagnostics = rejected["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(keys(diagnostic), key_set(&["code", "column", "explain", "help", "line", "message", "scope", "severity"]));
    assert_eq!(diagnostic["severity"].as_str(), Some("error"));
    assert_eq!(diagnostic["code"].as_str(), Some("DEF.PARSE_UNKNOWN_KEY"));
    assert_eq!(diagnostic["line"].as_u64(), Some(56));
    assert_eq!(diagnostic["column"].as_u64(), Some(5));
    for key in ["message", "scope", "help", "explain"] {
        assert!(diagnostic[key].is_string(), "{key} must remain a string: {diagnostic}");
    }
}

#[test]
fn product_serve_rejects_development_identity_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "serve", "--config", "unused.json", "--dev-actor", "1"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--dev-actor"));
}

#[test]
fn product_cli_generates_python_sdk_bindings() {
    let definition = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prototype/example/app.aip");
    let tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
    let out_path = tmp.path().to_str().unwrap();
    let output =
        Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "gen", definition, "--lang", "python", "--out", out_path]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let content = std::fs::read_to_string(out_path).unwrap();
    assert!(content.contains("CONTRACT_FINGERPRINT"), "{content}");
    assert!(content.contains("ReadResult"), "{content}");
    assert!(content.contains("ApplyInput"), "{content}");
    assert!(content.contains("JobState"), "{content}");

    let sdk_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
    let py_script = r#"
import importlib.util, sys, typing
sys.path.insert(0, sys.argv[1])
spec = importlib.util.spec_from_file_location('generated', sys.argv[2])
assert spec is not None and spec.loader is not None
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
assert hasattr(mod, 'CONTRACT_FINGERPRINT')
assert len(mod.CONTRACT_FINGERPRINT) == 64
assert hasattr(mod, 'ReadResult')
assert hasattr(mod, 'ApplyInput')
assert hasattr(mod, 'JobState')
assert typing.get_args(mod.JobState) == ('QUEUED', 'RUNNING', 'CANCEL_REQUESTED', 'SUCCEEDED', 'FAILED', 'CANCELLED')
print('CLI_IMPORT_OK')
"#;
    let py_check = Command::new("python3").args(["-c", py_script, sdk_path, out_path]).output().unwrap();
    assert!(
        py_check.status.success(),
        "python import failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&py_check.stdout),
        String::from_utf8_lossy(&py_check.stderr)
    );
    assert!(String::from_utf8_lossy(&py_check.stdout).contains("CLI_IMPORT_OK"));
}

#[test]
fn product_cli_generates_python_sdk_bindings_for_operations_with_principal_id() {
    let definition = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spikes/spike-v6-transport/tests/fixtures/operations.aip");
    let tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
    let out_path = tmp.path().to_str().unwrap();
    let output =
        Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "gen", definition, "--lang", "python", "--out", out_path]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let sdk_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
    let py_script = r#"
import importlib.util, sys, typing
sys.path.insert(0, sys.argv[1])
spec = importlib.util.spec_from_file_location('generated_ops', sys.argv[2])
assert spec is not None and spec.loader is not None
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
assert hasattr(mod, 'principalId'), 'operations.aip with actor principal must generate principalId'
assert hasattr(mod, 'LengthOperationInput')
assert mod.LengthOperationInput.__annotations__['owner'] == mod.principalId
assert typing.get_args(mod.JobState) == ('QUEUED', 'RUNNING', 'CANCEL_REQUESTED', 'SUCCEEDED', 'FAILED', 'CANCELLED')
print('OPS_IMPORT_OK')
"#;
    let py_check = Command::new("python3").args(["-c", py_script, sdk_path, out_path]).output().unwrap();
    assert!(
        py_check.status.success(),
        "python import failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&py_check.stdout),
        String::from_utf8_lossy(&py_check.stderr)
    );
    assert!(String::from_utf8_lossy(&py_check.stdout).contains("OPS_IMPORT_OK"));
}
