//! A failed call can be traced: every JSON reply names its request in `x-request-id`, an upstream id is kept
//! when it is a safe token, and a browser is allowed to read the header.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::{listen_with_options, ServerOptions};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

async fn head_of(addr: std::net::SocketAddr, extra_headers: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(format!("POST /read HTTP/1.1\r\nhost: {addr}\r\ncontent-length: 2\r\n{extra_headers}\r\n{{}}").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    text.split_once("\r\n\r\n").unwrap().0.to_ascii_lowercase()
}

fn request_id(head: &str) -> Option<String> {
    head.lines().find_map(|line| line.strip_prefix("x-request-id: ")).map(str::to_string)
}

async fn server(options: ServerOptions) -> spike_v6_transport::server::Server {
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    listen_with_options(facts, IdWire::SafeNumber, "127.0.0.1:0".parse().unwrap(), aip_test_support::test_database_url!().into(), None, options)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_reply_carries_a_distinct_generated_request_id() {
    let server = server(ServerOptions::default()).await;
    let (first, second) = (head_of(server.address, "").await, head_of(server.address, "").await);
    let (a, b) = (request_id(&first).expect("x-request-id on the reply"), request_id(&second).expect("x-request-id on the reply"));
    assert_ne!(a, b);
    for id in [&a, &b] {
        assert!(id.starts_with("req_") && id.len() == 28 && id[4..].bytes().all(|c| c.is_ascii_hexdigit()), "{id}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upstream_id_is_kept_only_when_it_is_a_safe_token() {
    let server = server(ServerOptions::default()).await;
    let kept = head_of(server.address, "x-request-id: proxy-7f3a9c.01_ab\r\n").await;
    assert_eq!(request_id(&kept).as_deref(), Some("proxy-7f3a9c.01_ab"));
    for bad in ["x-request-id: short\r\n", "x-request-id: has spaces in it\r\n", "x-request-id: semi;colon-1234\r\n"] {
        let head = head_of(server.address, bad).await;
        let id = request_id(&head).expect("a generated id replaces it");
        assert!(id.starts_with("req_"), "{bad:?} -> {id}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_browser_may_read_the_request_id() {
    let server = server(ServerOptions::default()).await;
    let origin = format!("http://{}", server.address);
    let head = head_of(server.address, &format!("origin: {origin}\r\n")).await;
    assert!(head.contains(&format!("access-control-allow-origin: {origin}")), "{head}");
    assert!(head.contains("access-control-expose-headers: x-request-id"), "{head}");
    assert!(request_id(&head).is_some());
}
