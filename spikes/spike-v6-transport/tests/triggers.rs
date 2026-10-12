//! Server-configured schedules and signed inbound webhooks start durable jobs as an operator-bound principal.
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    schedule::Cron,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
    triggers::{Schedule, ServicePrincipal, Signature, Triggers, Webhook},
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

async fn status(addr: std::net::SocketAddr, token: &str, fp: &str, id: &str) -> Value {
    for _ in 0..200 {
        let (_, body) =
            raw(addr, "/jobs/status", &[("authorization", format!("Bearer {token}")), ("x-aip-contract", fp.into())], &json!({"id":id}).to_string())
                .await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&body["run"]["state"].as_str().unwrap_or("")) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

fn signed(body: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(SECRET).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schedules_and_signed_webhooks_run_jobs_as_the_bound_service_principal() {
    let schema = format!("aip_triggers_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/triggers.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','svc-hooks',1),('test','svc-cron',1)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let triggers = |schedules: bool| {
        Arc::new(Triggers {
            schedules: if schedules {
                vec![Schedule {
                    name: "sweep".into(),
                    operation: "sweep".into(),
                    cron: Cron::parse("* * * * *", "Asia/Seoul").unwrap(),
                    input: json!({}),
                    principal: ServicePrincipal { issuer: "test".into(), subject: "svc-cron".into() },
                    misfire_grace: Duration::from_secs(120),
                }]
            } else {
                vec![]
            },
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
        })
    };
    let start = |lang: WorkerLang, schedules: bool| {
        listen_with_infrastructure(
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
            Infrastructure { triggers: triggers(schedules), ..Default::default() },
        )
    };

    for lang in [WorkerLang::Node, WorkerLang::Python] {
        db.batch_execute(&format!(
            "DELETE FROM {schema}.aip_effects; DELETE FROM {schema}.aip_jobs; DELETE FROM {schema}.\"order\"; INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(11,1,false),(12,1,false),(99,2,false)"
        ))
        .await
        .unwrap();
        db.batch_execute(&format!("UPDATE {schema}.aip_principals SET enabled=true")).await.unwrap();
        let server = start(lang, false).await.unwrap();
        let addr = server.address;
        let body = r#"{"order":11}"#;
        let headers = vec![("x-hub-signature-256", signed(body)), ("content-type", "application/json".into())];

        let (code, accepted) = raw(addr, "/hooks/orders", &headers, body).await;
        assert_eq!(code, 202, "{lang:?}: {accepted}");
        let id = accepted["id"].as_str().unwrap().to_string();
        let settled = status(addr, "svc-hooks", &fp, &id).await;
        assert_eq!(settled["run"]["state"], "SUCCEEDED", "{lang:?}: {settled}");
        let paid: bool = db.query_one(&format!("SELECT paid FROM {schema}.\"order\" WHERE id=11"), &[]).await.unwrap().get(0);
        assert!(paid, "{lang:?}");

        // A provider retry of the same delivery returns the same job instead of running it again.
        let (code, replay) = raw(addr, "/hooks/orders", &headers, body).await;
        assert_eq!((code, replay["id"].as_str()), (202, Some(id.as_str())), "{lang:?}: {replay}");
        // Unsigned, tampered and wrong-method requests never reach the job store.
        let other = r#"{"order":12}"#;
        let (code, _) = raw(addr, "/hooks/orders", &[("x-hub-signature-256", signed(body))], other).await;
        assert_eq!(code, 401, "{lang:?}");
        let (code, _) = raw(addr, "/hooks/orders", &[], other).await;
        assert_eq!(code, 401, "{lang:?}");
        let jobs: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_jobs"), &[]).await.unwrap().get(0);
        assert_eq!(jobs, 1, "{lang:?}");
        // A user token cannot see the service principal's job.
        let (_, hidden) =
            raw(addr, "/jobs/status", &[("authorization", "Bearer svc-cron".into()), ("x-aip-contract", fp.clone())], &json!({"id":id}).to_string())
                .await;
        assert_eq!(hidden["code"], "NOT_FOUND", "{lang:?}: {hidden}");
        // Disabling the bound principal stops intake with a retryable status.
        db.batch_execute(&format!("UPDATE {schema}.aip_principals SET enabled=false WHERE subject='svc-hooks'")).await.unwrap();
        let fresh = r#"{"order":12}"#;
        let (code, unavailable) = raw(addr, "/hooks/orders", &[("x-hub-signature-256", signed(fresh))], fresh).await;
        assert_eq!((code, unavailable["code"].as_str()), (503, Some("PRINCIPAL_UNAVAILABLE")), "{lang:?}: {unavailable}");
        server.shutdown().await.unwrap();

        // Two instances evaluate the same schedule; the per-slot key yields one job per slot.
        let first = start(lang, true).await.unwrap();
        let second = start(lang, true).await.unwrap();
        let mut swept = None;
        for _ in 0..200 {
            let row = db
                .query_opt(
                    &format!(
                        "SELECT id::text,state FROM {schema}.aip_jobs WHERE idempotency_key LIKE 'schedule:sweep:%' ORDER BY created_at LIMIT 1"
                    ),
                    &[],
                )
                .await
                .unwrap();
            if let Some(row) = row.filter(|row| row.get::<_, String>(1) == "SUCCEEDED") {
                swept = Some(row.get::<_, String>(0));
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(swept.is_some(), "{lang:?}: schedule did not run");
        let (rows, keys): (i64, i64) = {
            let row = db
                .query_one(
                    &format!("SELECT count(*),count(DISTINCT idempotency_key) FROM {schema}.aip_jobs WHERE idempotency_key LIKE 'schedule:sweep:%'"),
                    &[],
                )
                .await
                .unwrap();
            (row.get(0), row.get(1))
        };
        assert_eq!(rows, keys, "{lang:?}: duplicate schedule slot");
        let orders: Vec<(i64, bool)> = db
            .query(&format!("SELECT id,paid FROM {schema}.\"order\" ORDER BY id"), &[])
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        // The service principal's row policy still hides member 2's order from the scheduled sweep.
        assert_eq!(orders, vec![(11, true), (12, true), (99, false)], "{lang:?}");
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn trigger_configuration_is_rejected_before_listening() {
    let facts = load_str(include_str!("fixtures/triggers.aip"), Form::A).unwrap().execution;
    let base = || Webhook {
        name: "orders".into(),
        path: "/hooks/orders".into(),
        operation: "receive".into(),
        principal: ServicePrincipal { issuer: "test".into(), subject: "svc".into() },
        secret: SECRET.to_vec(),
        signature: Signature::Stripe { tolerance: Duration::from_secs(300) },
        max_body_bytes: 4096,
    };
    let check = |webhooks: Vec<Webhook>, schedules: Vec<Schedule>| {
        Triggers { webhooks, schedules, outbox: vec![], messages: vec![] }.validate(&facts, IdWire::SafeNumber, &["/jobs", "/assets"])
    };
    assert!(check(vec![base()], vec![]).is_ok());
    assert!(check(vec![Webhook { path: "/jobs/hook".into(), ..base() }], vec![]).is_err());
    assert!(check(vec![Webhook { path: "/assets".into(), ..base() }], vec![]).is_err());
    assert!(check(vec![Webhook { path: "/hooks/../x".into(), ..base() }], vec![]).is_err());
    assert!(check(vec![Webhook { operation: "sweep".into(), ..base() }], vec![]).is_err());
    assert!(check(vec![Webhook { secret: b"short".to_vec(), ..base() }], vec![]).is_err());
    assert!(check(vec![base(), Webhook { name: "other".into(), ..base() }], vec![]).is_err());
    let schedule = || Schedule {
        name: "sweep".into(),
        operation: "sweep".into(),
        cron: Cron::parse("0 3 * * *", "UTC").unwrap(),
        input: json!({}),
        principal: ServicePrincipal { issuer: "test".into(), subject: "svc".into() },
        misfire_grace: Duration::from_secs(60),
    };
    assert!(check(vec![], vec![schedule()]).is_ok());
    assert!(check(vec![], vec![Schedule { input: json!({"extra":1}), ..schedule() }]).is_err());
    assert!(check(vec![], vec![Schedule { operation: "missing".into(), ..schedule() }]).is_err());
    assert!(check(vec![], vec![Schedule { misfire_grace: Duration::from_secs(90_000), ..schedule() }]).is_err());
    assert!(check(vec![], vec![Schedule { misfire_grace: Duration::from_secs(1), ..schedule() }]).is_err());
}
