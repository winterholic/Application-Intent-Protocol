use aip_execution::rag::{AnswerProvider, EmbeddingProvider, OpenAiCompatibleEmbedder, OpenAiCompatibleGenerator, RagError};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

fn serve_once(response: &'static str) -> (String, thread::JoinHandle<(String, Vec<u8>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1/test", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let (headers, mut body) = read_headers(&mut stream);
        let content_len = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut rest = vec![0; content_len.saturating_sub(body.len())];
        if !rest.is_empty() {
            stream.read_exact(&mut rest).unwrap();
            body.extend(rest);
        }
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        );
        stream.write_all(raw.as_bytes()).unwrap();
        (headers, body)
    });
    (endpoint, worker)
}

fn read_headers(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 16 * 1024);
    }
    let header_end = bytes.len();
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    (headers, bytes[header_end..].to_vec())
}

#[tokio::test]
async fn named_embedding_provider_makes_real_http_request_without_exposing_credentials() {
    let (endpoint, worker) = serve_once(r#"{"data":[{"index":0,"embedding":[0.25,0.75]}]}"#);
    let provider = OpenAiCompatibleEmbedder::new(&endpoint, "test-embed", "server-secret", Duration::from_secs(2), 4).unwrap();
    let vectors = provider.embed(&["hello world".into()]).await.unwrap();
    assert_eq!(vectors, [vec![0.25, 0.75]]);
    let (headers, body) = worker.join().unwrap();
    assert!(headers.to_lowercase().contains("authorization: bearer server-secret"));
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["model"], "test-embed");
    assert_eq!(body["input"][0], "hello world");
}

#[tokio::test]
async fn named_generator_uses_chat_completion_http_contract_with_server_token_cap() {
    let (endpoint, worker) = serve_once(r#"{"choices":[{"message":{"content":"A grounded answer."}}]}"#);
    let generator = OpenAiCompatibleGenerator::new(&endpoint, "test-chat", "server-secret", Duration::from_secs(2), 4096).unwrap();
    let answer = generator.generate("system policy", "JSON data", 77).await.unwrap();
    assert_eq!(answer, "A grounded answer.");
    let (headers, body) = worker.join().unwrap();
    assert!(headers.to_lowercase().contains("authorization: bearer server-secret"));
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["model"], "test-chat");
    assert_eq!(body["max_completion_tokens"], 77);
    assert_eq!(body["messages"][0]["content"], "system policy");
    assert_eq!(body["messages"][1]["content"], "JSON data");
}

#[test]
fn remote_provider_urls_require_https_except_local_test_endpoints() {
    assert!(OpenAiCompatibleEmbedder::new("http://example.com/v1/embeddings", "model", "key", Duration::from_secs(1), 1).is_err());
    assert!(OpenAiCompatibleGenerator::new("https://api.example/v1/chat/completions", "model", "key", Duration::from_secs(1), 1024).is_ok());
    assert!(matches!(
        OpenAiCompatibleGenerator::new("https://user:pass@api.example/v1/chat/completions", "model", "key", Duration::from_secs(1), 1024),
        Err(RagError::InvalidInput)
    ));
}
