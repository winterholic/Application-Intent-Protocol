//! Outbound webhooks: signed delivery to subscriber URLs from business data, with SSRF rules and resendable failures.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
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
async fn jobs_deliver_signed_webhooks_to_public_subscribers_and_can_resend_failures() {
    use aip_execution::integrations::{standard_webhook_signature, Broker, WebhookSenderConfig};
    let schema = format!("aip_outbound_hooks_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/outbound-webhook.aip"), Form::A).unwrap().execution;
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
    const SECRET: &[u8] = b"subscriber-signing-secret";

    // The subscriber verifies the Standard Webhooks signature itself and records what it accepted.
    let receiver = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = receiver.local_addr().unwrap().port();
    let accepted = Arc::new(std::sync::Mutex::new(Vec::<(String, Value)>::new()));
    let log = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = receiver.accept().await {
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                raw.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = head
                        .lines()
                        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse().unwrap()))
                        .unwrap_or(0);
                    if body.len() >= length || n == 0 {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&raw).to_string();
            let (head, body) = text.split_once("\r\n\r\n").unwrap();
            let header = |name: &str| {
                head.lines().find_map(|line| line.split_once(": ").filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.to_string())).unwrap()
            };
            let (id, timestamp) = (header("webhook-id"), header("webhook-timestamp").parse::<u64>().unwrap());
            let expected = format!("v1,{}", standard_webhook_signature(SECRET, &id, timestamp, body.as_bytes()));
            let valid = header("webhook-signature") == expected;
            let status = if head.starts_with("POST /fail") || !valid { "500 Internal Server Error" } else { "204 No Content" };
            if status.starts_with("204") {
                log.lock().unwrap().push((id, serde_json::from_str(body).unwrap()));
            }
            let _ = stream.write_all(format!("HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n").as_bytes()).await;
        }
    });

    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let broker = Broker::new(vec![])
            .unwrap()
            .with_webhooks(vec![WebhookSenderConfig {
                name: "subscribers".into(),
                secret: SECRET.to_vec(),
                timeout: Duration::from_secs(2),
                max_request_bytes: 4096,
                allow_loopback: true,
            }])
            .unwrap();
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            aip_test_support::test_database_url!().into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/outbound-webhook").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            Some(Arc::new(broker)),
            false,
            Infrastructure::default(),
        )
        .await
        .unwrap();
        let label = format!("{lang:?}").to_lowercase();
        let before = accepted.lock().unwrap().len();
        let input = json!({
            "url": format!("http://127.0.0.1:{port}/hooks/{label}"),
            "private": "https://10.1.2.3/hook",
            "failing": format!("http://127.0.0.1:{port}/fail"),
        });
        let queued =
            post(server.address, "/jobs/enqueue", &fp, json!({"operation":"notify","input":input,"key":format!("notify-{label}"),"delayMs":0})).await;
        let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("{lang:?}: {queued}")).to_string();
        let status = settle(server.address, &fp, &id).await;
        assert_eq!(status["run"]["state"], "SUCCEEDED", "{lang:?}: {status}");
        let output = post(server.address, "/jobs/result", &fp, json!({"id":id})).await["output"].clone();
        assert_eq!(output["status"], 204, "{lang:?}: {output}");
        assert_eq!(output["same"], true, "{lang:?}: {output}");
        assert_eq!(output["privateCode"], "WIRE.DESTINATION_REJECTED", "{lang:?}: {output}");
        assert_eq!(output["failingCode"], "WIRE.DELIVERY_FAILED", "{lang:?}: {output}");
        // Exactly one signed delivery reached the subscriber, under the id the job saw.
        let delivered = accepted.lock().unwrap()[before..].to_vec();
        assert_eq!(delivered.len(), 1, "{lang:?}: {delivered:?}");
        assert_eq!(delivered[0].0, output["id"].as_str().unwrap(), "{lang:?}");
        assert_eq!(delivered[0].1, json!({"type":"order.paid","order":11}), "{lang:?}");
        // Rejected and failed deliveries leave no journal entry, so the same key can be sent again later.
        let keys: Vec<String> = db
            .query(&format!("SELECT effect_key FROM {schema}.aip_effects WHERE run_id=$1::text::uuid"), &[&id])
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(keys.len(), 1, "{lang:?}: {keys:?}");
        server.shutdown().await.unwrap();
    }
}
