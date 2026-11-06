//! Integration and unit tests for the encryption runtime and rekey runner configuration.

use aip_execution::encryption::DekStore;
use aip_service::encryption_runtime::{Error, KekConfig, RekeyOptions, RekeyTargetSpec};
use std::path::PathBuf;

#[tokio::test]
async fn public_execute_rekey_rejects_incomplete_inventory_and_closed_operational_gate() {
    let database_url = std::env::var("AIP_TEST_DATABASE_URL").expect("CI-provided isolated test database URL");
    let (mut db, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls).await.expect("connect test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let (mut budget, budget_connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls).await.expect("connect budget test database");
    tokio::spawn(async move {
        let _ = budget_connection.await;
    });
    let config = KekConfig::Spec { spec: "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".into(), active: "k1".into() };
    let mut options = RekeyOptions {
        scope_kind: "service".into(),
        scope_id: "global".into(),
        holder_id: "worker-1".into(),
        targets: vec![RekeyTargetSpec {
            table: "secret".into(),
            column: "value".into(),
            id_column: "id".into(),
            resource: "Secret".into(),
            field: "value".into(),
        }],
        inventory_complete: false,
    };

    let error = aip_service::encryption_runtime::execute_rekey(&mut db, &mut budget, "public", &config, &options)
        .await
        .expect_err("public execute_rekey must reject an incomplete inventory");
    assert!(matches!(error, Error::InvalidConfig(message) if message.contains("inventory_complete")));

    options.inventory_complete = true;

    let error = aip_service::encryption_runtime::execute_rekey(&mut db, &mut budget, "public", &config, &options)
        .await
        .expect_err("public execute_rekey must enforce the closed operational gate");

    assert_eq!(error, Error::OperationalGateClosed);
}

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
fn kek_spec_debug_and_serialization_never_leak_key_material() {
    let raw_key = "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    let config = KekConfig::Spec { spec: raw_key.to_string(), active: "k1".to_string() };

    // Debug representation must redact raw key spec
    let debug_output = format!("{config:?}");
    assert!(!debug_output.contains("BwcH"), "KekConfig Debug must redact key spec: {debug_output}");
    assert!(debug_output.contains("[REDACTED]"), "KekConfig Debug must indicate redaction: {debug_output}");

    // Serialization must redact raw key spec
    let json_output = serde_json::to_string(&config).expect("serialize config");
    assert!(!json_output.contains("BwcH"), "KekConfig serialization must redact key spec: {json_output}");
    assert!(json_output.contains("[REDACTED]"), "KekConfig serialization must indicate redaction: {json_output}");
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
fn kek_config_rejects_unknown_fields_and_missing_fields() {
    // Unknown field in env variant
    let json_unknown = r#"{"kind":"env","name":"AIP_KEK","active":"k1","extra_field":"disallowed"}"#;
    let res: Result<KekConfig, _> = serde_json::from_str(json_unknown);
    assert!(res.is_err(), "unknown field in KekConfig must fail deserialization");

    // Unknown field in spec variant
    let json_unknown_spec = r#"{"kind":"spec","spec":"k1:AAAA","active":"k1","leak":"payload"}"#;
    let res_spec: Result<KekConfig, _> = serde_json::from_str(json_unknown_spec);
    assert!(res_spec.is_err(), "unknown field in KekConfig::Spec must fail deserialization");

    // Missing field in env variant (missing active)
    let json_missing = r#"{"kind":"env","name":"AIP_KEK"}"#;
    let res_missing: Result<KekConfig, _> = serde_json::from_str(json_missing);
    assert!(res_missing.is_err(), "missing field in KekConfig must fail deserialization");
}

#[test]
fn kek_config_empty_or_blank_fields_fail_validation() {
    // Blank env name
    let config_blank_name = KekConfig::Env { name: "   ".to_string(), active: "k1".to_string() };
    let err = config_blank_name.validate().expect_err("blank env name must fail");
    assert_eq!(err, Error::InvalidConfig("KEK env 이름이 비어 있음".into()));

    // Blank active key
    let config_blank_active = KekConfig::Env { name: "AIP_KEK".to_string(), active: "".to_string() };
    let err = config_blank_active.validate().expect_err("blank active key must fail");
    assert_eq!(err, Error::InvalidConfig("active 키 식별자가 비어 있음".into()));

    // Blank file path
    let config_blank_file = KekConfig::File { path: PathBuf::new(), active: "k1".to_string() };
    let err = config_blank_file.validate().expect_err("empty file path must fail");
    assert_eq!(err, Error::InvalidConfig("KEK 파일 경로가 비어 있음".into()));

    // Blank spec
    let config_blank_spec = KekConfig::Spec { spec: "  ".to_string(), active: "k1".to_string() };
    let err = config_blank_spec.validate().expect_err("blank spec must fail");
    assert_eq!(err, Error::InvalidConfig("KEK spec이 비어 있음".into()));
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
        inventory_complete: true,
    };

    let json = serde_json::to_string(&options).unwrap();
    let deserialized: RekeyOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, options);

    // Call actual validation function
    let scope = deserialized.validate().expect("valid options must pass validation");
    assert_eq!(scope.kind(), "service");
    assert_eq!(scope.id(), "global");
}

