use aip_crypto::fingerprint::Keys;
use aip_execution::encrypted_journal::{self, Context, Error};
use serde_json::{Value, json};

fn facts() -> Value {
    json!({"resources":{
        "Secret":{"fields":{"value":{"ty":"Text?","encrypted":{"scope":{"kind":"service"}}},"name":{"ty":"Text"}}},
        "Version":{"fields":{"value":{"ty":"Text","encrypted":{"scope":{"kind":"service"}}}}}
    }})
}
fn context() -> Context<'static> {
    Context { schema: "aip_journal", principal: "actor:1", request_key: "request-1" }
}
fn keys(active: &str) -> Keys {
    Keys::new(active, vec![("old", [7; 32]), ("new", [9; 32])]).unwrap()
}

#[test]
fn all_write_shapes_protect_values_expect_and_where_and_replay_after_rotation() {
    let sentinel = "sensitive-canary-582e8c36";
    let plain = json!({"atomic":[
        {"create":"Secret","values":{"value":sentinel,"name":"ordinary"}},
        {"apply":"Secret.edit","values":{"value":sentinel},"expect":{"value":sentinel},"target":{"where":[{"field":"value","op":"eq","value":sentinel}]}},
        {"compose":"Secret","targets":{"where":[{"field":"value","op":"eq","value":sentinel}]},"steps":[
            {"transition":"edit","values":{"value":sentinel},"expect":{"value":sentinel}},
            {"create":"Version","values":{"value":{"const":sentinel}}}
        ]}
    ]});
    let stored = encrypted_journal::protect(&facts(), &plain, &keys("old"), &context()).unwrap();
    assert!(!stored.to_string().contains(sentinel));
    assert_eq!(stored["atomic"][0]["values"]["name"], "ordinary");
    assert_eq!(stored.to_string().matches("idem_v2:old:").count(), 8);
    assert!(encrypted_journal::verify(&facts(), &stored, &plain, &keys("new"), &context()).unwrap());
    let mut changed = plain.clone();
    changed["atomic"][2]["steps"][0]["expect"]["value"] = json!("tampered");
    assert!(!encrypted_journal::verify(&facts(), &stored, &changed, &keys("new"), &context()).unwrap());
    changed = plain.clone();
    changed["atomic"][0]["values"]["name"] = json!("changed");
    assert!(!encrypted_journal::verify(&facts(), &stored, &changed, &keys("new"), &context()).unwrap());
}

#[test]
fn compose_item_sources_are_preserved_and_only_the_constant_payload_is_fingerprinted() {
    let plain = json!({"compose":"Secret","targets":{"ids":[1]},"steps":[
        {"create":"Version","values":{"value":{"item":"row.name"}}},
        {"create":"Version","values":{"value":{"const":"sensitive-canary"}}}
    ]});
    let stored = encrypted_journal::protect(&facts(), &plain, &keys("old"), &context()).unwrap();
    assert_eq!(stored["steps"][0], plain["steps"][0]);
    assert!(stored["steps"][1]["values"]["value"]["const"]["$aip_enc_fp"].is_string());
    assert!(!stored.to_string().contains("sensitive-canary"));
    assert!(encrypted_journal::verify(&facts(), &stored, &plain, &keys("new"), &context()).unwrap());
}

#[test]
fn no_fallback_on_fingerprint_failure_and_missing_retained_keys_are_availability_errors() {
    let plain = json!({"create":"Secret","values":{"value":"sensitive-canary"}});
    let stored = encrypted_journal::protect(&facts(), &plain, &keys("old"), &context()).unwrap();
    let missing = Keys::new("new", vec![("new", [9; 32])]).unwrap();
    assert_eq!(encrypted_journal::verify(&facts(), &stored, &plain, &missing, &context()).unwrap_err(), Error::Unavailable);
    assert_eq!(encrypted_journal::protect(&facts(), &plain, &keys("old"), &Context { schema: "", ..context() }).unwrap_err(), Error::InvalidInput);
    assert_eq!(encrypted_journal::verify(&facts(), &plain, &plain, &keys("old"), &context()).unwrap_err(), Error::Corrupted);
}

