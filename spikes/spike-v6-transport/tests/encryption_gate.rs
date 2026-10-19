use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::listen_with_apply_contract;

#[tokio::test]
async fn encryption_metadata_is_rejected_before_a_listener_can_serve_plaintext() {
    let plain = load_str("actor principal resource Secret {fields {id: Id; value: Text}}", Form::A).unwrap().execution;
    for metadata in [json!({"scope":{"kind":"service"},"lookup":"none"}), json!({}), json!(false), json!(null)] {
        let mut facts = plain.clone();
        facts["resources"]["Secret"]["fields"]["value"]["encrypted"] = metadata;
        match listen_with_apply_contract(facts, IdWire::SafeNumber, "127.0.0.1:0".parse().unwrap(), "invalid DSN must never be used".into()).await {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}"),
            Ok(server) => {
                server.shutdown().await.unwrap();
                panic!("an unsupported encrypted schema became a serving listener");
            }
        }
    }
    let server = listen_with_apply_contract(plain, IdWire::SafeNumber, "127.0.0.1:0".parse().unwrap(), "unused DSN".into()).await.unwrap();
    server.shutdown().await.unwrap();
}
