use serde_json::{json, Value};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::listen_with_apply_contract;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

fn facts() -> Value {
    spike_v1_fixture::load_str(include_str!("../../../prototype/example/app.aip"), spike_v1_fixture::Form::A).unwrap().execution
}

const BAD_DB: &str = "host=127.0.0.1 port=1 dbname=postgres password=never_print_me connect_timeout=1";

async fn post(address: SocketAddr, path: &str, token: &str, contract: Option<&str>, fault: bool) -> Value {
    post_body(address, path, token, contract, fault, "{}").await
}

async fn post_body(address: SocketAddr, path: &str, token: &str, contract: Option<&str>, fault: bool, body: &str) -> Value {
    let mut sock = TcpStream::connect(address).await.unwrap();
    let extra = contract.map(|c| format!("x-aip-contract: {c}\r\n")).unwrap_or_default();
    let extra = format!("{extra}{}", if fault { "x-spike-drop-response: 1\r\n" } else { "" });
    sock.write_all(
        format!("POST {path} HTTP/1.1\r\nauthorization: Bearer {token}\r\n{extra}content-length: {}\r\n\r\n{body}", body.len()).as_bytes(),
    )
    .await
    .unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(3), sock.read_to_string(&mut response)).await.unwrap().unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").expect("HTTP response, including when fault header is supplied").1).unwrap()
}

#[tokio::test]
async fn ordinary_server_ignores_fault_header_after_a_successful_database_read() {
    use spike_v2_read::sqlgen;
    let schema = format!("aip_proto_server_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = spike_v2_read::connect().await;
    for statement in sqlgen::create_ddl(&facts()).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES(11,1,false,'title','link','READY','2026-10-04Z')")).await.unwrap();
    let server =
        listen_with_apply_contract(facts(), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), spike_v2_read::DB_URL.into()).await.unwrap();
    let token = server.keys.issue(1, 60);
    let address = server.address;
    let request = tokio::spawn(async move { post_body(address, "/read", &token, None, true, r#"{"query":{"read":"Event","select":["id"]}}"#).await });
    let outcome = request.await;
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    let result = outcome.expect("ordinary server must return HTTP even when a fault header is supplied");
    assert_eq!(result["ok"], true);
    assert_eq!(result["rows"], json!([{"id":"11"}]));
}

#[tokio::test]
async fn configured_database_and_authentication_order_are_observable() {
    let server = listen_with_apply_contract(facts(), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), BAD_DB.into()).await.unwrap();
    let token = server.keys.issue(1, 60);
    assert_eq!(post(server.address, "/session", &token, None, false).await["principal"]["actorId"], "1");
    assert_eq!(post(server.address, "/read", "invalid", None, false).await["code"], "UNAUTHENTICATED");
    assert_eq!(post(server.address, "/read", &token, Some(&"0".repeat(64)), false).await["code"], "CONTRACT_MISMATCH");
    let result = post(server.address, "/read", &token, None, true).await;
    assert_eq!(result["code"], "DB_UNAVAILABLE", "configured failing database cannot silently use the default database");
    assert!(!result.to_string().contains("never_print_me"));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn chosen_port_conflict_and_shutdown_close_listener_and_incomplete_request() {
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap();
    assert!(listen_with_apply_contract(facts(), IdWire::SafeNumber, address, BAD_DB.into()).await.is_err());
    drop(reserved);
    let server = listen_with_apply_contract(facts(), IdWire::SafeNumber, address, BAD_DB.into()).await.unwrap();
    assert_eq!(server.address, address);
    let mut sock = TcpStream::connect(address).await.unwrap();
    sock.write_all(b"POST /read HTTP/1.1\r\ncontent-length: 40\r\n\r\n{").await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    server.shutdown().await.unwrap();
    assert!(TcpStream::connect(address).await.is_err());
    let mut buffer = vec![];
    let end = timeout(Duration::from_secs(1), sock.read_to_end(&mut buffer)).await.expect("request task must terminate on shutdown");
    assert!(end.is_err() || end.unwrap() == 0, "incomplete request must not produce a successful response");
}

#[tokio::test]
async fn configured_server_rejects_non_loopback_addresses() {
    assert!(listen_with_apply_contract(json!({}), IdWire::SafeNumber, "0.0.0.0:0".parse().unwrap(), BAD_DB.into()).await.is_err());
}
