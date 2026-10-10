use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::{listen_with_extensions, ReadExtensions, WorkerLang};
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

async fn post(address: std::net::SocketAddr, route: &str, token: Option<&str>, fingerprint: Option<&str>, body: Value) -> Value {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let body = body.to_string();
    let auth = token.map(|t| format!("authorization: Bearer {t}\r\n")).unwrap_or_default();
    let contract = fingerprint.map(|f| format!("x-aip-contract: {f}\r\n")).unwrap_or_default();
    stream
        .write_all(format!("POST {route} HTTP/1.1\r\nhost: localhost\r\n{auth}{contract}content-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut output)).await.unwrap().unwrap();
    let output = String::from_utf8(output).unwrap();
    serde_json::from_str(output.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resource_free_operations_validate_auth_policy_input_output_and_no_ctx_in_both_workers() {
    let facts = load_str(include_str!("fixtures/operations.aip"), Form::A).unwrap().execution;
    let ddl = spike_v2_read::sqlgen::create_ddl(&facts).unwrap().join("\n");
    assert!(facts["resources"].as_object().unwrap().is_empty());
    assert!(
        ddl.lines().filter(|line| line.starts_with("CREATE TABLE")).all(|line| line.contains(".aip_")),
        "operation must not generate application tables: {ddl}"
    );
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        for lang in [WorkerLang::Node, WorkerLang::Python] {
            let fingerprint = spike_v5_sdk::contract_fingerprint_with_extensions(&facts, wire);
            let config = ReadExtensions { lang, dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/operations")) };
            let server = listen_with_extensions(facts.clone(), wire, "127.0.0.1:0".parse().unwrap(), spike_v2_read::DB_URL.into(), Some(config))
                .await
                .unwrap();
            let token = server.keys.issue(1, 30);
            let owner = if wire == IdWire::SafeNumber { json!(1) } else { json!("1") };
            let input = json!({"text":"가😀A","owner":owner,"optional":null});
            let body = json!({"operation":"length","input":input});
            let catalog = post(server.address, "/capabilities", Some(&token), None, json!({})).await;
            assert_eq!(catalog["ok"], true, "{catalog}");
            assert_eq!(catalog["operations"]["length"]["available"], true);
            assert_eq!(catalog["operations"]["length"]["dependencies"]["worker"], json!([]));
            assert!(catalog["operations"]["length"]["dependencies"]["authorization"].as_array().unwrap().contains(&json!("database")));
            assert!(!catalog.to_string().contains("compute.length"));
            assert!(catalog["operations"]["length"].get("allow").is_none());
            let result = post(server.address, "/operation", Some(&token), Some(&fingerprint), body.clone()).await;
            assert_eq!(result["ok"], true, "{lang:?}/{wire:?}: {result}");
            assert_eq!(result["output"], json!({"length":3}));
            assert_eq!(result["contractFingerprint"], fingerprint);
            for (case, token, fingerprint, body, code) in [
                ("anonymous", None, Some(fingerprint.as_str()), body.clone(), "UNAUTHENTICATED"),
                ("missing contract", Some(token.as_str()), None, body.clone(), "CONTRACT_MISMATCH"),
                (
                    "wrong contract",
                    Some(token.as_str()),
                    Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                    body.clone(),
                    "CONTRACT_MISMATCH",
                ),
                ("unknown", Some(token.as_str()), Some(fingerprint.as_str()), json!({"operation":"missing","input":{}}), "NOT_EXPOSED"),
                (
                    "extra body",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"length","input":input,"actor":1}),
                    "BAD_REQUEST",
                ),
                (
                    "extra input",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"length","input":{"text":"a","owner":owner,"optional":null,"actor":1}}),
                    "BAD_VALUE",
                ),
                (
                    "missing nullable",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"length","input":{"text":"a","owner":owner}}),
                    "BAD_VALUE",
                ),
                (
                    "text range",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"ranged","input":{"text":"가😀AB","count":1}}),
                    "BAD_VALUE",
                ),
                (
                    "int range",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"ranged","input":{"text":"a","count":4}}),
                    "BAD_VALUE",
                ),
                (
                    "output range",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"ranged","input":{"text":"a","count":3}}),
                    "OUTPUT_INVALID",
                ),
                (
                    "unsafe Int input",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"length","input":{"text":"a","owner":owner,"optional":9007199254740992i64}}),
                    "BAD_VALUE",
                ),
                (
                    "unsafe Int output",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"unsafeInteger","input":{}}),
                    "OUTPUT_INVALID",
                ),
                ("policy false", Some(token.as_str()), Some(fingerprint.as_str()), json!({"operation":"forbidden","input":{}}), "ACCESS_DENIED"),
                ("output", Some(token.as_str()), Some(fingerprint.as_str()), json!({"operation":"invalid","input":{}}), "OUTPUT_INVALID"),
                (
                    "caught ctx violation",
                    Some(token.as_str()),
                    Some(fingerprint.as_str()),
                    json!({"operation":"ctxProbe","input":{}}),
                    "ACCESS_NOT_DECLARED",
                ),
                ("deadline", Some(token.as_str()), Some(fingerprint.as_str()), json!({"operation":"slow","input":{}}), "DEADLINE_EXCEEDED"),
            ] {
                let result = post(server.address, "/operation", token, fingerprint, body).await;
                assert_eq!(result["code"], code, "{lang:?}/{wire:?}/{case}: {result}");
            }
            let other_token = server.keys.issue(2, 30);
            let denied = post(server.address, "/operation", Some(&other_token), Some(&fingerprint), body.clone()).await;
            assert_eq!(denied["code"], "ACCESS_DENIED", "cross-principal: {denied}");
            let no_alias = post(server.address, "/extension", Some(&token), Some(&fingerprint), json!({"extension":"length","input":input})).await;
            assert_ne!(no_alias["ok"], true, "operation cannot be called through an unguarded extension route");
            server.shutdown().await.unwrap();
        }
    }
}
