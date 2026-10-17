//! O-21: the heartbeat of a running job must survive a short database outage. A migration that holds the deployment
//! lock for a few seconds makes one heartbeat fail; that must not kill the worker (which would restart the job from
//! scratch, or leave an in-flight HTTP effect permanently unknown).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    server::{listen_with_authenticator_and_deployment, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

struct Auth {
    schema: String,
    iat: i64,
}
impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let grant = DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: 1, issued_at: self.iat, min_iat: 0 };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.iat + 600)
        })
    }
    fn verify_durable<'a>(
        &'a self,
        g: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.map_err(|_| AuthError::Unavailable)?;
            let row = db
                .query_opt(
                    &format!("SELECT enabled,min_iat FROM {}.aip_principals WHERE issuer=$1 AND subject=$2 AND actor_id=$3", self.schema),
                    &[&g.issuer, &g.subject, &g.actor_id],
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            if row.is_some_and(|r| r.get::<_, bool>(0) && r.get::<_, i64>(1) == g.min_iat) {
                Ok(())
            } else {
                Err(AuthError::BadSignature)
            }
        })
    }
}

async fn post(addr: std::net::SocketAddr, path: &str, token: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_database_outage_does_not_restart_a_running_job() {
    let schema = format!("aip_heartbeat_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let server = listen_with_authenticator_and_deployment(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::test_database_url!().into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), iat }),
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
    )
    .await
    .unwrap();

    let submitted =
        post(server.address, "/jobs/enqueue", "owner", &fp, json!({"operation":"slow","input":{},"key":"heartbeat-outage","delayMs":0})).await;
    let id = submitted["run"]["id"].as_str().unwrap().to_owned();
    // The worker sleeps for 5 seconds. Hold the deployment lock exclusively for 3: the heartbeat's shared lock waits 2s
    // (its lock_timeout) and fails once, exactly what a migration in progress does.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let blocker = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    blocker.query_one("SELECT pg_advisory_lock(1095323725,hashtext($1))", &[&schema]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(3000)).await;
    blocker.query_one("SELECT pg_advisory_unlock(1095323725,hashtext($1))", &[&schema]).await.unwrap();

    let mut state = Value::Null;
    for _ in 0..200 {
        state = post(server.address, "/jobs/status", "owner", &fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&state["run"]["state"].as_str().unwrap_or("")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let attempts: i32 = db.query_one(&format!("SELECT attempt FROM {schema}.aip_jobs WHERE id::text=$1"), &[&id]).await.unwrap().get(0);
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert_eq!(state["run"]["state"], "SUCCEEDED", "{state}");
    assert_eq!(attempts, 1, "one failed heartbeat restarted the job from scratch");
}