#[test]
fn rekey_options_empty_targets_rejected_by_validator() {
    let options = RekeyOptions {
        scope_kind: "service".to_string(),
        scope_id: "global".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![],
        inventory_complete: false,
    };
    // Call actual validation function to verify rejection before any DB connection
    let err = options.validate().expect_err("empty targets must fail validation");
    assert_eq!(err, Error::InvalidConfig("rekey target 목록이 비어 있음".into()));
}

#[test]
fn rekey_options_invalid_scope_kind_rejected_by_validator() {
    let options = RekeyOptions {
        scope_kind: "unsupported_scope".to_string(),
        scope_id: "123".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![RekeyTargetSpec {
            table: "secret".to_string(),
            column: "value".to_string(),
            id_column: "id".to_string(),
            resource: "Secret".to_string(),
            field: "value".to_string(),
        }],
        inventory_complete: false,
    };
    let err = options.validate().expect_err("unsupported scope kind must fail validation");
    assert!(matches!(err, Error::InvalidConfig(msg) if msg.contains("알 수 없는 scope_kind")));
}

#[test]
fn rekey_options_blank_holder_id_rejected_by_validator() {
    let options = RekeyOptions {
        scope_kind: "service".to_string(),
        scope_id: "global".to_string(),
        holder_id: "   ".to_string(),
        targets: vec![RekeyTargetSpec {
            table: "secret".to_string(),
            column: "value".to_string(),
            id_column: "id".to_string(),
            resource: "Secret".to_string(),
            field: "value".to_string(),
        }],
        inventory_complete: false,
    };
    let err = options.validate().expect_err("blank holder_id must fail validation");
    assert_eq!(err, Error::InvalidConfig("holder_id 누락".into()));
}

#[test]
fn rekey_options_missing_target_fields_rejected_by_validator() {
    let options = RekeyOptions {
        scope_kind: "service".to_string(),
        scope_id: "global".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![RekeyTargetSpec {
            table: "".to_string(), // missing table
            column: "value".to_string(),
            id_column: "id".to_string(),
            resource: "Secret".to_string(),
            field: "value".to_string(),
        }],
        inventory_complete: false,
    };
    let err = options.validate().expect_err("missing target table must fail validation");
    assert!(matches!(err, Error::InvalidConfig(msg) if msg.contains("target[0] 필드 누락")));
}

#[test]
fn rekey_options_rejects_unknown_fields() {
    // Unknown field in RekeyOptions
    let json_options = r#"{
        "scope_kind": "service",
        "scope_id": "global",
        "holder_id": "worker-1",
        "targets": [],
        "unexpected_flag": true
    }"#;
    let res: Result<RekeyOptions, _> = serde_json::from_str(json_options);
    assert!(res.is_err(), "unknown field in RekeyOptions must fail deserialization");

    // Unknown field in RekeyTargetSpec
    let json_target = r#"{
        "table": "secret",
        "column": "value",
        "id_column": "id",
        "resource": "Secret",
        "field": "value",
        "unknown_column_attribute": "disallowed"
    }"#;
    let res_target: Result<RekeyTargetSpec, _> = serde_json::from_str(json_target);
    assert!(res_target.is_err(), "unknown field in RekeyTargetSpec must fail deserialization");
}

#[test]
fn rekey_options_partial_inventory_rejected_before_db_mutation() {
    let options = RekeyOptions {
        scope_kind: "service".to_string(),
        scope_id: "global".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![RekeyTargetSpec {
            table: "secret".to_string(),
            column: "value".to_string(),
            id_column: "id".to_string(),
            resource: "Secret".to_string(),
            field: "value".to_string(),
        }],
        inventory_complete: false, // partial target slice!
    };
    let err = options.assert_complete_inventory().expect_err("partial inventory must fail before DB changes");
    assert!(matches!(err, Error::InvalidConfig(msg) if msg.contains("전체 대상 인벤토리 보증")));
}

#[test]
fn rekey_options_complete_inventory_accepted_by_assertions() {
    let options = RekeyOptions {
        scope_kind: "service".to_string(),
        scope_id: "global".to_string(),
        holder_id: "worker-1".to_string(),
        targets: vec![RekeyTargetSpec {
            table: "secret".to_string(),
            column: "value".to_string(),
            id_column: "id".to_string(),
            resource: "Secret".to_string(),
            field: "value".to_string(),
        }],
        inventory_complete: true,
    };
    assert!(options.assert_complete_inventory().is_ok());
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
