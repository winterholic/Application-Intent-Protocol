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
