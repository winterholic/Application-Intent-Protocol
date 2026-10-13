//! `/metrics` exists only with a configured scrape token, answers only that token, and counts finished requests.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, ServerOptions},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

struct NoSessions;
impl Authenticator for NoSessions {
    fn verify_session<'a>(&'a self, _: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
    fn verify_durable<'a>(
        &'a self,
        _: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::BadSignature) })
    }
}

const TOKEN: &str = "metrics-scrape-token-0123456789abcdef";

async fn get(addr: std::net::SocketAddr, path: &str, authorization: Option<&str>) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let auth = authorization.map(|value| format!("authorization: {value}\r\n")).unwrap_or_default();
    stream.write_all(format!("GET {path} HTTP/1.1\r\nhost: localhost\r\n{auth}\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (head.split_whitespace().nth(1).unwrap().parse().unwrap(), head.to_ascii_lowercase(), body.to_string())
}

async fn server(token: Option<&str>) -> spike_v6_transport::server::Server {
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    let infrastructure = Infrastructure { metrics_token: token.map(str::to_string), ..Infrastructure::default() };
    listen_with_infrastructure(
        facts,
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(NoSessions),
        None,
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_need_the_configured_token_and_count_requests_by_route_status_and_code() {
    let closed = server(None).await;
    let (_, head, body) = get(closed.address, "/metrics", Some(&format!("Bearer {TOKEN}"))).await;
    assert!(head.contains("content-type: application/json") && !body.contains("aip_requests_total"), "unconfigured: {body}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["ok"], false, "{body}");
    closed.shutdown().await.unwrap();

    let open = server(Some(TOKEN)).await;
    let addr = open.address;
    assert_eq!(get(addr, "/health", None).await.0, 200);
    // An unauthenticated read is rejected with a code; the metric keeps the code, never the path or the body.
    let (status, _, _) = get(addr, "/read?secret=1", None).await;
    assert_ne!(status, 0);
    // A header without the Bearer scheme never reaches a route: the header parser rejects it first.
    let (_, _, body) = get(addr, "/metrics", Some(TOKEN)).await;
    assert!(body.contains("\"WIRE.BAD_REQUEST\"") && !body.contains("aip_requests_total"), "{body}");
    for authorization in [None, Some("Bearer wrong")] {
        let (status, head, body) = get(addr, "/metrics", authorization).await;
        assert_eq!(status, 401, "{authorization:?}: {body}");
        assert!(head.contains("www-authenticate: bearer"), "{head}");
    }
    let (status, head, body) = get(addr, "/metrics", Some(&format!("Bearer {TOKEN}"))).await;
    assert_eq!(status, 200, "{body}");
    assert!(head.contains("content-type: text/plain; version=0.0.4"), "{head}");
    assert!(body.contains("aip_requests_total{route=\"health\",status=\"200\",code=\"\"}"), "{body}");
    assert!(body.contains("aip_request_duration_seconds_count{route=\"health\"}"), "{body}");
    assert!(body.contains("aip_requests_total{route=\"metrics\",status=\"401\",code=\"WIRE.UNAUTHENTICATED\"} 2"), "{body}");
    assert!(body.lines().any(|line| line.starts_with("aip_requests_total{route=\"read\"")), "{body}");
    assert!(!body.contains("secret"), "{body}");
    open.shutdown().await.unwrap();
}
