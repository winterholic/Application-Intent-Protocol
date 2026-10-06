use serde_json::{json, Value};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::{listen_with_extensions, listen_with_options, ServerOptions};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const BAD_DB: &str = "host=127.0.0.1 port=1 connect_timeout=1";

async fn request(address: SocketAddr, head: &str, body: &str) -> (String, Value) {
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("{head}\r\n{body}").as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(2), socket.read_to_string(&mut response)).await.unwrap().unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (headers.to_string(), serde_json::from_str(body).unwrap())
}

#[tokio::test]
async fn same_origin_browser_and_errors_are_readable_without_database() {
    let server = listen_with_extensions(json!({}), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), BAD_DB.into(), None).await.unwrap();
    let origin = format!("http://{}", server.address);
    let (headers, body) =
        request(server.address, &format!("POST /session HTTP/1.1\r\nHost: {}\r\nOrigin: {origin}\r\nContent-Length: 2\r\n", server.address), "{}")
            .await;
    assert_eq!(body["ok"], true);
    assert!(headers.contains(&format!("access-control-allow-origin: {origin}")), "{headers}");
    let (headers, body) = request(
        server.address,
        &format!("POST /session HTTP/1.1\r\nAuthorization: Bearer invalid\r\nHost: {}\r\nOrigin: {origin}\r\nContent-Length: 2\r\n", server.address),
        "{}",
    )
    .await;
    assert_eq!(body["code"], "UNAUTHENTICATED");
    assert!(headers.contains(&format!("access-control-allow-origin: {origin}")));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn unconfigured_foreign_origin_is_rejected_before_database_or_body() {
    let server = listen_with_extensions(json!({}), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), BAD_DB.into(), None).await.unwrap();
    let (headers, body) = request(server.address, "POST /apply HTTP/1.1\r\nOrigin: http://127.0.0.1:3000\r\nContent-Length: 999\r\n", "").await;
    assert_eq!(body["code"], "ORIGIN_NOT_ALLOWED");
    assert!(!headers.contains("access-control-allow-origin"));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_origin_configuration_is_rejected_before_binding() {
    for origin in [
        "*",
        "null",
        "https://localhost:3000",
        "http://example.com",
        "http://localhost:3000/",
        "http://localhost:3000?x=1",
        "http://user@localhost:3000",
        "http://localhost:",
        "http://[::1]:",
        "http://localhost:03000",
        "http://localhost:80",
        "http://localhost:0",
        "http://localhost:65536",
        "http://localhost:3000\r\nx-inject: 1",
    ] {
        let result = listen_with_options(
            json!({}),
            IdWire::DecimalString,
            "127.0.0.1:0".parse().unwrap(),
            BAD_DB.into(),
            None,
            ServerOptions { allowed_origins: vec![origin.into()] },
        )
        .await;
        assert!(result.is_err(), "accepted {origin:?}");
    }
}

#[tokio::test]
async fn explicit_preflight_is_route_and_header_bounded_before_authentication() {
    let origin = "http://localhost:3000";
    let server = listen_with_options(
        json!({}),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        BAD_DB.into(),
        None,
        ServerOptions { allowed_origins: vec![origin.into()] },
    )
    .await
    .unwrap();
    for path in ["/session", "/read", "/apply", "/status", "/extension"] {
        for requested in ["Content-Type", "authorization, content-type, x-aip-contract"] {
            let (headers, body) = request(server.address, &format!("OPTIONS {path} HTTP/1.1\r\nHost: {}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: {requested}\r\n", server.address), "").await;
            assert_eq!(body["ok"], true, "{path}: {body}");
            assert!(headers.contains(&format!("access-control-allow-origin: {origin}")));
            assert!(headers.contains("access-control-allow-methods: POST"));
            assert!(headers.contains("vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers"));
            assert!(!headers.contains("allow-credentials"));
        }
    }
    for extra in [
        "Access-Control-Request-Method: GET\r\n",
        "Access-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: x-unknown\r\n",
        "Access-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type,\r\n",
        "Access-Control-Request-Method: POST\r\nAccess-Control-Request-Method: POST\r\n",
    ] {
        let (headers, body) = request(server.address, &format!("OPTIONS /read HTTP/1.1\r\nOrigin: {origin}\r\n{extra}"), "").await;
        assert_eq!(body["ok"], false);
        assert!(!headers.contains("access-control-allow-methods"));
    }
    let (_, body) =
        request(server.address, &format!("OPTIONS /unknown HTTP/1.1\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\n"), "").await;
    assert_eq!(body["code"], "NOT_FOUND");
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn allowed_origin_can_read_authentication_contract_and_framing_errors() {
    let origin = "http://127.0.0.1:3000";
    let server = listen_with_options(
        json!({}),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        BAD_DB.into(),
        None,
        ServerOptions { allowed_origins: vec![origin.into()] },
    )
    .await
    .unwrap();
    let token = server.keys.issue(1, 60);
    let cases = [
        ("Authorization: Bearer invalid\r\n".to_string(), "{}", "UNAUTHENTICATED"),
        (format!("Authorization: Bearer {token}\r\nx-aip-contract: {}\r\n", "0".repeat(64)), "{}", "CONTRACT_MISMATCH"),
        ("Authorization: Bearer invalid\r\nAuthorization: Bearer invalid\r\n".into(), "{}", "BAD_REQUEST"),
        ("".into(), "{", "BAD_REQUEST"),
    ];
    for (extra, body, code) in cases {
        let (headers, response) = request(
            server.address,
            &format!("POST /read HTTP/1.1\r\n{extra}Host: {}\r\nOrigin: {origin}\r\nContent-Length: {}\r\n", server.address, body.len()),
            body,
        )
        .await;
        assert_eq!(response["code"], code);
        assert!(headers.contains(&format!("access-control-allow-origin: {origin}")), "{code}: {headers}");
    }
    let (headers, response) = request(
        server.address,
        &format!("POST /read HTTP/1.1\r\nHost: {}\r\nOrigin: {origin}\r\nAuthorization: Bearer {}\r\n", server.address, "x".repeat(1025)),
        "",
    )
    .await;
    assert_eq!(response["code"], "BAD_REQUEST");
    assert!(headers.contains(&format!("access-control-allow-origin: {origin}")), "oversized token: {headers}");
    for headers in [
        format!("Host: attacker.example:{}\r\nOrigin: http://attacker.example:{}\r\n", server.address.port(), server.address.port()),
        format!("Host: {}\r\nOrigin: {origin}\r\nOrigin: {origin}\r\n", server.address),
        "Origin: null\r\n".to_string(),
        format!("Origin: {origin}\r\nHost: {}\r\nHost: {}\r\n", server.address, server.address),
    ] {
        let (head, body) = request(server.address, &format!("POST /apply HTTP/1.1\r\n{headers}Content-Length: 999\r\n"), "").await;
        assert_eq!(body["code"], "ORIGIN_NOT_ALLOWED");
        assert!(!head.contains("access-control-allow-origin"));
    }
    server.shutdown().await.unwrap();
}
