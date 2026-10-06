use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

async fn request(port: u16, bytes: &str) -> Value {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(bytes.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(2), socket.read_to_end(&mut response)).await.unwrap().unwrap();
    let text = String::from_utf8(response).unwrap();
    serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn v13_contract_header_validation_and_auth_precede_database_access() {
    let (port, keys) = spike_v6_transport::start(json!({})).await;
    let fingerprint = "a".repeat(64);
    for value in [String::new(), "a".repeat(63), "A".repeat(64), "g".repeat(64), format!("{fingerprint}\r\nx-aip-contract: {fingerprint}")] {
        // 잘못된 헤더에서는 미전송 본문을 기다리지 않는다.
        let response = request(port, &format!("POST /read HTTP/1.1\r\ncontent-length: 2\r\nx-aip-contract: {value}\r\n\r\n")).await;
        assert_eq!(response["code"], "BAD_REQUEST");
    }
    for path in ["/read", "/apply", "/status"] {
        let response = request(
            port,
            &format!("POST {path} HTTP/1.1\r\ncontent-length: 2\r\nauthorization: Bearer invalid\r\nx-aip-contract: {fingerprint}\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(response["code"], "UNAUTHENTICATED");
        let token = keys.issue(1, 60);
        let response = request(
            port,
            &format!("POST {path} HTTP/1.1\r\ncontent-length: 2\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(response["code"], "CONTRACT_MISMATCH");
    }
}
