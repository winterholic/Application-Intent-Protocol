use aip_crypto::cell::{Context, DataKey};
use aip_migrate::external::{Mapping, Table, plan_external};
use futures_util::FutureExt;
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();

#[test]
fn encrypted_fields_generate_proper_ddl_and_blind_indices() {
    let source = r#"
actor principal
resource Project {
  fields {
    id: Id
    name: Text
  }
}
resource Secret {
  fields {
    id: Id
    project: Project
    value: Text(1..10000) encrypted(scope: project)
    tag: Text(1..255) encrypted(scope: project, lookup: eq)
    service_token: Text(1..500)? encrypted(scope: service)
  }
  unique tag
}
"#;
    let facts = load_str(source, Form::A).unwrap().execution;
    let (ddl, _) = aip_migrate::creation("aip_enc_fixture", &facts).unwrap();

    // Check that Secret table was generated with text column (no range check for encrypted values)
    let secret_ddl = ddl.iter().find(|s| s.contains("CREATE TABLE aip_enc_fixture.secret")).expect("secret table must be created");
    assert!(secret_ddl.contains("value text NOT NULL"), "encrypted value must be a plain text column without range check: {secret_ddl}");
    assert!(!secret_ddl.contains("char_length(value)"), "ciphertext length must not have plaintext range check: {secret_ddl}");
    assert!(secret_ddl.contains("tag text NOT NULL"), "encrypted tag must be present: {secret_ddl}");
    assert!(secret_ddl.contains("tag__bidx text NOT NULL"), "lookup: eq requires tag__bidx: {secret_ddl}");
    assert!(secret_ddl.contains("tag__bidx_next text"), "lookup: eq requires tag__bidx_next: {secret_ddl}");
    assert!(secret_ddl.contains("service_token text"), "optional encrypted field must be nullable text: {secret_ddl}");

    // Check that unique index targets tag__bidx, NOT tag
    let unique_idx = ddl.iter().find(|s| s.contains("CREATE UNIQUE INDEX") && s.contains("secret")).expect("unique index must exist");
    assert!(unique_idx.contains("(tag__bidx)"), "unique constraint on encrypted field must index tag__bidx: {unique_idx}");
    assert!(!unique_idx.contains("(tag)"), "unique constraint must not index raw tag column: {unique_idx}");
}

