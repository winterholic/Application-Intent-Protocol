use serde_json::{json, Value};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, Session},
    server::{listen_with_authenticator_and_deployment, ServerOptions},
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Provider;

impl Authenticator for Provider {
    fn verify_session<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async { Session::new(1, i64::MAX) })
    }
}

async fn post(address: std::net::SocketAddr, path: &str, body: &Value) -> Value {
    let body = body.to_string();
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect HTTP server");
    socket
        .write_all(format!("POST {path} HTTP/1.1\r\nauthorization: Bearer test\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .expect("send");
    let mut response = String::new();
    socket.read_to_string(&mut response).await.expect("read");
    serde_json::from_str(response.split_once("\r\n\r\n").expect("HTTP response").1).expect("JSON response")
}

// NUL은 PostgreSQL text에 들어가지 못해 INTERNAL과 sqlstate를 응답에 노출했다. 제어 문자 key는 DB 전에 거부한다.
#[tokio::test]
async fn control_character_keys_are_rejected_before_the_database() {
    let schema =
        format!("aip_key_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos());
    sqlgen::try_set_schema(&schema).expect("test schema context");
    let db_url = "host=localhost dbname=postgres";
    let db = connect_owned_with_url(db_url).await.expect("local PostgreSQL");
    db.batch_execute(&format!(
        "CREATE SCHEMA {schema}; CREATE TABLE {schema}.aip_idem(principal text,key text,request text,result jsonb,PRIMARY KEY(principal,key))"
    ))
    .await
    .expect("owned schema setup");
    let server = listen_with_authenticator_and_deployment(
        json!({}),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().expect("address"),
        db_url.into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(Provider),
        None,
    )
    .await
    .expect("server");
    for key in ["a\u{0}b", "line\nbreak", "\u{1}", "tab\t"] {
        for path in ["/apply", "/status"] {
            let reply = post(server.address, path, &json!({"key": key, "request": {"apply": "X.y", "target": {"ids": ["1"]}}})).await;
            assert_eq!(reply["code"], "BAD_REQUEST", "{path} {key:?}: {reply}");
            assert!(!reply.to_string().contains("sqlstate"), "{reply}");
        }
    }
    // 한글·이모지 같은 일반 문자는 그대로 key로 쓸 수 있다.
    let reply = post(server.address, "/status", &json!({"key": "주문-🙂", "request": {}})).await;
    assert_eq!(reply["code"], "NOT_FOUND", "{reply}");
    drop(server);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.expect("drop test schema");
}
