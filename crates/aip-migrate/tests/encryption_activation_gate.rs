use aip_migrate::external::{Mapping, plan_external};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};

#[tokio::test]
async fn encrypted_deployment_requires_aead_and_key_readiness_before_database_access() {
    let facts = load_str("actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}", Form::A).unwrap().execution;
    let mapping = Mapping { schema: "source_fixture".into(), tables: Default::default() };
    let error = plan_external("deliberately invalid DSN", "aip_enc_fixture", &facts, &mapping, "decimal").await.unwrap_err();
    assert_eq!(error["code"], "BAD_FACTS", "a header-only scanner must not make an encrypted deployment eligible: {error}");
    assert_eq!(aip_migrate::creation("aip_enc_fixture", &facts).unwrap_err()["code"], "BAD_FACTS");
    let plain = load_str("actor principal resource Secret {fields {id: Id; value: Text}}", Form::A).unwrap().execution;
    assert!(aip_migrate::creation("aip_enc_fixture", &plain).is_ok());
    for metadata in [json!({}), json!(null), json!(false)] {
        let mut malformed = plain.clone();
        malformed["resources"]["Secret"]["fields"]["value"]["encrypted"] = metadata;
        assert_eq!(aip_migrate::creation("aip_enc_fixture", &malformed).unwrap_err()["code"], "BAD_FACTS");
    }
}