#[test]
fn invalid_encryption_metadata_is_rejected() {
    let base_facts = load_str("actor principal resource Secret {fields {id: Id; value: Text}}", Form::A).unwrap().execution;

    // Bad metadata shapes
    for bad in [json!({}), json!(true), json!(null), json!("string"), json!({"scope":"none"})] {
        let mut facts = base_facts.clone();
        facts["resources"]["Secret"]["fields"]["value"]["encrypted"] = bad;
        let err = aip_migrate::creation("aip_enc_fixture", &facts).expect_err("malformed encrypted metadata must be rejected");
        assert_eq!(err["code"], "BAD_FACTS");
    }

    // Unsupported scope kind
    let mut bad_scope = base_facts.clone();
    bad_scope["resources"]["Secret"]["fields"]["value"]["encrypted"] = json!({"scope":{"kind":"global"},"lookup":"none"});
    assert_eq!(aip_migrate::creation("aip_enc_fixture", &bad_scope).unwrap_err()["code"], "BAD_FACTS");

    // Missing ref scope target
    let mut bad_ref = base_facts.clone();
    bad_ref["resources"]["Secret"]["fields"]["value"]["encrypted"] =
        json!({"scope":{"kind":"ref","field":"proj","target":"NonExistent"},"lookup":"none"});
    assert_eq!(aip_migrate::creation("aip_enc_fixture", &bad_ref).unwrap_err()["code"], "BAD_FACTS");

    // Unsupported type
    let mut bad_type = load_str("actor principal resource Secret {fields {id: Id; count: Int}}", Form::A).unwrap().execution;
    bad_type["resources"]["Secret"]["fields"]["count"]["encrypted"] = json!({"scope":{"kind":"service"},"lookup":"none"});
    assert_eq!(aip_migrate::creation("aip_enc_fixture", &bad_type).unwrap_err()["code"], "BAD_FACTS");

    // Unique without lookup: eq
    let mut no_lookup_unique = base_facts.clone();
    no_lookup_unique["resources"]["Secret"]["fields"]["value"]["encrypted"] = json!({"scope":{"kind":"service"},"lookup":"none"});
    no_lookup_unique["resources"]["Secret"]["unique"] = json!([["value"]]);
    assert_eq!(aip_migrate::creation("aip_enc_fixture", &no_lookup_unique).unwrap_err()["code"], "BAD_FACTS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn derive_blocks_plaintext_encrypted_transitions() {
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let schema = format!("aip_derive_enc_{}_{}", pid, nanos);

    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();

    let old_source = r#"
actor principal
resource Secret {
  fields {
    id: Id
    val: Text
  }
}
"#;
    let new_source = r#"
actor principal
resource Secret {
  fields {
    id: Id
    val: Text encrypted(scope: service)
  }
}
"#;
    let old_facts = load_str(old_source, Form::A).unwrap().execution;
    let new_facts = load_str(new_source, Form::A).unwrap().execution;

    let setup = AssertUnwindSafe(async {
        // Initialize schema with old facts first
        aip_migrate::init(DB, &schema, &old_facts).await.unwrap();

        // Planning with new facts (transitioning plaintext -> encrypted) must be Blocked
        let plan_v2 = aip_migrate::plan(DB, &schema, &new_facts, &None).await.unwrap();
        assert_eq!(plan_v2["blocked"], true);
        let blocked_change = plan_v2["changes"].as_array().unwrap().iter().find(|c| c["class"] == "Blocked").expect("Blocked change must exist");
        assert!(
            blocked_change["reason"].as_str().unwrap().contains("plaintext와 encrypted 간 전환 또는 scope 변경 불가"),
            "reason: {blocked_change}"
        );

        // Applying this plan must also be rejected with MIGRATION_BLOCKED
        let apply_err = aip_migrate::apply(DB, &schema, &new_facts, &None, plan_v2["digest"].as_str().unwrap())
            .await
            .expect_err("apply must reject blocked migration");
        assert_eq!(apply_err["code"], "MIGRATION_BLOCKED");
    })
    .catch_unwind()
    .await;

    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;

    if let Err(panic) = setup {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_adopt_full_table_scan_detects_tampered_row() {
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let source_schema = format!("aip_ext_src_{}_{}", pid, nanos);
    let target_schema = format!("aip_ext_tgt_{}_{}", pid, nanos);

    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();

    let key = DataKey::new("dek1", 1, [0x42; 32]).unwrap();

    let setup = AssertUnwindSafe(async {
        db.batch_execute(&format!(
            "CREATE SCHEMA {source_schema}; CREATE TABLE {source_schema}.secrets (id serial PRIMARY KEY, secret_val text NOT NULL)"
        ))
        .await
        .unwrap();

        // Insert 150 valid v2 cell frames
        for id in 1..=150 {
            let ctx = Context {
                schema: &target_schema,
                resource: "Secret",
                field: "secret_val",
                row_id: &id.to_string(),
                scope_kind: "service",
                scope_id: "global",
            };
            let frame = key.seal(&ctx, format!("secret content {id}").as_bytes()).unwrap();
            db.execute(&format!("INSERT INTO {source_schema}.secrets (secret_val) VALUES ($1)"), &[&frame]).await.unwrap();
        }

        let mut mapping = Mapping { schema: source_schema.clone(), tables: BTreeMap::new() };
        let mut cols = BTreeMap::new();
        cols.insert("id".to_string(), "id".to_string());
        cols.insert("secret_val".to_string(), "secret_val".to_string());
        mapping.tables.insert("Secret".to_string(), Table { table: "secrets".to_string(), columns: cols, shared_pk_without_fk: false });

        let source = r#"
actor principal
resource Secret {
  fields {
    id: Id
    secret_val: Text encrypted(scope: service)
  }
}
"#;
        let facts = load_str(source, Form::A).unwrap().execution;

        // 1. All 150 rows are valid v2 cell frames -> plan_external succeeds!
        let plan = plan_external(DB, &target_schema, &facts, &mapping, "decimal")
            .await
            .expect("plan_external must succeed when all 150 rows have valid v2 frames");
        assert_eq!(plan["ok"], true);

        // 2. Tamper the 101st row (past prefix 100 sample threshold) with plaintext
        db.execute(&format!("UPDATE {source_schema}.secrets SET secret_val = 'tampered_plaintext_secret' WHERE id = 101"), &[]).await.unwrap();

        let err = plan_external(DB, &target_schema, &facts, &mapping, "decimal").await.expect_err("plan_external must reject tampered 101st row");
        assert_eq!(err["code"], "EXTERNAL_SCHEMA");
        assert!(err["message"].as_str().unwrap().contains("암호화 셀 포맷 오류"), "error message should indicate cell format failure: {err}");

        // 3. Tamper the 101st row with a tag mismatch (e.g. tag 'r' instead of 's')
        let ref_ctx =
            Context { schema: &target_schema, resource: "Secret", field: "secret_val", row_id: "101", scope_kind: "ref:project", scope_id: "p1" };
        let ref_frame = key.seal(&ref_ctx, b"mismatched tag frame").unwrap();
        db.execute(&format!("UPDATE {source_schema}.secrets SET secret_val = $1 WHERE id = 101"), &[&ref_frame]).await.unwrap();

        let err_tag = plan_external(DB, &target_schema, &facts, &mapping, "decimal")
            .await
            .expect_err("plan_external must reject scope tag mismatch on 101st row");
        assert_eq!(err_tag["code"], "EXTERNAL_SCHEMA");
        assert!(err_tag["message"].as_str().unwrap().contains("암호화 셀 태그 불일치"), "error message should indicate tag mismatch: {err_tag}");
    })
    .catch_unwind()
    .await;

    // Resource cleanup
    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {source_schema} CASCADE; DROP SCHEMA IF EXISTS {target_schema} CASCADE")).await;

    if let Err(panic) = setup {
        std::panic::resume_unwind(panic);
    }
}
