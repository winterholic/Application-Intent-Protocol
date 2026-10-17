//! Committed `notify` outbox rows of configured topics become exactly one durable job each.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
    triggers::{OutboxHandler, ServicePrincipal, Triggers},
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
                    let infra = aip_execution::infrastructure_schema(&schema).unwrap();
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {infra} CASCADE")).await;
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

fn handlers(subject: &str) -> Arc<Triggers> {
    Arc::new(Triggers {
        outbox: vec![OutboxHandler {
            topic: "order.paid".into(),
            operation: "fulfil".into(),
            principal: ServicePrincipal { issuer: "test".into(), subject: subject.into() },
        }],
        ..Default::default()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_notify_rows_become_one_job_per_row_across_instances() {
    let schema = format!("aip_outbox_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/outbox.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let infra = aip_execution::infrastructure_schema(&schema).unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1),('test','svc-relay',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let start = |lang: WorkerLang| {
        listen_with_infrastructure(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            aip_test_support::test_database_url!().into(),
            Some(ReadExtensions { lang, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-data").into() }),
            ServerOptions::default(),
            false,
            auth.clone(),
            Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
            None,
            false,
            Infrastructure { triggers: handlers("svc-relay"), ..Default::default() },
        )
    };
    for (round, lang) in [WorkerLang::Node, WorkerLang::Python].into_iter().enumerate() {
        let order = 11 + round as i64;
        db.batch_execute(&format!("INSERT INTO {schema}.\"order\"(id,member_id,paid,shipped) VALUES({order},1,false,false)")).await.unwrap();
        let first = start(lang).await.unwrap();
        let second = start(lang).await.unwrap();
        let applied =
            post(first.address, "/apply", &fp, json!({"key":format!("pay-{order}"),"request":{"apply":"Order.pay","target":{"ids":[order]}}})).await;
        assert_eq!(applied["changed"], json!([order]), "{lang:?}: {applied}");
        // A rejected transition rolls back its notify row, so nothing is relayed for it.
        let rejected =
            post(first.address, "/apply", &fp, json!({"key":format!("again-{order}"),"request":{"apply":"Order.pay","target":{"ids":[order]}}}))
                .await;
        assert_eq!(rejected["code"], "WIRE.INVALID_STATE", "{lang:?}: {rejected}");

        let mut shipped = false;
        for _ in 0..200 {
            shipped = db.query_one(&format!("SELECT shipped FROM {schema}.\"order\" WHERE id=$1"), &[&order]).await.unwrap().get(0);
            if shipped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(shipped, "{lang:?}: relay did not ship order {order}");
        // Give the second instance time to race on the same row before counting.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let outbox: Vec<i64> = db
            .query(&format!("SELECT id FROM {schema}.aip_outbox WHERE source_id=$1"), &[&order])
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(outbox.len(), 1, "{lang:?}");
        let jobs: i64 = db
            .query_one(
                &format!("SELECT count(*) FROM {schema}.aip_jobs WHERE idempotency_key LIKE $1 AND state='SUCCEEDED'"),
                &[&format!("outbox:{}:%", outbox[0])],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(jobs, 1, "{lang:?}");
        let dispatched: i64 =
            db.query_one(&format!("SELECT count(*) FROM {infra}.aip_outbox_dispatch WHERE outbox_id=$1"), &[&outbox[0]]).await.unwrap().get(0);
        assert_eq!(dispatched, 1, "{lang:?}");
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn outbox_handlers_require_a_declared_topic_and_event_input() {
    let facts = load_str(include_str!("fixtures/outbox.aip"), Form::A).unwrap().execution;
    let check = |topic: &str, operation: &str| {
        Triggers {
            outbox: vec![OutboxHandler {
                topic: topic.into(),
                operation: operation.into(),
                principal: ServicePrincipal { issuer: "test".into(), subject: "svc".into() },
            }],
            ..Default::default()
        }
        .validate(&facts, IdWire::SafeNumber, &[])
    };
    assert!(check("order.paid", "fulfil").is_ok());
    assert!(check("order.refunded", "fulfil").is_err());
    assert!(check("order.paid", "missing").is_err());
}
