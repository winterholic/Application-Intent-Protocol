//! O-10: a pooled database connection must go back to the pool before the response is written. A client that
//! stops reading its response must not keep a connection (and with a deployment fence, the shared migration lock).
use serde_json::Value;
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, Session},
    infrastructure::Infrastructure,
    pool::PoolOptions,
    server::{listen_with_infrastructure, ServerOptions},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

struct AnyToken;
impl Authenticator for AnyToken {
    fn verify_session<'a>(&'a self, _token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
            Session::new(1, now + 60)
        })
    }
}

fn facts() -> Value {
    spike_v1_fixture::load_str(include_str!("../../../prototype/example/app.aip"), spike_v1_fixture::Form::A).unwrap().execution
}

fn read_request(select: &str) -> String {
    let body = format!(r#"{{"query":{{"read":"Event","select":[{select}]}}}}"#);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts(), IdWire::DecimalString);
    format!("POST /read HTTP/1.1\r\nauthorization: Bearer any\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_does_not_read_its_response_does_not_hold_a_pool_connection() {
    let schema = format!("aip_resp_conn_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = spike_v2_read::connect().await;
    for statement in sqlgen::create_ddl(&facts()).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    // One row of nearly 1 MiB (the response cap) is more than a client with a tiny receive buffer takes, so writing
    // the response to a client that never reads blocks for the server's whole write timeout.
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) SELECT g,1,false,repeat('x',1000000),'link','READY','2026-10-04Z' FROM generate_series(1,1) g"
    ))
    .await
    .unwrap();

    let infrastructure = Infrastructure {
        db_pool: PoolOptions { max_size: 1, acquire_timeout: Duration::from_millis(800), idle_timeout: Duration::from_secs(60) },
        ..Infrastructure::default()
    };
    let server = listen_with_infrastructure(
        facts(),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(AnyToken),
        None,
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();

    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut stalled = socket.connect(server.address).await.unwrap();
    stalled.write_all(read_request(r#""id","title""#).as_bytes()).await.unwrap();
    // Let the server run the query and start writing into a client that is not reading.
    tokio::time::sleep(Duration::from_millis(700)).await;

    let mut other = TcpStream::connect(server.address).await.unwrap();
    other.write_all(read_request(r#""id""#).as_bytes()).await.unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(4), other.read_to_string(&mut response)).await.expect("second request answers").unwrap();
    let body: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["ok"], true, "the only pool connection was held while writing to a stalled client: {body}");
    assert_eq!(body["rows"].as_array().map(Vec::len), Some(1));

    drop(stalled);
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
