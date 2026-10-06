//! `AIP.UNAVAILABLE` is a transient failure of a dependency, so it answers 503 and
//! tells the caller it may retry. The database is replaced by a closed port, so
//! the pool cannot hand out a connection.

use aip_runtime::engine::{Call, Engine};
use aip_runtime::http::{AppState, serve};
use aip_runtime::objects::ObjectStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn engine_without_database() -> Arc<Engine> {
    let path = format!("{}/../../examples/shop/app.aip", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(path).expect("shop source");
    let (core, map) = aip_sema::pipeline::check_source(&src).into_core().expect("shop checks clean");
    let compiled = aip_pg::compile(&core, &map);
    let pool = aip_runtime::pool("postgres://127.0.0.1:1/none", 1).expect("pool");
    Arc::new(Engine {
        program: Arc::new(compiled.program),
        pool,
        objects: ObjectStore::new(std::env::temp_dir().join("aip-unavailable-objects")),
        secret: Vec::new(),
        outbound: Default::default(),
        keys: None,
    })
}

#[tokio::test]
async fn unavailable_database_is_a_retryable_error() {
    let e = engine_without_database();
    let err = e.call(Call { intent: "Catalog".into(), input: Value::Null, ..Default::default() }).await.expect_err("no database");
    assert_eq!(err.code, aip_ir::codes::UNAVAILABLE);
    assert_eq!(err.status(), 503);
    assert!(err.retryable);
}

#[tokio::test]
async fn unavailable_database_answers_http_503() {
    let engine = engine_without_database();
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let state = AppState::new(engine, Arc::new(b"test".to_vec()), false, false, Arc::new(json!({})), "postgres://localhost/postgres").await;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tokio::spawn(async move {
        let _ = serve(state, addr).await;
    });
    let mut stream = None;
    for _ in 0..50 {
        if let Ok(s) = tokio::net::TcpStream::connect(addr).await {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let mut stream = stream.expect("server did not start");
    stream.write_all(b"POST /aip/Catalog HTTP/1.1\r\nhost: localhost\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.expect("send");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.expect("read");
    assert!(raw.starts_with("HTTP/1.1 503"), "{raw}");
    assert!(raw.contains("AIP.UNAVAILABLE"), "{raw}");
}
