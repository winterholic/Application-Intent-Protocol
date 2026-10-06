use spike_v2_read::id_wire::IdWire;
use std::{fs, process::Command};
const APP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../spikes/spike-v1-fixture/fixture/recruitment.aip");
#[test]
fn gen_uses_the_same_extension_contract_as_the_server() {
    let out = std::env::temp_dir().join(format!("aip-extension-cli-{}.ts", std::process::id()));
    assert!(!out.exists());
    let run = Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
        .args(["gen", APP, "--wire", "decimal", "--out", out.to_str().unwrap(), "--sdk-import", "@aip/prototype-sdk"])
        .output()
        .unwrap();
    assert!(run.status.success());
    let content = fs::read_to_string(&out).unwrap();
    fs::remove_file(&out).unwrap();
    assert!(content.contains("ExtensionBinding<Contract, ApplyContract, ExtensionContract>"));
    let facts = spike_v1_fixture::load_str(&fs::read_to_string(APP).unwrap(), spike_v1_fixture::Form::A).unwrap().execution;
    let event: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(event["fingerprint"], spike_v5_sdk::contract_fingerprint_with_extensions(&facts, IdWire::DecimalString));
}

#[test]
fn worker_configuration_is_explicit_and_checked_before_database_access() {
    let base = [
        "serve",
        APP,
        "--schema",
        "aip_extension_options",
        "--db-url",
        "host=127.0.0.1 port=1 dbname=postgres",
        "--listen",
        "127.0.0.1:0",
        "--wire",
        "safe",
    ];
    for option in [vec!["--worker-dir", "/missing/aip-extensions"], vec!["--worker-lang", "node"]] {
        let run = Command::new(env!("CARGO_BIN_EXE_aip-prototype")).args(base).args(option).output().unwrap();
        assert_eq!(run.status.code(), Some(2), "CLI must require both worker options");
    }
    let run = Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
        .args(base)
        .args(["--worker-dir", "/missing/aip-extensions", "--worker-lang", "python"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    let event: serde_json::Value = serde_json::from_slice(&run.stderr).unwrap();
    assert_eq!(event["code"], "WORKER_CONFIG", "invalid extension path precedes the failing database");
}
