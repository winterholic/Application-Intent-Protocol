//! O-20: a principal's retained job input/output bytes are bounded like files (10 MiB) and messages (64 MiB).
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

async fn post(addr: std::net::SocketAddr, path: &str, token: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    stream
        .write_all(
            format!("POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    serde_json::from_str(String::from_utf8(out).unwrap().split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_job_payload_bytes_are_bounded_per_principal() {
    let schema = format!("aip_job_quota_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let source = r#"
actor principal
operation job bulk {
  input { text: Text }
  output { length: Int }
  allow actor != null
  effect none
  deadline 10s
  implementation "jobs.count"
}"#;
    let facts = load_str(source, Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','alice',1),('test','bob',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let server = listen_with_managed(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        auth,
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        None,
        false,
    )
    .await
    .unwrap();
    let big = "a".repeat(900_000);
    let mut accepted = 0;
    let mut refused = None;
    for index in 0..90 {
        // A day of delay keeps every job queued, so its input stays retained.
        let response = post(
            server.address,
            "/jobs/enqueue",
            "alice",
            &fp,
            json!({"operation":"bulk","input":{"text":big},"key":format!("bulk-{index}"),"delayMs":86_400_000}),
        )
        .await;
        if response["ok"] == true {
            accepted += 1;
        } else {
            refused = Some(response["code"].as_str().unwrap_or("").to_string());
            break;
        }
    }
    assert_eq!(refused.as_deref(), Some("QUOTA_EXCEEDED"), "{accepted} jobs of 900 kB were accepted without a byte quota");
    // 64 MiB of retained payload / 900 kB.
    assert!((70..=75).contains(&accepted), "accepted {accepted}");
    // A replay of an accepted key is not new retention.
    let replay =
        post(server.address, "/jobs/enqueue", "alice", &fp, json!({"operation":"bulk","input":{"text":big},"key":"bulk-0","delayMs":86_400_000}))
            .await;
    assert_eq!(replay["ok"], true, "{replay}");
    // The quota is per principal.
    let other =
        post(server.address, "/jobs/enqueue", "bob", &fp, json!({"operation":"bulk","input":{"text":big},"key":"bulk-0","delayMs":86_400_000})).await;
    assert_eq!(other["ok"], true, "{other}");
    server.shutdown().await.unwrap();
}
