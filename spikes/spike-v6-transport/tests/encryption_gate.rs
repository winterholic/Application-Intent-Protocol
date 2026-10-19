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

#[tokio::test]
async fn fresh_process_local_keys_do_not_open_any_listener_constructor() {
    use aip_crypto::{cell::DataKey, fingerprint::Keys};
    use spike_v6_transport::encryption::{get_runtime_crypto, set_runtime_crypto, RuntimeCrypto};
    use spike_v6_transport::server::{listen_with_apply_contract, listen_with_options, ServerOptions};
    use std::{collections::BTreeMap, sync::Arc};
    // Fresh key material no fixture allowlist can recognise: the gate must not
    // depend on which process-local keys are installed.
    let crypto = RuntimeCrypto {
        fp_keys: Keys::new("gate-counter-fp", vec![("gate-counter-fp", [0x9d; 32])]).unwrap(),
        data_keys: BTreeMap::from([("gate-counter-dek:7".into(), DataKey::new("gate-counter-dek", 7, [0x5a; 32]).unwrap())]),
    };
    let previous = get_runtime_crypto();
    set_runtime_crypto(Some(Arc::new(crypto)));
    let facts = load_str("actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}", Form::A).unwrap().execution;
    // A non-loopback address and an unusable DSN would fail differently, so
    // Unsupported proves the encryption readiness gate runs before address
    // validation, socket bind, and any database use, on every constructor.
    let mut failures = 0;
    for result in [
        listen_with_apply_contract(facts.clone(), IdWire::SafeNumber, "10.0.0.1:0".parse().unwrap(), "invalid DSN must never be used".into()).await,
        listen_with_options(
            facts.clone(),
            IdWire::SafeNumber,
            "10.0.0.1:0".parse().unwrap(),
            "invalid DSN must never be used".into(),
            None,
            ServerOptions::default(),
        )
        .await,
    ] {
        match result {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}"),
            Ok(server) => {
                failures += 1;
                server.shutdown().await.unwrap();
            }
        }
    }
    set_runtime_crypto(previous);
    assert_eq!(failures, 0, "a process-local key opened an encrypted listener");
}

#[tokio::test]
async fn installing_an_in_memory_key_does_not_enable_unverified_encrypted_storage() {
    use aip_crypto::{cell::DataKey, fingerprint::Keys};
    use spike_v6_transport::encryption::{set_runtime_crypto, RuntimeCrypto};
    use std::{collections::BTreeMap, sync::Arc};
    let crypto = RuntimeCrypto {
        fp_keys: Keys::new("fixture-fp", vec![("fixture-fp", [11; 32])]).unwrap(),
        data_keys: BTreeMap::from([("fixture:1".into(), DataKey::new("fixture", 1, [22; 32]).unwrap())]),
    };
    set_runtime_crypto(Some(Arc::new(crypto)));
    let facts = load_str("actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}", Form::A).unwrap().execution;
    let result = listen_with_apply_contract(facts, IdWire::SafeNumber, "127.0.0.1:0".parse().unwrap(), "invalid DSN must not be used".into()).await;
    set_runtime_crypto(None);
    match result {
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::Unsupported),
        Ok(server) => {
            server.shutdown().await.unwrap();
            panic!("a process-local test key bypassed the encrypted storage readiness gate");
        }
    }
}
