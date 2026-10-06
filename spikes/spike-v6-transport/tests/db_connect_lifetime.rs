#![cfg(target_os = "macos")]
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::server::{listen_with_prototype_options, ReadExtensions, ServerOptions, WorkerLang};
use std::{
    fs,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{sleep, timeout},
};

async fn post(address: SocketAddr, token: &str, fp: &str, path: &str, body: Value) -> Result<Value, String> {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.map_err(|e| e.to_string())?;
    socket.write_all(format!("POST {path} HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.map_err(|e|e.to_string())?;
    let mut bytes = vec![];
    timeout(Duration::from_secs(7), socket.read_to_end(&mut bytes))
        .await
        .map_err(|_| "HTTP response remained blocked".to_string())?
        .map_err(|e| e.to_string())?;
    let response = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    serde_json::from_str(response.split_once("\r\n\r\n").ok_or("missing HTTP body")?.1).map_err(|e| e.to_string())
}
async fn count_reaches(value: &AtomicUsize, count: usize) -> bool {
    timeout(Duration::from_secs(1), async {
        while value.load(Ordering::SeqCst) < count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn silent_database_handshake_releases_requests_sockets_and_extension_slots() {
    let path = std::env::temp_dir().join(format!(
        "aip-db-connect-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&path).expect("fresh worker directory");
    fs::write(path.join("write.mjs"), "export async function run(){throw new Error('worker must not start before DB connection');}\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let db_address = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let (a, c) = (accepted.clone(), closed.clone());
    let silent_db = tokio::spawn(async move {
        let mut peers = JoinSet::new();
        loop {
            tokio::select! {
                connection=listener.accept()=>{
                    let (mut socket,_)=connection.unwrap();a.fetch_add(1,Ordering::SeqCst);let c=c.clone();
                    peers.spawn(async move {
                        let mut bytes=vec![];
                        if socket.read_to_end(&mut bytes).await.is_ok(){c.fetch_add(1,Ordering::SeqCst);}
                    });
                }
                _=peers.join_next(),if !peers.is_empty()=>{}
            }
        }
    });
    let facts = load_str(include_str!("fixtures/write-extension.aip"), Form::A).unwrap().execution;
    let server = listen_with_prototype_options(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        format!("host=127.0.0.1 port={} dbname=postgres", db_address.port()),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: path.clone() }),
        ServerOptions::default(),
        true,
    )
    .await
    .unwrap();
    let token = server.keys.issue(1, 60);
    let fp = contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let mut calls = vec![];
    for i in 0..5 {
        let (address, t, f) = (server.address, token.clone(), fp.clone());
        calls.push(tokio::spawn(async move {
            if i == 4 {
                post(address, &t, &f, "/read", json!({"read":"Event","select":["id"]})).await
            } else {
                post(address, &t, &f, "/apply", json!({"key":format!("held-{i}"),"request":{"extension":"Event.run","input":{"id":11}}})).await
            }
        }));
    }
    let mut failures = vec![];
    if !count_reaches(&accepted, 5).await {
        failures.push("five requests did not reach the test-owned silent database".to_string());
    }
    let body = json!({"key":"overflow","request":{"extension":"Event.run","input":{"id":11}}});
    let overflow = post(server.address, &token, &fp, "/apply", body.clone()).await;
    if !matches!(&overflow,Ok(value) if value["code"]=="WORKER_BUSY") {
        failures.push(format!("overflow: {overflow:?}"));
    }
    for call in calls {
        let result = call.await.unwrap();
        if !matches!(&result,Ok(value) if value["code"]=="DB_UNAVAILABLE") {
            failures.push(format!("silent handshake: {result:?}"));
        }
    }
    if !count_reaches(&closed, 5).await {
        failures.push("timed-out database handshake sockets did not close".to_string());
    }
    let retry = post(server.address, &token, &fp, "/apply", body).await;
    if !matches!(&retry,Ok(value) if value["code"]=="DB_UNAVAILABLE") {
        failures.push(format!("slot was not returned: {retry:?}"));
    }
    if !count_reaches(&closed, 6).await {
        failures.push("retry handshake socket did not close".to_string());
    }
    server.shutdown().await.unwrap();
    silent_db.abort();
    let _ = silent_db.await;
    for entry in fs::read_dir(&path).unwrap() {
        if entry.unwrap().file_name() != "write.mjs" {
            failures.push("unexpected worker artifact before connection".to_string());
        }
    }
    fs::remove_dir_all(path).unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
