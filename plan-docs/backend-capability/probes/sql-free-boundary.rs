use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_extensions;
use spike_v6_transport::server::{listen_with_extensions, ReadExtensions, Server, WorkerLang};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const A: &str = r#"actor Compute
resource Compute {
  fields { id: Id }
  extension read count {
    input { text: Text }
    output { length: Int }
    effect none
    deadline 9s
    implementation "compute.count"
  }
}"#;
async fn post(server: &Server, fingerprint: &str, name: &str) -> Value {
    let token = server.keys.issue(1, 60);
    let body = json!({"extension":name,"input":{"text":"가😀A"}}).to_string();
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream
        .write_all(
            format!(
                "POST /extension HTTP/1.1\r\nhost: {}\r\nauthorization: Bearer {}\r\nx-aip-contract: {}\r\ncontent-length: {}\r\n\r\n{}",
                server.address,
                token,
                fingerprint,
                body.len(),
                body
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut data = Vec::new();
    tokio::time::timeout(Duration::from_secs(12), stream.read_to_end(&mut data)).await.unwrap().unwrap();
    let wire = String::from_utf8(data).unwrap();
    serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap()
}
#[tokio::main]
async fn main() {
    sqlgen::set_schema("aip_capability_probe");
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        for (label, source, dburl, expect) in [
            ("pure", A.to_string(), "host=localhost dbname=postgres", "OK"),
            ("db-unavailable", A.to_string(), "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1", "DB_UNAVAILABLE"),
            ("sync-over-five-seconds", A.replace("compute.count", "compute.slow"), "host=localhost dbname=postgres", "OK"),
            (
                "declaration-deadline",
                A.replace("compute.count", "compute.slow").replace("deadline 9s", "deadline 1s"),
                "host=localhost dbname=postgres",
                "DEADLINE_EXCEEDED",
            ),
        ] {
            let facts = load_str(&source, Form::A).unwrap().execution;
            assert_eq!(facts["resources"]["Compute"]["extensions"]["count"]["access"], json!({}));
            let fingerprint = contract_fingerprint_with_extensions(&facts, IdWire::DecimalString);
            let server = listen_with_extensions(
                facts,
                IdWire::DecimalString,
                "127.0.0.1:0".parse().unwrap(),
                dburl.into(),
                Some(ReadExtensions { lang, dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/extensions")) }),
            )
            .await
            .unwrap();
            let start = Instant::now();
            let result = post(&server, &fingerprint, "Compute.count").await;
            let elapsed = start.elapsed().as_millis();
            if expect == "OK" {
                assert_eq!(result["ok"], true, "{label}: {result}");
                assert_eq!(result["output"], json!({"length":3}));
            } else {
                assert_eq!(result["code"], expect, "{label}: {result}");
            }
            if label == "sync-over-five-seconds" {
                assert!(elapsed >= 6000);
            }
            println!("{lang:?} | {label} | {expect} | {elapsed}ms");
            server.shutdown().await.unwrap();
        }
    }
    let facts = load_str(A, Form::A).unwrap().execution;
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    assert!(ddl.iter().any(|s| s.contains(".compute ")));
    println!("resource namespace produces table DDL: true (DDL not executed)");
}
