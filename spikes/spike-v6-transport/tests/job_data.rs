//! Durable jobs reach business data only through declared read/apply capabilities and the job principal's policies.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    server::{listen_with_managed, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        let _ = cleanup.join();
    }
}

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
            let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.map_err(|_| AuthError::Unavailable)?;
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

async fn post(addr: std::net::SocketAddr, path: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer owner\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().split_once("\r\n\r\n").unwrap().1).unwrap()
}

async fn settle(addr: std::net::SocketAddr, fp: &str, id: &str) -> Value {
    for _ in 0..200 {
        let status = post(addr, "/jobs/status", fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&status["run"]["state"].as_str().unwrap_or("")) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_jobs_read_and_apply_business_data_with_principal_policies_and_atomic_journal() {
    let schema = format!("aip_job_data_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    assert_eq!(facts["operations"]["settle"]["access"], json!({"read.Order":true,"apply.Order.pay":true}));
    assert_eq!(facts["operations"]["settle"]["dependencies"]["worker"], json!(["database"]));
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        db.batch_execute(&format!(
            "DELETE FROM {schema}.\"order\"; INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(11,1,false),(12,1,false),(99,2,false)"
        ))
        .await
        .unwrap();
        let server = listen_with_managed(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            spike_v2_read::DB_URL.into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-data").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            None,
            false,
        )
        .await
        .unwrap();
        let addr = server.address;
        let label = format!("{lang:?}").to_lowercase();
        let queued =
            post(addr, "/jobs/enqueue", &fp, json!({"operation":"settle","input":{"other":99},"key":format!("settle-{label}"),"delayMs":0})).await;
        let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("{lang:?} enqueue: {queued}")).to_string();
        let status = settle(addr, &fp, &id).await;
        assert_eq!(status["run"]["state"], "SUCCEEDED", "{lang:?}: {status}");
        let result = post(addr, "/jobs/result", &fp, json!({"id":id})).await;
        let output = &result["output"];
        // Row policy hides member 2's order from both the read and the transition.
        assert_eq!(output["visible"], 2, "{lang:?}: {result}");
        assert_eq!(output["paid"], 2, "{lang:?}: {result}");
        // Same code as a direct /apply on an invisible row, so the job learns nothing about rows its principal cannot see.
        assert_eq!(output["denied"], "WIRE.MISSING_TARGET", "{lang:?}: {result}");
        // The repeated key replays the journaled result instead of re-running a now-invalid transition.
        assert_eq!(output["replayed"], true, "{lang:?}: {result}");
        let paid: Vec<(i64, bool)> = db
            .query(&format!("SELECT id,paid FROM {schema}.\"order\" ORDER BY id"), &[])
            .await
            .unwrap()
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();
        assert_eq!(paid, vec![(11, true), (12, true), (99, false)], "{lang:?}");
        let journal: Vec<(String, String)> = db
            .query(&format!("SELECT effect_key,state FROM {schema}.aip_effects WHERE run_id=$1::text::uuid ORDER BY effect_key"), &[&id])
            .await
            .unwrap()
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();
        assert_eq!(journal.len(), 3, "{lang:?}: {journal:?}");
        assert_eq!(journal.iter().filter(|(_, state)| state == "SUCCEEDED").count(), 2, "{lang:?}: {journal:?}");
        assert_eq!(journal.iter().filter(|(_, state)| state == "FAILED").count(), 1, "{lang:?}: {journal:?}");

        // A job without the transition capability cannot write, even for rows its principal could change.
        db.batch_execute(&format!("UPDATE {schema}.\"order\" SET paid=false WHERE id=11")).await.unwrap();
        let queued = post(addr, "/jobs/enqueue", &fp, json!({"operation":"sneaky","input":{},"key":format!("sneaky-{label}"),"delayMs":0})).await;
        let id = queued["run"]["id"].as_str().unwrap().to_string();
        let status = settle(addr, &fp, &id).await;
        assert_eq!(status["run"]["state"], "FAILED", "{lang:?}: {status}");
        assert_eq!(status["run"]["error"], "WIRE.ACCESS_NOT_DECLARED", "{lang:?}: {status}");
        let changed: bool = db.query_one(&format!("SELECT paid FROM {schema}.\"order\" WHERE id=11"), &[]).await.unwrap().get(0);
        assert!(!changed, "{lang:?}: undeclared apply changed data");

        // A worker crash after a committed transition retries the job; the retry replays the journaled result.
        db.batch_execute(&format!("UPDATE {schema}.\"order\" SET paid=false WHERE id=11")).await.unwrap();
        let queued = post(addr, "/jobs/enqueue", &fp, json!({"operation":"crashy","input":{},"key":format!("crashy-{label}"),"delayMs":0})).await;
        let id = queued["run"]["id"].as_str().unwrap().to_string();
        let status = settle(addr, &fp, &id).await;
        assert_eq!(status["run"]["state"], "SUCCEEDED", "{lang:?}: {status}");
        assert_eq!(status["run"]["attempt"], 2, "{lang:?}: {status}");
        let result = post(addr, "/jobs/result", &fp, json!({"id":id})).await;
        assert_eq!(result["output"], json!({"seenPaid":true,"changed":1}), "{lang:?}: {result}");
        server.shutdown().await.unwrap();
    }
}
