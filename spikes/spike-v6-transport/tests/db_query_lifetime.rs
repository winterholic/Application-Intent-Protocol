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
        atomic::{AtomicBool, AtomicUsize, Ordering},
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

// Authentication is relayed to the real local PostgreSQL. Only post-auth SQL is held.
async fn hold_sql(socket: TcpStream, held: Arc<AtomicUsize>, closed: Arc<AtomicUsize>) {
    proxy_sql(socket, held, closed, false).await
}

async fn proxy_sql(socket: TcpStream, held: Arc<AtomicUsize>, closed: Arc<AtomicUsize>, after_clock: bool) {
    let db = TcpStream::connect("127.0.0.1:5432").await.unwrap();
    let (mut input, mut output) = socket.into_split();
    let (mut db_input, mut db_output) = db.into_split();
    let ready = Arc::new(AtomicBool::new(false));
    let ready_out = ready.clone();
    let backend = async move {
        let mut clock_row = false;
        loop {
            let kind = db_input.read_u8().await?;
            let length = db_input.read_u32().await?;
            if !(4..=1_048_576).contains(&length) {
                return Err(std::io::Error::other("invalid PostgreSQL packet"));
            }
            let mut bytes = vec![0; (length - 4) as usize];
            db_input.read_exact(&mut bytes).await?;
            if kind == b'D' {
                clock_row = true;
            }
            if kind == b'Z' && (!after_clock || clock_row) {
                ready_out.store(true, Ordering::SeqCst);
            }
            output.write_u8(kind).await?;
            output.write_u32(length).await?;
            output.write_all(&bytes).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    };
    let frontend = async move {
        let mut seen = false;
        let mut bytes = [0; 8192];
        loop {
            let n = input.read(&mut bytes).await?;
            if n == 0 {
                closed.fetch_add(1, Ordering::SeqCst);
                return Ok::<(), std::io::Error>(());
            }
            if ready.load(Ordering::SeqCst) {
                if !seen {
                    held.fetch_add(1, Ordering::SeqCst);
                    seen = true;
                }
            } else {
                db_output.write_all(&bytes[..n]).await?;
            }
        }
    };
    tokio::select! { _=frontend=>{}, _=backend=>{} }
}

#[tokio::test]
async fn silent_operation_sql_after_clock_respects_declared_deadlines_and_returns_slots() {
    let path = std::env::temp_dir().join(format!(
        "aip-db-operation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    fs::write(path.join("write.mjs"), "export async function run(){throw new Error('transaction must finish starting before worker');}\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let held = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let (h, c) = (held.clone(), closed.clone());
    let proxy = tokio::spawn(async move {
        let mut peers = JoinSet::new();
        loop {
            tokio::select! {
                socket=listener.accept()=>{let(socket,_)=socket.unwrap();peers.spawn(proxy_sql(socket,h.clone(),c.clone(),true));}
                _=peers.join_next(),if !peers.is_empty()=>{}
            }
        }
    });
    let facts = load_str(include_str!("fixtures/write-extension.aip"), Form::A).unwrap().execution;
    let server = listen_with_prototype_options(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        format!("host=127.0.0.1 port={} dbname=postgres", address.port()),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: path.clone() }),
        ServerOptions::default(),
        true,
    )
    .await
    .unwrap();
    let token = server.keys.issue(1, 60);
    let fp = contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let mut calls = vec![];
    for i in 0..7 {
        let (a, t, f) = (server.address, token.clone(), fp.clone());
        calls.push(tokio::spawn(async move {
            let (path, body, expected) = match i {
                4 => ("/read", json!({"query":{"read":"Event","select":["id"]}}), "DEADLINE_EXCEEDED"),
                5 => ("/status", json!({"key":"held","request":{"extension":"Event.run","input":{"id":11}}}), "DB_UNAVAILABLE"),
                6 => ("/apply", json!({"key":"standard","request":{"apply":"Event.mark","target":{"ids":[11]}}}), "DEADLINE_EXCEEDED"),
                _ => ("/apply", json!({"key":format!("held-{i}"),"request":{"extension":"Event.run","input":{"id":11}}}), "DEADLINE_EXCEEDED"),
            };
            (post(a, &t, &f, path, body).await, expected)
        }));
    }
    let mut failures = vec![];
    if !reaches(&held, 7).await {
        failures.push("clock query did not finish before seven held SQL operations".to_string());
    }
    let body = json!({"key":"retry","request":{"extension":"Event.run","input":{"id":11}}});
    let overflow = post(server.address, &token, &fp, "/apply", body.clone()).await;
    if !matches!(&overflow,Ok(v) if v["code"]=="WORKER_BUSY") {
        failures.push(format!("overflow: {overflow:?}"));
    }
    for call in calls {
        let (response, expected) = call.await.unwrap();
        if !matches!(&response,Ok(v) if v["code"]==expected) {
            failures.push(format!("after clock expected {expected}: {response:?}"));
        }
    }
    if !reaches(&closed, 7).await {
        failures.push("operation timeout did not close DB driver sockets".to_string());
    }
    let retry = post(server.address, &token, &fp, "/apply", body).await;
    if !matches!(&retry,Ok(v) if v["code"]=="DEADLINE_EXCEEDED") {
        failures.push(format!("operation did not return extension slot: {retry:?}"));
    }
    if !reaches(&closed, 8).await {
        failures.push("retry operation kept a DB socket alive".to_string());
    }
    server.shutdown().await.unwrap();
    proxy.abort();
    let _ = proxy.await;
    fs::remove_dir_all(path).unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

async fn reaches(value: &AtomicUsize, count: usize) -> bool {
    timeout(Duration::from_secs(2), async {
        while value.load(Ordering::SeqCst) < count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}
async fn post(address: SocketAddr, token: &str, fp: &str, path: &str, body: Value) -> Result<Value, String> {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.map_err(|e| e.to_string())?;
    socket.write_all(format!("POST {path} HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.map_err(|e|e.to_string())?;
    let mut bytes = vec![];
    timeout(Duration::from_secs(7), socket.read_to_end(&mut bytes))
        .await
        .map_err(|_| "SQL response remained blocked".to_string())?
        .map_err(|e| e.to_string())?;
    let response = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    serde_json::from_str(response.split_once("\r\n\r\n").ok_or("missing body")?.1).map_err(|e| e.to_string())
}

#[tokio::test]
async fn authenticated_but_silent_sql_releases_slots_and_request_owned_connections() {
    let path = std::env::temp_dir().join(format!(
        "aip-db-query-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    fs::write(path.join("write.mjs"), "export async function run(){throw new Error('SQL preflight must finish before worker');}\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let held = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let (h, c) = (held.clone(), closed.clone());
    let proxy = tokio::spawn(async move {
        let mut peers = JoinSet::new();
        loop {
            tokio::select! {
                socket=listener.accept()=>{let(socket,_)=socket.unwrap();peers.spawn(hold_sql(socket,h.clone(),c.clone()));}
                _=peers.join_next(),if !peers.is_empty()=>{}
            }
        }
    });
    let facts = load_str(include_str!("fixtures/write-extension.aip"), Form::A).unwrap().execution;
    let server = listen_with_prototype_options(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        format!("host=127.0.0.1 port={} dbname=postgres", address.port()),
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
        let (a, t, f) = (server.address, token.clone(), fp.clone());
        calls.push(tokio::spawn(async move {
            if i == 4 {
                post(a, &t, &f, "/read", json!({"read":"Event","select":["id"]})).await
            } else {
                post(a, &t, &f, "/apply", json!({"key":format!("held-{i}"),"request":{"extension":"Event.run","input":{"id":11}}})).await
            }
        }));
    }
    let mut failures = vec![];
    if !reaches(&held, 5).await {
        failures.push("five connections did not authenticate and send SQL".to_string());
    }
    let body = json!({"key":"retry","request":{"extension":"Event.run","input":{"id":11}}});
    let overflow = post(server.address, &token, &fp, "/apply", body.clone()).await;
    if !matches!(&overflow,Ok(v) if v["code"]=="WORKER_BUSY") {
        failures.push(format!("overflow: {overflow:?}"));
    }
    for call in calls {
        let response = call.await.unwrap();
        if !matches!(&response,Ok(v) if v["code"]=="DB_UNAVAILABLE") {
            failures.push(format!("SQL wait: {response:?}"));
        }
    }
    if !reaches(&closed, 5).await {
        failures.push("SQL timeout left DB driver sockets open".to_string());
    }
    let retry = post(server.address, &token, &fp, "/apply", body).await;
    if !matches!(&retry,Ok(v) if v["code"]=="DB_UNAVAILABLE") {
        failures.push(format!("slot not returned: {retry:?}"));
    }
    if !reaches(&closed, 6).await {
        failures.push("retry SQL driver socket stayed open".to_string());
    }
    let (a, t, f) = (server.address, token.clone(), fp.clone());
    let pending = tokio::spawn(async move { post(a, &t, &f, "/read", json!({"read":"Event","select":["id"]})).await });
    if !reaches(&held, 7).await {
        failures.push("shutdown SQL request did not authenticate".to_string());
    }
    if timeout(Duration::from_secs(2), server.shutdown()).await.is_err() {
        failures.push("server shutdown remained blocked".to_string());
    }
    if !reaches(&closed, 7).await {
        failures.push("shutdown left detached DB driver alive".to_string());
    }
    let _ = pending.await;
    proxy.abort();
    let _ = proxy.await;
    fs::remove_dir_all(path).unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
