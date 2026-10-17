//! Job retry policy: the extension asks for a retry, the operator bounds attempts, and uncaught errors retry only by opt-in.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::triggers::JobPolicy;
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
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
                if let Ok(db) = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await {
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
    // Backoff between attempts is 1s then 2s, so allow well beyond three attempts with worker start-up.
    for _ in 0..400 {
        let status = post(addr, "/jobs/status", fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&status["run"]["state"].as_str().unwrap_or("")) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_retry_on_request_within_the_configured_attempts_and_uncaught_errors_only_by_opt_in() {
    let schema = format!("aip_job_retry_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-retry.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let policies: std::collections::HashMap<String, JobPolicy> = [
        ("flaky".to_string(), JobPolicy { max_attempts: 4, retry_uncaught: false }),
        ("boomRetry".to_string(), JobPolicy { max_attempts: 2, retry_uncaught: true }),
    ]
    .into();
    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            aip_test_support::test_database_url!().into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-retry").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            None,
            false,
            Infrastructure { job_policies: policies.clone(), ..Default::default() },
        )
        .await
        .unwrap();
        let addr = server.address;
        let label = format!("{lang:?}").to_lowercase();
        let run = |operation: &'static str| {
            let fp = fp.clone();
            let key = format!("{operation}-{label}");
            async move {
                let queued = post(addr, "/jobs/enqueue", &fp, json!({"operation":operation,"input":{},"key":key,"delayMs":0})).await;
                let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("{operation} enqueue: {queued}")).to_string();
                (id.clone(), settle(addr, &fp, &id).await)
            }
        };
        let ((flaky_id, flaky), (_, exhausted), (_, boom), (retry_id, boom_retry)) =
            tokio::join!(run("flaky"), run("exhausted"), run("boom"), run("boomRetry"));

        // `ctx.retryLater(reason, {afterMs})`: the extension chooses the wait, so the second attempt is not due after the
        // default one second of backoff but after the three seconds it asked for.
        let started = std::time::Instant::now();
        let queued = post(addr, "/jobs/enqueue", &fp, json!({"operation":"later","input":{},"key":format!("later-{label}"),"delayMs":0})).await;
        let later_id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("later enqueue: {queued}")).to_string();
        let mut wait_ms = None;
        for _ in 0..200 {
            let row = db
                .query_one(
                    &format!("SELECT state, attempt, (extract(epoch FROM next_run_at - clock_timestamp())*1000)::bigint FROM {schema}.aip_jobs WHERE id=$1::text::uuid"),
                    &[&later_id],
                )
                .await
                .unwrap();
            if row.get::<_, String>(0) == "QUEUED" && row.get::<_, i32>(1) >= 1 {
                wait_ms = Some(row.get::<_, i64>(2));
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let wait_ms = wait_ms.unwrap_or_else(|| panic!("{lang:?}: the first attempt never asked for a retry"));
        assert!((2_000..=3_000).contains(&wait_ms), "{lang:?}: the requested 3s wait, not the default 1s: {wait_ms}ms");
        let later = settle(addr, &fp, &later_id).await;
        assert_eq!(later["run"]["state"], "SUCCEEDED", "{lang:?}: {later}");
        assert_eq!(later["run"]["attempt"], 2, "{lang:?}: {later}");
        assert!(started.elapsed() >= Duration::from_millis(2_500), "{lang:?}: the second attempt waited for the requested time");

        // The extension saw its own run id and attempt number, and the policy allowed a third attempt.
        assert_eq!(flaky["run"]["state"], "SUCCEEDED", "{lang:?}: {flaky}");
        let result = post(addr, "/jobs/result", &fp, json!({"id":flaky_id})).await;
        assert_eq!(result["output"], json!({"attempt":3,"id":flaky_id}), "{lang:?}: {result}");
        // Without a policy the default three attempts bound a job that keeps asking.
        assert_eq!(exhausted["run"]["state"], "FAILED", "{lang:?}: {exhausted}");
        assert_eq!(exhausted["run"]["attempt"], 3, "{lang:?}: {exhausted}");
        assert_eq!(exhausted["run"]["error"], "WIRE.RETRY_LATER", "{lang:?}: {exhausted}");
        // An uncaught error is final unless the operator opted the job in.
        assert_eq!(boom["run"]["state"], "FAILED", "{lang:?}: {boom}");
        assert_eq!(boom["run"]["attempt"], 1, "{lang:?}: {boom}");
        assert_eq!(boom["run"]["error"], "WIRE.EXTENSION_ERROR", "{lang:?}: {boom}");
        assert_eq!(boom_retry["run"]["state"], "SUCCEEDED", "{lang:?}: {boom_retry}");
        let result = post(addr, "/jobs/result", &fp, json!({"id":retry_id})).await;
        assert_eq!(result["output"], json!({"attempt":2}), "{lang:?}: {result}");
        server.shutdown().await.unwrap();
    }
}
