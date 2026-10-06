use serde_json::{Value, json};
use std::{path::Path, process::Command};

const APP: &str = include_str!("../../../prototype/example/app.aip");

fn fail(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_aip")).arg("service").args(args).output().unwrap();
    assert!(!output.status.success(), "must reject invalid product input");
    serde_json::from_slice(&output.stderr).unwrap()
}
fn config(path: &Path, extra: Value) {
    let mut value = json!({"schema":"aip_boundary","database_url_env":"AIP_BOUNDARY_UNUSED_ENV","auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}});
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    std::fs::write(path, value.to_string()).unwrap();
}

#[test]
fn product_config_rejects_ignored_fields_and_public_http_binding() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("service.json");
    let file = dir.path().join("app.aip");
    std::fs::write(&file, APP).unwrap();
    config(&settings, json!({"dev_actor":42}));
    assert_eq!(fail(&["init", file.to_str().unwrap(), "--config", settings.to_str().unwrap()])["code"], "BAD_CONFIG");
    config(&settings, json!({"listen":"0.0.0.0:8080"}));
    assert_eq!(fail(&["serve", file.to_str().unwrap(), "--config", settings.to_str().unwrap()])["code"], "BAD_LISTEN");
    config(&settings, json!({"allowed_origins":["https://app.example.com/path"]}));
    assert_eq!(fail(&["serve", file.to_str().unwrap(), "--config", settings.to_str().unwrap()])["code"], "BAD_ORIGIN");
}

#[test]
fn product_reads_relative_database_ca_before_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("service.json");
    let file = dir.path().join("app.aip");
    std::fs::write(&file, APP).unwrap();
    config(&settings, json!({"database_ca_file":"private-db-ca.pem"}));
    std::fs::write(dir.path().join("private-db-ca.pem"), "invalid certificate").unwrap();
    assert_eq!(fail(&["init", file.to_str().unwrap(), "--config", settings.to_str().unwrap()])["code"], "DB_CA_CONFIG");
}

#[test]
fn product_rejects_oversized_definition_before_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.aip");
    std::fs::write(&file, vec![b' '; (1 << 20) + 1]).unwrap();
    assert_eq!(fail(&["check", file.to_str().unwrap()])["code"], "INPUT_TOO_LARGE");
}

#[test]
fn product_generator_preserves_definition_when_output_aliases_source() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.aip");
    let alias = dir.path().join("binding.ts");
    std::fs::write(&file, APP).unwrap();
    std::fs::hard_link(&file, &alias).unwrap();
    assert_eq!(fail(&["gen", file.to_str().unwrap(), "--out", alias.to_str().unwrap()])["code"], "OUTPUT_CONFLICT");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), APP);
}

#[cfg(unix)]
#[tokio::test]
async fn product_file_inputs_reject_unconnected_fifos_without_waiting_for_a_writer() {
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("input.aip");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let settings = dir.path().join("service.json");
    let ca_settings = dir.path().join("service-ca.json");
    let file = dir.path().join("app.aip");
    config(&settings, json!({}));
    config(&ca_settings, json!({"database_ca_file":fifo}));
    std::fs::write(&file, APP).unwrap();

    let cases = [
        (vec!["check", fifo.to_str().unwrap()], "SOURCE_IO"),
        (vec!["init", file.to_str().unwrap(), "--config", fifo.to_str().unwrap()], "CONFIG_IO"),
        (vec!["migrate", file.to_str().unwrap(), "--config", settings.to_str().unwrap(), "--migration", fifo.to_str().unwrap()], "MIGRATION_IO"),
        (vec!["init", file.to_str().unwrap(), "--config", ca_settings.to_str().unwrap()], "DB_CA_IO"),
    ];
    for (args, code) in cases {
        let output = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_aip")).arg("service").args(&args).kill_on_drop(true).output(),
        )
        .await
        .unwrap_or_else(|_| panic!("{args:?} waited for a FIFO writer"))
        .unwrap();
        assert!(!output.status.success());
        let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(diagnostic["code"], code);
    }
}
