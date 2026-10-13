//! O-31: a webhook body that fits the configured limit but is too large once it becomes a job input (JSON escaping
//! doubles every quote) can never succeed. The provider must get a permanent 4xx, not a 503 it will retry forever.
use hmac::{Hmac, Mac};
use serde_json::Value;
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

fn signed(body: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(SECRET).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())
}

async fn raw(addr: std::net::SocketAddr, path: &str, headers: &[(&str, String)], body: &str) -> (u16, String, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    stream
        .write_all(format!("POST {path} HTTP/1.1\r\nhost: localhost\r\n{extra}content-length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), stream.read_to_end(&mut out)).await.unwrap().unwrap();
    let text = String::from_utf8(out).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (status, head.to_ascii_lowercase(), serde_json::from_str(body).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_cannot_become_a_job_is_a_permanent_failure_not_a_retry_request() {
    let schema = format!("aip_webhook_big_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let facts = load_str(include_str!("fixtures/webhook-big.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','svc-hooks',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let digest = spike_v1_fixture::digest(&facts);
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-data").into() }),
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), iat }),
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        None,
        false,
        Infrastructure {
            triggers: Arc::new(Triggers {
                schedules: vec![],
                webhooks: vec![Webhook {
                    name: "orders".into(),
                    path: "/hooks/orders".into(),
                    operation: "receive".into(),
                    principal: ServicePrincipal { issuer: "test".into(), subject: "svc-hooks".into() },
                    secret: SECRET.to_vec(),
                    signature: Signature::HmacSha256 { header: "x-hub-signature-256".into(), prefix: "sha256=".into(), base64: false },
                    max_body_bytes: 1 << 20,
                }],
                outbox: vec![],
                messages: vec![],
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // 600000 quotes fit the 1 MiB body limit but become 1.2 MB of JSON in the job input.
    let body = "\"".repeat(600_000);
    let headers = vec![("x-hub-signature-256", signed(&body))];
    let (status, head, reply) = raw(server.address, "/hooks/orders", &headers, &body).await;
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert_eq!(status, 413, "a permanent failure tells the provider to stop retrying: {status} {reply}");
    assert_eq!(reply["code"], "WIRE.PAYLOAD_TOO_LARGE");
    assert!(!head.contains("retry-after"), "{head}");
}
