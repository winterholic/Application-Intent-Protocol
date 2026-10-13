//! O-19: operations that server triggers target cannot be enqueued directly by users.
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
    triggers::{ServicePrincipal, Signature, Triggers, Webhook},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const SECRET: &[u8] = b"test-webhook-secret-0123456789";

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

async fn raw(addr: std::net::SocketAddr, path: &str, headers: &[(&str, String)], body: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    stream
        .write_all(format!("POST {path} HTTP/1.1\r\nhost: localhost\r\n{extra}content-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap())
}

fn signed(body: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(SECRET).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// O-19: the operation a webhook, schedule, outbox or message handler targets takes its input from a verified source
/// (HMAC-signed body, configured input, committed outbox row). A user who can pass `allow` must not enqueue it directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trigger_target_operations_cannot_be_enqueued_directly_by_users() {
    let schema = format!("aip_trigger_guard_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/triggers.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','svc-hooks',1),('test','alice',1); INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(11,1,false)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let triggers = Arc::new(Triggers {
        schedules: vec![],
        webhooks: vec![Webhook {
            name: "orders".into(),
            path: "/hooks/orders".into(),
            operation: "receive".into(),
            principal: ServicePrincipal { issuer: "test".into(), subject: "svc-hooks".into() },
            secret: SECRET.to_vec(),
            signature: Signature::HmacSha256 { header: "x-hub-signature-256".into(), prefix: "sha256=".into(), base64: false },
            max_body_bytes: 4096,
        }],
        outbox: vec![],
        messages: vec![],
    });
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-data").into() }),
        ServerOptions::default(),
        false,
        auth,
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        None,
        false,
        Infrastructure { triggers, ..Default::default() },
    )
    .await
    .unwrap();
    let enqueue = |operation: &'static str, key: &'static str| {
        let fp = fp.clone();
        let addr = server.address;
        async move {
            let input = if operation == "receive" { json!({"body":"{\"order\":11}"}) } else { json!({}) };
            raw(
                addr,
                "/jobs/enqueue",
                &[("authorization", "Bearer alice".into()), ("x-aip-contract", fp)],
                &json!({"operation":operation,"input":input,"key":key,"delayMs":0}).to_string(),
            )
            .await
            .1
        }
    };
    // Control: an operation no trigger targets is still an ordinary user job.
    let control = enqueue("sweep", "plain-sweep").await;
    assert_eq!(control["ok"], true, "{control}");
    // The webhook's operation would run with a forged, unsigned body.
    let forged = enqueue("receive", "forged-webhook").await;
    assert_eq!((forged["ok"].as_bool(), forged["code"].as_str()), (Some(false), Some("WIRE.ACCESS_DENIED")), "{forged}");
    let jobs: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_jobs WHERE operation='receive'"), &[]).await.unwrap().get(0);
    assert_eq!(jobs, 0);
    // The signed path itself still works.
    let body = r#"{"order":11}"#;
    let (code, accepted) = raw(server.address, "/hooks/orders", &[("x-hub-signature-256", signed(body))], body).await;
    assert_eq!(code, 202, "{accepted}");
    server.shutdown().await.unwrap();
}
