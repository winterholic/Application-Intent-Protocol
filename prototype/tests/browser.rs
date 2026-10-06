use serde_json::Value;
use std::process::{Command, Output};

const APP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../spikes/spike-v1-fixture/fixture/recruitment.aip");
const UNREACHABLE_DB: &str = "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1";

fn serve_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aip-prototype"));
    command.args(["serve", APP, "--schema", "aip_browser_origin_test", "--db-url", UNREACHABLE_DB, "--listen", "127.0.0.1:0", "--wire", "safe"]);
    command
}

fn result(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr)
        .unwrap_or_else(|error| panic!("stderr must be JSON, got {:?}: {error}", String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn invalid_allow_origins_are_rejected_before_database_access() {
    let invalid_origins =
        ["*", "null", "http://example.com", "http://localhost/path", "http://localhost:", "http://localhost:5173\r\nx-injected: value"];

    for origin in invalid_origins {
        let output = serve_command().arg("--allow-origin").arg(origin).output().expect("run serve CLI");
        assert!(!output.status.success(), "invalid origin unexpectedly started: {origin:?}");
        assert_eq!(result(&output)["code"], "BAD_ORIGIN", "origin {origin:?} must fail before the unreachable DB");
    }
}

#[test]
fn repeated_valid_allow_origins_reach_database_validation() {
    let output =
        serve_command().args(["--allow-origin", "http://localhost:5173", "--allow-origin", "http://127.0.0.1:5173"]).output().expect("run serve CLI");
    assert!(!output.status.success(), "unreachable database must stop server startup");
    assert_eq!(result(&output)["code"], "DB_CONNECT", "valid repeated origins should pass origin validation before DB access");
}

#[test]
fn serve_help_exposes_allow_origin() {
    let output = Command::new(env!("CARGO_BIN_EXE_aip-prototype")).args(["serve", "--help"]).output().expect("run serve help");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--allow-origin"));
}
