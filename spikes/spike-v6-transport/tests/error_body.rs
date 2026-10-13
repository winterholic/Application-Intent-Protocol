//! CD-2: a failed reply says in its body which request it was (`requestId`, the same value as the `x-request-id`
//! header) and whether to send it again (`retry`, the registry value of the code), so a caller that only keeps the
//! body (an AI agent reading a log line) can still report and decide.
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

async fn exchange(port: u16, request: &str) -> (String, Value) {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let id = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("x-request-id: ").map(str::to_string)).expect("x-request-id header");
    (id, serde_json::from_str(body).unwrap())
}

#[tokio::test]
async fn failed_replies_carry_request_id_and_retry_in_the_body() {
    let (port, keys) = spike_v6_transport::start(json!({"resources": {}})).await;
    let token = keys.issue(1, 60);
    let cases = [
        // (request, code, retry)
        (format!("POST /read HTTP/1.1\r\ncontent-length: 2\r\nauthorization: Bearer {token}\r\n\r\n{{}}"), "WIRE.CONTRACT_REQUIRED", "never"),
        ("POST /read HTTP/1.1\r\ncontent-length: 2\r\nauthorization: Bearer nope\r\n\r\n{}".to_string(), "WIRE.UNAUTHENTICATED", "reauthenticate"),
        ("POST /nowhere HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}".to_string(), "WIRE.NOT_FOUND", "never"),
    ];
    for (request, code, retry) in cases {
        let (header_id, body) = exchange(port, &request).await;
        assert_eq!(body["code"], code, "{body}");
        assert_eq!(body["requestId"], header_id.as_str(), "{code}: body requestId equals x-request-id: {body}");
        assert_eq!(body["retry"], retry, "{code}: {body}");
    }
}

#[tokio::test]
async fn successful_replies_do_not_get_error_fields() {
    let (port, keys) = spike_v6_transport::start(json!({})).await;
    let token = keys.issue(1, 60);
    let (_, body) = exchange(port, &format!("POST /session HTTP/1.1\r\ncontent-length: 2\r\nauthorization: Bearer {token}\r\n\r\n{{}}")).await;
    assert_eq!(body["ok"], true, "{body}");
    assert!(body.get("requestId").is_none() && body.get("retry").is_none(), "{body}");
}
