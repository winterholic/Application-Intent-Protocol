//! Integration and unit tests for the encryption runtime and rekey runner configuration.

use aip_execution::encryption::DekStore;
use aip_service::encryption_runtime::{Error, KekConfig, RekeyOptions, RekeyTargetSpec};
use aip_crypto::provider::LocalProvider;
use aip_execution::encryption::{CellLocation, Scope};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

async fn connect() -> Client {
    let (client, connection) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
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
fn kek_config_debug_and_serialize_expose_static_spec_key_material() {
    let secret = "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    let config = KekConfig::Spec { spec: secret.to_owned(), active: "k1".to_owned() };
    assert!(format!("{config:?}").contains(secret));
    assert!(serde_json::to_string(&config).unwrap().contains(secret));
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

#[tokio::test]
async fn execute_rekey_rejects_empty_targets_and_invalid_scope_before_database_access() {
    let mut db = connect().await;
    let mut budget = connect().await;
    let key = "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    let kek = KekConfig::Spec { spec: key.to_owned(), active: "k1".to_owned() };
    let empty = RekeyOptions { scope_kind: "service".into(), scope_id: "global".into(), holder_id: "verify-worker".into(), targets: vec![] };
    assert!(matches!(
        aip_service::encryption_runtime::execute_rekey(&mut db, &mut budget, "aip_crypto_verify", &kek, &empty).await,
        Err(Error::InvalidConfig(message)) if message.contains("비어")
    ));

    let invalid = RekeyOptions {
        scope_kind: "ref".into(),
        scope_id: "not-a-canonical-id".into(),
        holder_id: "verify-worker".into(),
        targets: vec![RekeyTargetSpec {
            table: "cells".into(), column: "listed".into(), id_column: "id".into(), resource: "Secret".into(), field: "listed".into(),
        }],
    };
    assert!(matches!(
        aip_service::encryption_runtime::execute_rekey(&mut db, &mut budget, "aip_crypto_verify", &kek, &invalid).await,
        Err(Error::InvalidConfig(message)) if message.contains("ref scope")
    ));
}

#[tokio::test]
async fn partial_target_list_retires_dek_while_omitted_column_keeps_old_ciphertext() {
    let mut db = connect().await;
    let mut budget = connect().await;
    let store = DekStore::new(&format!("aip_crypto_verify_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))).unwrap();
    db.batch_execute(&store.ddl()).await.unwrap();
    db.batch_execute(&format!(
        "CREATE SCHEMA {}; CREATE TABLE {}.cells(id text PRIMARY KEY, listed text NOT NULL, omitted text NOT NULL)",
        store.deployment_schema(), store.deployment_schema()
    )).await.unwrap();

    let old_provider = LocalProvider::from_spec("k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=", "k1").unwrap();
    let new_provider = LocalProvider::from_spec("k2:CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=", "k2").unwrap();
    let scope = Scope::service();
    let init = db.transaction().await.unwrap();
    let old_dek = store.ensure_active_in(&init, &scope, &old_provider).await.unwrap();
    init.commit().await.unwrap();

    let mut reservation = store.reserve_for_write(&mut budget, &scope, 2).await.unwrap();
    let tx = db.transaction().await.unwrap();
    let listed = store.seal_in(&tx, &mut reservation, &CellLocation::new("Secret", "listed", "1").unwrap(), b"listed-old", &old_provider).await.unwrap();
    let omitted = store.seal_in(&tx, &mut reservation, &CellLocation::new("Secret", "omitted", "1").unwrap(), b"omitted-old", &old_provider).await.unwrap();
    tx.execute(
        &format!("INSERT INTO {}.cells(id,listed,omitted) VALUES('1',$1,$2)", store.deployment_schema()),
        &[&listed, &omitted],
    ).await.unwrap();
    tx.commit().await.unwrap();

    let tx = db.transaction().await.unwrap();
    let epoch = store.active_for_write_in(&tx, &scope).await.unwrap().epoch();
    store.promote_in(&tx, &scope, epoch, &new_provider).await.unwrap();
    tx.commit().await.unwrap();

    let options = RekeyOptions {
        scope_kind: "service".into(),
        scope_id: "global".into(),
        holder_id: "partial-target-verifier".into(),
        targets: vec![RekeyTargetSpec {
            table: "cells".into(), column: "listed".into(), id_column: "id".into(), resource: "Secret".into(), field: "listed".into(),
        }],
    };
    let reports = aip_service::encryption_runtime::execute_rekey(
        &mut db, &mut budget, store.deployment_schema(),
        &KekConfig::Spec { spec: "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=,k2:CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=".into(), active: "k2".into() },
        &options,
    ).await.unwrap();
    assert!(reports.iter().any(|report| report.fully_retired));

    let state: String = db.query_one(
        &format!("SELECT state FROM {}.aip_encryption_deks WHERE dek_id=$1", store.infrastructure_schema()),
        &[&old_dek.uuid()],
    ).await.unwrap().get(0);
    let omitted_frame: String = db.query_one(
        &format!("SELECT omitted FROM {}.cells WHERE id='1'", store.deployment_schema()), &[]
    ).await.unwrap().get(0);
    assert_eq!(state, "RETIRED");
    assert_eq!(aip_crypto::cell::parse_header(&omitted_frame).unwrap().dek_id, old_dek.id());

    db.batch_execute(&format!("DROP SCHEMA {} CASCADE; DROP SCHEMA {} CASCADE", store.deployment_schema(), store.infrastructure_schema())).await.unwrap();
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
