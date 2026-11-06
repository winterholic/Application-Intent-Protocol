//! Integration and unit tests for the encryption runtime and rekey runner configuration.

use aip_execution::encryption::DekStore;
use aip_service::encryption_runtime::{Error, KekConfig, RekeyOptions, RekeyTargetSpec};
use std::path::PathBuf;

#[tokio::test]
async fn public_execute_rekey_verifies_metadata_then_keeps_operational_gate_closed() {
    let (mut db, schema, mut options) = inventory_fixture(false, false).await;
    let database_url = std::env::var("AIP_TEST_DATABASE_URL").expect("CI-provided isolated test database URL");
    let (mut budget, budget_connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls).await.expect("connect budget test database");
    tokio::spawn(async move {
        let _ = budget_connection.await;
    });
    let config = KekConfig::Spec { spec: "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".into(), active: "k1".into() };
    options.inventory_complete = false;
    let error = aip_service::encryption_runtime::execute_rekey(&mut db, &mut budget, &schema, &config, &options)
        .await
        .expect_err("valid metadata inventory must still enforce the closed operational gate");

    assert_eq!(error, Error::OperationalGateClosed);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.expect("drop owned inventory fixture schema");
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
        scope_resource: None,
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
        scope_resource: None,
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
        scope_resource: None,
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
        scope_resource: None,
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
        scope_resource: None,
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

const INVENTORY_DEFINITION: &str = r#"
actor principal
resource Team { fields { id: Id; } }
resource Secret {
  fields {
    id: Id;
    team: Team;
    value: Text encrypted(scope: team);
    backup: Text encrypted(scope: team);
  }
}
resource SecretVersion {
  fields {
    id: Id;
    team: Team;
    encryptedValue: Text encrypted(scope: team);
  }
}
"#;

async fn inventory_fixture(omit_backup: bool, wrong_scope_fk: bool) -> (tokio_postgres::Client, String, RekeyOptions) {
    use serde_json::Value;
    let database_url = std::env::var("AIP_TEST_DATABASE_URL").expect("CI-provided isolated test database URL");
    let (db, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls).await.expect("connect test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let schema = format!("aip_rekey_inv_{}", uuid::Uuid::new_v4().simple());
    db.batch_execute(&format!("CREATE SCHEMA {schema}")).await.expect("create owned inventory fixture schema");
    db.batch_execute(&format!(
        "CREATE TABLE {schema}.aip_migrate_meta (singleton boolean PRIMARY KEY, facts jsonb NOT NULL, facts_digest text NOT NULL);
         CREATE TABLE {schema}.team (id bigint PRIMARY KEY);
         CREATE TABLE {schema}.project (id bigint PRIMARY KEY);"
    ))
    .await
    .expect("create inventory metadata and referenced resources");

    let facts =
        spike_v1_fixture::load_str(INVENTORY_DEFINITION, spike_v1_fixture::Form::A).expect("compile declared encrypted resource metadata").execution;
    let facts_value: Value = facts.clone();
    let digest = spike_v1_fixture::digest(&facts_value);
    db.execute(&format!("INSERT INTO {schema}.aip_migrate_meta(singleton, facts, facts_digest) VALUES (true, $1, $2)"), &[&facts_value, &digest])
        .await
        .expect("persist the declaration facts used by the deployment");

    let secret_scope = if wrong_scope_fk { "project" } else { "team" };
    let backup = if omit_backup { "" } else { ", backup text NOT NULL" };
    db.batch_execute(&format!(
        "CREATE TABLE {schema}.secret (
           id bigint PRIMARY KEY,
           team_id bigint NOT NULL REFERENCES {schema}.{secret_scope}(id),
           value text NOT NULL{backup}
         );
         CREATE TABLE {schema}.secret_version (
           id bigint PRIMARY KEY,
           team_id bigint NOT NULL REFERENCES {schema}.team(id),
           encrypted_value text NOT NULL
         );"
    ))
    .await
    .expect("create physical tables from the encrypted declarations");

    let options = RekeyOptions {
        scope_kind: "ref".into(),
        scope_id: "42".into(),
        scope_resource: Some("Team".into()),
        holder_id: "inventory-test-worker".into(),
        targets: vec![
            RekeyTargetSpec {
                table: "secret".into(),
                column: "value".into(),
                id_column: "id".into(),
                resource: "Secret".into(),
                field: "value".into(),
            },
            RekeyTargetSpec {
                table: "secret".into(),
                column: "backup".into(),
                id_column: "id".into(),
                resource: "Secret".into(),
                field: "backup".into(),
            },
            RekeyTargetSpec {
                table: "secret_version".into(),
                column: "encrypted_value".into(),
                id_column: "id".into(),
                resource: "SecretVersion".into(),
                field: "encryptedValue".into(),
            },
        ],
        inventory_complete: true,
    };
    (db, schema, options)
}

async fn drop_inventory_fixture(db: &tokio_postgres::Client, schema: &str) {
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.expect("drop owned inventory fixture schema");
}

#[tokio::test]
async fn metadata_inventory_accepts_all_declared_columns_across_multiple_resources() {
    let (db, schema, options) = inventory_fixture(false, false).await;
    let inventory = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options).await.expect("metadata and PostgreSQL agree");
    assert_eq!(inventory.targets.len(), 3);
    assert_eq!(inventory.scope.kind(), "ref:Team");
    assert_eq!(inventory.scope.id(), "42");
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn caller_complete_claim_cannot_hide_a_missing_declared_encrypted_column() {
    let (db, schema, mut options) = inventory_fixture(false, false).await;
    options.targets.retain(|target| target.field != "backup");
    options.inventory_complete = true;
    let error = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options).await.expect_err("omitted encrypted field must fail");
    assert!(matches!(error, Error::InvalidConfig(message) if message.contains("backup") && message.contains("missing")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_duplicate_targets_and_scope_mismatch() {
    let (db, schema, mut options) = inventory_fixture(false, false).await;
    options.targets.push(options.targets[0].clone());
    let duplicate =
        aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options).await.expect_err("duplicate caller target must fail");
    assert!(matches!(duplicate, Error::InvalidConfig(message) if message.contains("duplicate")));
    drop_inventory_fixture(&db, &schema).await;

    let (db, schema, mut options) = inventory_fixture(false, false).await;
    options.scope_kind = "service".into();
    options.scope_id = "global".into();
    let scope = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("declared ref scope cannot be asserted as service");
    assert!(matches!(scope, Error::InvalidConfig(message) if message.contains("scope")));
    drop_inventory_fixture(&db, &schema).await;

    let (db, schema, mut options) = inventory_fixture(false, false).await;
    options.scope_resource = Some("Project".into());
    let scope =
        aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options).await.expect_err("undeclared scope resource must fail");
    assert!(matches!(scope, Error::InvalidConfig(message) if message.contains("inventory") && message.contains("비어 있음")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_missing_physical_column_and_wrong_scope_foreign_key() {
    let (db, schema, options) = inventory_fixture(true, false).await;
    let missing_column =
        aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options).await.expect_err("missing PG column must fail");
    assert!(matches!(missing_column, Error::Database(message) if message.contains("backup")));
    drop_inventory_fixture(&db, &schema).await;

    let (db, schema, options) = inventory_fixture(false, true).await;
    let wrong_fk = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options).await.expect_err("wrong PG scope FK must fail");
    assert!(matches!(wrong_fk, Error::Database(message) if message.contains("team_id") && message.contains("team.id")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_nonunique_rekey_ids() {
    let (db, schema, options) = inventory_fixture(false, false).await;
    db.batch_execute(&format!(
        "ALTER TABLE {schema}.secret DROP CONSTRAINT secret_pkey;
         ALTER TABLE {schema}.secret ALTER COLUMN id SET NOT NULL;
         INSERT INTO {schema}.secret(id, team_id, value, backup) VALUES (7, 42, 'cipher-a', 'backup-a'), (7, 42, 'cipher-b', 'backup-b');"
    ))
    .await
    .expect("create duplicate IDs in a table without a uniqueness constraint");
    let error = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("rekey IDs must be unique because paging uses the ID cursor");
    assert!(matches!(error, Error::Database(message) if message.contains("id") && message.contains("unique")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_an_unvalidated_scope_foreign_key() {
    let (db, schema, options) = inventory_fixture(false, false).await;
    db.batch_execute(&format!(
        "ALTER TABLE {schema}.secret DROP CONSTRAINT secret_team_id_fkey;
         INSERT INTO {schema}.secret(id, team_id, value, backup) VALUES (7, 999, 'cipher', 'backup');
         ALTER TABLE {schema}.secret ADD CONSTRAINT secret_team_id_fkey FOREIGN KEY(team_id) REFERENCES {schema}.team(id) NOT VALID;"
    ))
    .await
    .expect("create an FK that is enforced for new writes but has not validated existing rows");
    let error = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("a NOT VALID FK does not prove existing scope rows are valid");
    assert!(matches!(error, Error::Database(message) if message.contains("scope FK") && message.contains("team_id")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_encrypted_target_ids_without_single_column_primary_key() {
    let (db, schema, options) = inventory_fixture(false, false).await;
    db.batch_execute(&format!(
        "INSERT INTO {schema}.team(id) VALUES (42);
         ALTER TABLE {schema}.secret DROP CONSTRAINT secret_pkey;
         INSERT INTO {schema}.secret(id, team_id, value, backup) VALUES
           (7, 42, 'first', 'first-backup'), (7, 42, 'second', 'second-backup');"
    ))
    .await
    .expect("create duplicate encrypted target IDs after removing the ID primary key");
    let error = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("encrypted target ID without a single-column primary key must fail");
    assert!(matches!(error, Error::Database(message) if message.contains("primary key") && message.contains("Secret.id")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_not_valid_scope_fk_with_existing_orphan() {
    let (db, schema, options) = inventory_fixture(false, false).await;
    db.batch_execute(&format!(
        "ALTER TABLE {schema}.secret DROP CONSTRAINT secret_team_id_fkey;
         INSERT INTO {schema}.secret(id, team_id, value, backup) VALUES (8, 999, 'orphan', 'orphan-backup');
         ALTER TABLE {schema}.secret ADD CONSTRAINT secret_team_id_fkey
           FOREIGN KEY (team_id) REFERENCES {schema}.team(id) NOT VALID;"
    ))
    .await
    .expect("install a matching NOT VALID FK after inserting a pre-existing orphan row");
    let error = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("scope FK with unvalidated existing rows must fail");
    assert!(matches!(error, Error::Database(message) if message.contains("validated") && message.contains("team_id")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_encrypted_column_type_and_nullability_drift() {
    let (db, schema, options) = inventory_fixture(false, false).await;
    db.batch_execute(&format!("ALTER TABLE {schema}.secret ALTER COLUMN value TYPE bigint USING 1::bigint"))
        .await
        .expect("change encrypted column type away from the declaration");
    let wrong_type = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("wrong encrypted column type must fail");
    assert!(matches!(wrong_type, Error::Database(message) if message.contains("type=int8") && message.contains("expected_type=text")));
    drop_inventory_fixture(&db, &schema).await;

    let (db, schema, options) = inventory_fixture(false, false).await;
    db.batch_execute(&format!("ALTER TABLE {schema}.secret ALTER COLUMN backup DROP NOT NULL"))
        .await
        .expect("change encrypted column nullability away from the declaration");
    let wrong_nullability = aip_service::encryption_runtime::verify_rekey_inventory(&db, &schema, &options)
        .await
        .expect_err("wrong encrypted column nullability must fail");
    assert!(matches!(wrong_nullability, Error::Database(message) if message.contains("not_null=false") && message.contains("expected_not_null=true")));
    drop_inventory_fixture(&db, &schema).await;
}

#[tokio::test]
async fn metadata_inventory_rejects_an_unrelated_schema() {
    let (db, _schema, options) = inventory_fixture(false, false).await;
    let error = aip_service::encryption_runtime::verify_rekey_inventory(&db, "aip_rekey_wrong_schema", &options)
        .await
        .expect_err("schema without deployment declaration metadata must fail");
    assert!(matches!(error, Error::Database(message) if message.contains("metadata")));
    drop_inventory_fixture(&db, &_schema).await;
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
        "auth": {"issuer": "https://auth.example.com", "audience": "aip", "jwks": {"kind": "https", "url": "https://auth.example.com/.well-known/jwks.json"}},
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
        "auth": {"issuer": "https://auth.example.com", "audience": "aip", "jwks": {"kind": "https", "url": "https://auth.example.com/.well-known/jwks.json"}}
    }"#;
    let config: aip_service::Config = serde_json::from_str(json).expect("valid config without encryption");
    assert!(config.encryption.is_none());
}
