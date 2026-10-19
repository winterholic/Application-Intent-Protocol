use serde_json::json;
use spike_v1_fixture::{Form, load_str};

#[test]
fn unimplemented_encryption_metadata_never_becomes_plaintext_ddl() {
    let facts = load_str("actor principal resource Secret {fields {id: Id; value: Text}}", Form::A).unwrap().execution;
    aip_migrate::creation("aip_enc_fixture", &facts).unwrap();
    let normalized = load_str("actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}", Form::A).unwrap().execution;
    assert_eq!(aip_migrate::creation("aip_enc_fixture", &normalized).unwrap_err()["code"], "BAD_FACTS");
    for metadata in [json!({"scope":{"kind":"service"},"lookup":"none"}), json!({}), json!(true), json!(null)] {
        let mut future = facts.clone();
        future["resources"]["Secret"]["fields"]["value"]["encrypted"] = metadata;
        let error = aip_migrate::creation("aip_enc_fixture", &future).expect_err("unsupported metadata must never be silently ignored");
        assert_eq!(error["code"], "BAD_FACTS");
    }
}
