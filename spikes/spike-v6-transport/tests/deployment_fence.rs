use serde_json::{json, Value};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, Session},
    server::{listen_with_authenticator_and_deployment, DeploymentFence, ServerOptions},
};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Provider;

impl Authenticator for Provider {
    fn verify_session<'a>(&'a self, _: &'a str) -> Pin<Box<dyn Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async { Session::new(1, i64::MAX) })
    }
}

async fn status(address: std::net::SocketAddr) -> Value {
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect HTTP server");
    let body = r#"{"key":"missing","request":{}}"#;
    socket
        .write_all(format!("POST /status HTTP/1.1\r\nauthorization: Bearer test\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .expect("send status");
    let mut response = String::new();
    socket.read_to_string(&mut response).await.expect("read status");
    serde_json::from_str(response.split_once("\r\n\r\n").expect("HTTP response").1).expect("JSON response")
}

#[tokio::test]
async fn migration_lock_linearizes_requests_and_old_listener_rejects_new_facts() {
    let schema =
        format!("aip_fence_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos());
    sqlgen::try_set_schema(&schema).expect("test schema context");
    let db_url = "host=localhost dbname=postgres";
    let db = connect_owned_with_url(db_url).await.expect("local PostgreSQL");
    db.batch_execute(&format!(
        "CREATE SCHEMA {schema}; CREATE TABLE {schema}.aip_migrate_meta(singleton boolean PRIMARY KEY, facts_digest text NOT NULL); INSERT INTO {schema}.aip_migrate_meta VALUES(true,'{old}'); CREATE TABLE {schema}.aip_idem(principal text,key text,request text,result jsonb,PRIMARY KEY(principal,key))",
        old = "a".repeat(64)
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
        Some(DeploymentFence::new(schema.clone(), "a".repeat(64)).expect("fence")),
    )
    .await
    .expect("server");
    assert_eq!(status(server.address).await["code"], "NOT_FOUND");

    let migration = connect_owned_with_url(db_url).await.expect("migration connection");
    migration.batch_execute("BEGIN; SET LOCAL lock_timeout='2s'").await.expect("migration transaction");
    migration.query_one("SELECT pg_advisory_xact_lock(1095323725,hashtext($1))", &[&schema]).await.expect("exclusive deployment lock");
    migration
        .execute(&format!("UPDATE {schema}.aip_migrate_meta SET facts_digest=$1 WHERE singleton=true"), &[&"b".repeat(64)])
        .await
        .expect("new deployment marker");
    let request = tokio::spawn(status(server.address));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!request.is_finished(), "request must wait behind the active migration");
    migration.batch_execute("COMMIT").await.expect("publish new deployment");
    assert_eq!(request.await.expect("status task")["code"], "DEPLOYMENT_CHANGED");
    assert_eq!(status(server.address).await["code"], "DEPLOYMENT_CHANGED");
    server.shutdown().await.expect("server shutdown");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.expect("owned schema cleanup");
}
