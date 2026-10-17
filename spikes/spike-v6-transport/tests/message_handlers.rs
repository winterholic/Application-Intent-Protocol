//! Configured message handlers turn managed queue messages into durable jobs, acking with the job acceptance.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::triggers::{MessageHandler, ServicePrincipal, Triggers};
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
async fn message_state(db: &tokio_postgres::Client, schema: &str, id: &str) -> (String, i32, Option<String>) {
    let row = db.query_one(&format!("SELECT state,deliveries,last_error FROM {schema}.aip_messages WHERE id=$1::text::uuid"), &[&id]).await.unwrap();
    (row.get(0), row.get(1), row.get(2))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_messages_become_jobs_once_and_failed_dispatch_reaches_the_dead_letter_queue() {
    use aip_execution::messages::{MessageStore, NewMessage};
    let schema = format!("aip_msg_handlers_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/message-handlers.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id,enabled) VALUES('test','owner',1,true),('test','svc-consumer',1,true),('test','svc-off',1,false)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    // The service principal's own message scope, the same key its jobs publish under.
    let svc_owner = spike_v1_fixture::digest(&json!(["test", "svc-consumer", 1, 0]));
    let store = MessageStore::new(&schema).unwrap();
    let principal = |subject: &str| ServicePrincipal { issuer: "test".into(), subject: subject.into() };
    let triggers = Arc::new(Triggers {
        messages: vec![
            MessageHandler { queue: "orders".into(), operation: "consume".into(), principal: principal("svc-consumer"), owner: None },
            MessageHandler {
                queue: "partner".into(),
                operation: "consume".into(),
                principal: principal("svc-off"),
                owner: Some("partner-owner".into()),
            },
        ],
        ..Default::default()
    });
    let invalid = Triggers {
        messages: vec![MessageHandler { queue: "undeclared".into(), operation: "consume".into(), principal: principal("svc-consumer"), owner: None }],
        ..Default::default()
    };
    assert!(invalid.validate(&facts, IdWire::SafeNumber, &[]).is_err(), "an undeclared queue must be rejected at startup");

    for lang in [WorkerLang::Node, WorkerLang::Python] {
        let label = format!("{lang:?}").to_lowercase();
        let mut ids = Vec::new();
        for order in [11, 12] {
            let message = store
                .enqueue(
                    &*db,
                    NewMessage {
                        owner: svc_owner.clone(),
                        queue: "orders".into(),
                        key: format!("{label}-{order}"),
                        payload: json!({"order":order}),
                        max_deliveries: 3,
                        ttl_ms: 600_000,
                        delay_ms: 0,
                    },
                )
                .await
                .unwrap();
            ids.push((order, message.id));
        }
        let dead = store
            .enqueue(
                &*db,
                NewMessage {
                    owner: "partner-owner".into(),
                    queue: "partner".into(),
                    key: format!("{label}-dead"),
                    payload: json!({"order":99}),
                    max_deliveries: 1,
                    ttl_ms: 600_000,
                    delay_ms: 0,
                },
            )
            .await
            .unwrap();
        // Two instances relay the same queue; each message still becomes one job.
        let mut servers = Vec::new();
        for _ in 0..2 {
            servers.push(
                listen_with_infrastructure(
                    facts.clone(),
                    IdWire::SafeNumber,
                    "127.0.0.1:0".parse().unwrap(),
                    aip_test_support::test_database_url!().into(),
                    Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/message-handlers").into() }),
                    ServerOptions::default(),
                    false,
                    auth.clone(),
                    Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
                    None,
                    false,
                    Infrastructure { messaging: true, triggers: triggers.clone(), ..Default::default() },
                )
                .await
                .unwrap(),
            );
        }
        for (order, id) in &ids {
            let mut settled = None;
            for _ in 0..200 {
                let row = db
                    .query_opt(&format!("SELECT state,output FROM {schema}.aip_jobs WHERE idempotency_key=$1"), &[&format!("message:{id}")])
                    .await
                    .unwrap();
                if let Some(row) = row.filter(|row| row.get::<_, String>(0) == "SUCCEEDED") {
                    settled = Some(row.get::<_, Value>(1));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(settled, Some(json!({"order":order,"deliveries":1})), "{lang:?}: message {id}");
            let jobs: i64 = db
                .query_one(&format!("SELECT count(*) FROM {schema}.aip_jobs WHERE idempotency_key=$1"), &[&format!("message:{id}")])
                .await
                .unwrap()
                .get(0);
            assert_eq!(jobs, 1, "{lang:?}");
            assert_eq!(message_state(&db, &schema, id).await.0, "ACKED", "{lang:?}");
        }
        // A disabled service principal cannot accept the job; the message is nacked into the DLQ.
        let mut state = message_state(&db, &schema, &dead.id).await;
        for _ in 0..100 {
            if state.0 == "DEAD" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            state = message_state(&db, &schema, &dead.id).await;
        }
        assert_eq!(state, ("DEAD".to_string(), 1, Some("WIRE.PRINCIPAL_UNAVAILABLE".to_string())), "{lang:?}");
        // A user cannot submit a job with the reserved message key.
        let forged = post(
            servers[0].address,
            "/jobs/enqueue",
            &fp,
            json!({"operation":"consume","input":{"message":"{}"},"key":"message:forged","delayMs":0}),
        )
        .await;
        assert_eq!(forged["ok"], false, "{lang:?}: {forged}");
        for server in servers {
            server.shutdown().await.unwrap();
        }
    }
}
