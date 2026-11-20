use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::listen_with_extensions;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

async fn capabilities(address: std::net::SocketAddr, token: &str) -> Value {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let body = "{}";
    stream
        .write_all(
            format!(
                "POST /capabilities HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut output)).await.unwrap().unwrap();
    let output = String::from_utf8(output).unwrap();
    serde_json::from_str(output.split_once("\r\n\r\n").unwrap().1).unwrap()
}

// `protocol` is an optional field of /capabilities: absent until the hosting product sets it, then the exact protocol string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capabilities_reports_the_protocol_only_after_the_host_sets_it() {
    let facts = load_str(include_str!("fixtures/operations.aip"), Form::A).unwrap().execution;
    let server =
        listen_with_extensions(facts, IdWire::SafeNumber, "127.0.0.1:0".parse().unwrap(), aip_test_support::test_database_url!().into(), None)
            .await
            .unwrap();
    let token = server.keys.issue(1, 30);

    let before = capabilities(server.address, &token).await;
    assert_eq!(before["ok"], true, "{before}");
    assert!(before.get("protocol").is_none(), "an engine that was not told its protocol must not invent one: {before}");

    spike_v6_transport::set_protocol_version("aip-protocol/1.0");
    let after = capabilities(server.address, &token).await;
    assert_eq!(after["protocol"], json!("aip-protocol/1.0"), "{after}");
    assert_eq!(after["catalogVersion"], "aip-capabilities/1");
    server.shutdown().await.unwrap();
}
