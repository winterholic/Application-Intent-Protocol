use serde_json::json;
use spike_v1_fixture::{Form, load_str};

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
