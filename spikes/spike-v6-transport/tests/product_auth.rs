use serde_json::{json, Value};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, Session},
    server::{listen_with_authenticator, ServerOptions},
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Provider;
impl Authenticator for Provider {
    fn verify_session<'a>(&'a self, token: &'a str) -> Pin<Box<dyn Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            if token == "api-access-token" {
                Session::new(42, i64::MAX)
            } else if token == "offline" {
                Err(AuthError::Unavailable)
            } else {
                Err(AuthError::BadSignature)
            }
        })
    }
}

async fn session(address: std::net::SocketAddr, token: &str) -> Value {
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("POST /session HTTP/1.1\r\nauthorization: Bearer {token}\r\ncontent-length: 2\r\n\r\n{{}}").as_bytes()).await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn product_uses_only_the_injected_identity_provider() {
    let server = listen_with_authenticator(
        json!({}),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        "host=127.0.0.1 port=1".into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(Provider),
    )
    .await
    .unwrap();
    let dev_token = server.keys.issue(42, 60);
    assert_eq!(
        session(server.address, &dev_token).await["code"],
        "UNAUTHENTICATED",
        "product must not accept its otherwise valid development keyring"
    );
    assert_eq!(session(server.address, "api-access-token").await["principal"]["actorId"], "42");
    assert_eq!(session(server.address, "offline").await["code"], "DB_UNAVAILABLE");
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn product_accepts_canonical_https_frontend_origins() {
    let server = listen_with_authenticator(
        json!({}),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        "host=127.0.0.1 port=1".into(),
        None,
        ServerOptions { allowed_origins: vec!["https://app.example.com".into()] },
        false,
        Arc::new(Provider),
    )
    .await;
    assert!(server.is_ok(), "TLS frontend must be configurable");
    server.unwrap().shutdown().await.unwrap();
    for invalid in [
        "https://app.example.com/",
        "https://app.example.com:443",
        "https://user@app.example.com",
        "https://app.example.com?x=1",
        "https://*.example.com",
        "http://app.example.com",
    ] {
        assert!(ServerOptions { allowed_origins: vec![invalid.into()] }.validate_product().is_err(), "accepted {invalid}");
    }
}