#[test]
fn marker_strings_are_reprotected_and_scalar_types_and_contexts_do_not_collide() {
    let plain = json!({"create":"Secret","values":{"value":"idem_v2:old:forged"}});
    let stored = encrypted_journal::protect(&facts(), &plain, &keys("old"), &context()).unwrap();
    assert!(!stored.to_string().contains("forged"));
    assert!(encrypted_journal::verify(&facts(), &stored, &plain, &keys("old"), &context()).unwrap());
    assert_eq!(encrypted_journal::verify(&facts(), &stored, &stored, &keys("old"), &context()).unwrap_err(), Error::InvalidInput);
    for (first, second) in [(json!("true"), json!(true)), (json!("1"), json!(1))] {
        let a = json!({"create":"Secret","values":{"value":first}});
        let b = json!({"create":"Secret","values":{"value":second}});
        let protected = encrypted_journal::protect(&facts(), &a, &keys("old"), &context()).unwrap();
        assert!(!encrypted_journal::verify(&facts(), &protected, &b, &keys("old"), &context()).unwrap());
    }
    for changed in [
        Context { schema: "aip_other", ..context() },
        Context { principal: "actor:2", ..context() },
        Context { request_key: "request-2", ..context() },
    ] {
        assert!(!encrypted_journal::verify(&facts(), &stored, &plain, &keys("old"), &changed).unwrap());
    }
    let nullable = json!({"create":"Secret","values":{"value":null}});
    assert_eq!(encrypted_journal::protect(&facts(), &nullable, &keys("old"), &context()).unwrap(), nullable);
}

#[test]
fn malformed_or_excessively_nested_requests_do_not_silently_drop_sensitive_paths() {
    for invalid in [
        json!({"create":"Missing","values":{"value":"sentinel"}}),
        json!({"apply":"Secret.edit","target":{"where":{"value":"sentinel"}}}),
        json!({"create":"Secret","values":{"value":{"$aip_enc_fp":"forged"}}}),
        json!({"compose":"Secret","steps":[{"transition":"edit","values":"sentinel"}]}),
        json!({"apply":"Secret.edit","create":"Secret","values":{"value":"sentinel"}}),
    ] {
        assert_eq!(encrypted_journal::protect(&facts(), &invalid, &keys("old"), &context()).unwrap_err(), Error::InvalidInput);
    }
    let mut nested = json!({"create":"Secret","values":{"value":"sentinel"}});
    for _ in 0..40 {
        nested = json!({"atomic":[nested]});
    }
    assert_eq!(encrypted_journal::protect(&facts(), &nested, &keys("old"), &context()).unwrap_err(), Error::InvalidInput);
}

#[test]
fn effect_journal_fingerprints_bind_the_full_input_and_retain_the_original_key() {
    use encrypted_journal::Effect;
    let input = json!({"key":"step1","values":{"value":"sensitive-canary"},"expect":{"value":"previous-sensitive-value"}});
    let effect = Effect { kind: "apply", name: "Secret.edit", input: &input, implementation: "bundle-1" };
    let marker = encrypted_journal::effect_marker(&effect, &keys("old"), &context()).unwrap();
    assert!(marker.starts_with("idem_v2:old:"));
    assert!(!marker.contains("sensitive"));
    assert!(encrypted_journal::verify_effect(&effect, &marker, &keys("new"), &context()).unwrap());
    for changed in [Effect { kind: "create", ..effect }, Effect { name: "Version.edit", ..effect }, Effect { implementation: "bundle-2", ..effect }] {
        assert!(!encrypted_journal::verify_effect(&changed, &marker, &keys("new"), &context()).unwrap());
    }
    let changed_input = json!({"key":"step1","values":{"value":"tampered"}});
    assert!(!encrypted_journal::verify_effect(&Effect { input: &changed_input, ..effect }, &marker, &keys("new"), &context()).unwrap());
    let missing = Keys::new("new", vec![("new", [9; 32])]).unwrap();
    assert_eq!(encrypted_journal::verify_effect(&effect, &marker, &missing, &context()).unwrap_err(), Error::Unavailable);
}
