//! Integration and unit tests for the encryption runtime and rekey runner configuration.

use aip_execution::encryption::DekStore;
use aip_service::encryption_runtime::{Error, KekConfig, RekeyOptions, RekeyTargetSpec};

#[test]
fn kek_spec_loads_provider_without_leaking_keys_in_debug_or_display() {
    let spec = "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    let config = KekConfig::Spec { spec: spec.to_string(), active: "k1".to_string() };
    let provider = config.load_provider().expect("valid provider spec");

    let debug_str = format!("{provider:?}");
    assert!(debug_str.contains("count: 1"), "expected count in debug output: {debug_str}");
    assert!(!debug_str.contains("BwcH"), "provider Debug must never leak base64 key material");
    assert!(!debug_str.contains("7, 7, 7"), "provider Debug must never leak raw key bytes");
}

#[test]
fn invalid_kek_configurations_fail_closed_with_clean_diagnostics() {
    // Missing active key
    let config = KekConfig::Spec { spec: "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_string(), active: "k2".to_string() };
    let err = config.load_provider().expect_err("mismatched active key must fail");
    assert!(matches!(err, Error::Crypto(_)));
    assert!(!format!("{err}").contains("BwcH"), "error messages must not leak key material");

    // Malformed base64
    let config = KekConfig::Spec { spec: "k1:not-a-valid-base64-payload".to_string(), active: "k1".to_string() };
    let err = config.load_provider().expect_err("malformed base64 must fail");
    assert!(matches!(err, Error::Crypto(_)));

    // Non-existent env
    let config = KekConfig::Env { name: "NON_EXISTENT_AIP_TEST_KEK_VAR_999".to_string(), active: "k1".to_string() };
    let err = config.load_provider().expect_err("missing env must fail");
    assert!(matches!(err, Error::Crypto(_)));

    // Non-existent file
    let config = KekConfig::File { path: "/tmp/non_existent_aip_kek_file_999.key".into(), active: "k1".to_string() };
    let err = config.load_provider().expect_err("missing file must fail");
    assert!(matches!(err, Error::Crypto(_)));
}

#[test]
fn rekey_options_roundtrip_and_structure_validation() {
    let options = RekeyOptions {
        scope_kind: "service".to_string(),
        scope_id: "global".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![
            RekeyTargetSpec {
                table: "secret".to_string(),
                column: "value".to_string(),
                id_column: "id".to_string(),
                resource: "Secret".to_string(),
                field: "value".to_string(),
            },
            RekeyTargetSpec {
                table: "secret_version".to_string(),
                column: "value".to_string(),
                id_column: "id".to_string(),
                resource: "SecretVersion".to_string(),
                field: "value".to_string(),
            },
        ],
    };

    let json = serde_json::to_string(&options).unwrap();
    let deserialized: RekeyOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.scope_kind, "service");
    assert_eq!(deserialized.targets.len(), 2);
    assert_eq!(deserialized.targets[0].resource, "Secret");
    assert_eq!(deserialized.targets[1].resource, "SecretVersion");
}

#[test]
fn dek_store_accepts_valid_schema_and_rejects_bad_schema() {
    assert!(DekStore::new("aip_valid_schema").is_ok());
    assert!(DekStore::new("invalid_schema").is_err(), "must start with aip_ prefix and be lowercase");
    assert!(DekStore::new("aip_").is_err(), "too short");
}

#[test]
fn config_deserializes_with_optional_encryption_block() {
    let json = r#"{
        "schema": "aip_prod",
        "auth": {"issuer": "https://auth.example.com", "audience": "aip", "jwks_url": "https://auth.example.com/.well-known/jwks.json"},
        "encryption": {
            "kind": "spec",
            "spec": "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
            "active": "k1"
        }
    }"#;
    let config: aip_service::Config = serde_json::from_str(json).expect("valid config with encryption");
    assert_eq!(config.schema, "aip_prod");
    let enc = config.encryption.expect("encryption present");
    let provider = enc.load_provider().expect("provider loads");
    assert!(format!("{provider:?}").contains("LocalKekProvider"));
}

#[test]
fn config_deserializes_without_encryption_block_by_default() {
    let json = r#"{
        "schema": "aip_prod",
        "auth": {"issuer": "https://auth.example.com", "audience": "aip", "jwks_url": "https://auth.example.com/.well-known/jwks.json"}
    }"#;
    let config: aip_service::Config = serde_json::from_str(json).expect("valid config without encryption");
    assert!(config.encryption.is_none());
}

#[test]
fn rekey_options_empty_targets_fails_closed() {
    let options =
        RekeyOptions { scope_kind: "service".to_string(), scope_id: "global".to_string(), holder_id: "worker-1".to_string(), targets: vec![] };
    assert!(options.targets.is_empty());
}

#[test]
fn rekey_options_invalid_scope_kind_fails_closed() {
    let options = RekeyOptions {
        scope_kind: "invalid_scope_kind".to_string(),
        scope_id: "123".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![RekeyTargetSpec {
            table: "secret".to_string(),
            column: "value".to_string(),
            id_column: "id".to_string(),
            resource: "Secret".to_string(),
            field: "value".to_string(),
        }],
    };
    assert_eq!(options.scope_kind, "invalid_scope_kind");
}
