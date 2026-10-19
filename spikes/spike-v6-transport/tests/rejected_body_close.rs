use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn rejected_upload_reaches_the_client_without_resetting_the_response() {
    let (port, _) = spike_v6_transport::start(json!({})).await;
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let length = spike_v6_transport::MAX_BODY + 1024;
    writer.write_all(format!("POST /session HTTP/1.1\r\nhost: localhost\r\ncontent-length: {length}\r\n\r\n").as_bytes()).await.unwrap();
    let upload = tokio::spawn(async move {
        let result = writer.write_all(&vec![b'x'; length]).await;
        let _ = writer.shutdown().await;
        result
    });
    let mut response = Vec::new();
    let received = tokio::time::timeout(Duration::from_secs(2), reader.read_to_end(&mut response)).await.unwrap();
    assert!(received.is_ok(), "a proxy must receive a complete HTTP rejection instead of a TCP reset: {received:?}");
    let text = String::from_utf8_lossy(&response);
    assert!(text.contains("WIRE.PAYLOAD_TOO_LARGE"), "{text}");
    let _ = upload.await;
}

#[tokio::test]
async fn rejected_upload_does_not_wait_for_a_body_that_the_client_never_sends() {
    let (port, _) = spike_v6_transport::start(json!({})).await;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(b"POST /session HTTP/1.1\r\ncontent-length: 999999999\r\n\r\n").await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut response)).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&response).contains("WIRE.PAYLOAD_TOO_LARGE"));
}
