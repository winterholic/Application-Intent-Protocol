use std::process::Command;

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
    assert!(py_check.status.success(), "python import failed:\nstdout: {}\nstderr: {}", String::from_utf8_lossy(&py_check.stdout), String::from_utf8_lossy(&py_check.stderr));
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
    assert!(py_check.status.success(), "python import failed:\nstdout: {}\nstderr: {}", String::from_utf8_lossy(&py_check.stdout), String::from_utf8_lossy(&py_check.stderr));
    assert!(String::from_utf8_lossy(&py_check.stdout).contains("OPS_IMPORT_OK"));
}
