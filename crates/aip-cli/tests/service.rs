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
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let out_path = tmp.path().to_str().unwrap();
    let output =
        Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "gen", definition, "--lang", "python", "--out", out_path]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let content = std::fs::read_to_string(out_path).unwrap();
    assert!(content.contains("CONTRACT_FINGERPRINT"), "{content}");
    assert!(content.contains("ReadResult"), "{content}");
    assert!(content.contains("ApplyInput"), "{content}");
    assert!(content.contains("JobState"), "{content}");

    let py_check =
        Command::new("python3").args(["-c", "import sys; compile(open(sys.argv[1]).read(), sys.argv[1], 'exec')", out_path]).output().unwrap();
    assert!(py_check.status.success(), "{}", String::from_utf8_lossy(&py_check.stderr));
}
