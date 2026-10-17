//! Every committed transition leaves one append-only business audit row in the same transaction, whichever path ran it.
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

/// Drops the deployment schema and its infrastructure schema after the test, also on panic.
struct SchemaCleanup(Vec<String>);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schemas = self.0.clone();
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await {
                    for schema in schemas {
                        let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                    }
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
    for _ in 0..200 {
        let status = post(addr, "/jobs/status", fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&status["run"]["state"].as_str().unwrap_or("")) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

type AuditRow = (Option<i64>, String, Option<String>, String, String, i64, Value, Value);

async fn audit_rows(db: &tokio_postgres::Client, table: &str) -> Vec<AuditRow> {
    db.query(&format!("SELECT actor_id,via,request_key,resource,transition,target_id,before,after FROM {table} ORDER BY id"), &[])
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5), r.get(6), r.get(7)))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_committed_transition_leaves_one_audit_row_in_the_same_transaction() {
    let schema = format!("aip_audit_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let infra = aip_execution::infrastructure_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(vec![schema.clone(), infra.clone()]);
    let audit = format!("{infra}.aip_audit");
    let facts = load_str(include_str!("fixtures/audit.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); \
         INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1); \
         INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(11,1,false),(12,1,false),(99,2,false)"
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
        aip_test_support::test_database_url!().into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/audit").into() }),
        ServerOptions::default(),
        true,
        auth.clone(),
        Some(DeploymentFence::new(schema.clone(), digest.clone()).unwrap()),
        None,
        false,
    )
    .await
    .unwrap();
    let addr = server.address;

    // A durable job's data transitions are attributed to the job run and each effect key.
    let queued = post(addr, "/jobs/enqueue", &fp, json!({"operation":"settle","input":{"other":99},"key":"settle","delayMs":0})).await;
    let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("enqueue: {queued}")).to_string();
    let status = settle(addr, &fp, &id).await;
    assert_eq!(status["run"]["state"], "SUCCEEDED", "{status}");
    let paid = |via: &str, key: Option<String>, target: i64| -> AuditRow {
        (Some(1), via.into(), key, "Order".into(), "pay".into(), target, json!({"paid":false}), json!({"paid":true}))
    };
    // The key joins the effect journal; the denied call on 99 and the journal replay of 11 record nothing.
    let effects: Vec<String> = db
        .query(&format!("SELECT effect_key FROM {schema}.aip_effects WHERE run_id=$1::text::uuid AND state='SUCCEEDED'"), &[&id])
        .await
        .unwrap()
        .iter()
        .map(|row| format!("{id}:{}", row.get::<_, String>(0)))
        .collect();
    let job_rows = audit_rows(&db, &audit).await;
    assert_eq!(job_rows.len(), 2, "{job_rows:?}");
    for (row, target) in job_rows.iter().zip([11, 12]) {
        assert!(effects.contains(row.2.as_ref().unwrap()), "{row:?} not in {effects:?}");
        assert_eq!(*row, paid("job", row.2.clone(), target));
    }

    db.batch_execute(&format!("INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(13,1,false),(14,1,false)")).await.unwrap();
    let applied = post(addr, "/apply", &fp, json!({"key":"pay-13","request":{"apply":"Order.pay","target":{"ids":[13]}}})).await;
    assert_eq!(applied["changed"], json!([13]), "{applied}");
    let mut expected = job_rows.clone();
    expected.push(paid("apply", Some("pay-13".into()), 13));
    assert_eq!(audit_rows(&db, &audit).await, expected);

    // A replayed key, an invisible target and an all-or-nothing bulk that fails on one row change nothing and record nothing.
    let replay = post(addr, "/apply", &fp, json!({"key":"pay-13","request":{"apply":"Order.pay","target":{"ids":[13]}}})).await;
    assert_eq!(replay["replayed"], true, "{replay}");
    let hidden = post(addr, "/apply", &fp, json!({"key":"pay-99","request":{"apply":"Order.pay","target":{"ids":[99]}}})).await;
    assert_eq!(hidden["code"], "WIRE.MISSING_TARGET", "{hidden}");
    let mixed = post(addr, "/apply", &fp, json!({"key":"pay-mixed","request":{"apply":"Order.pay","target":{"ids":[14,99]}}})).await;
    assert_eq!(mixed["code"], "WIRE.MISSING_TARGET", "{mixed}");
    assert_eq!(audit_rows(&db, &audit).await, expected);

    // A write extension's ctx transition commits with its audit row in the server-owned transaction.
    let extension = post(addr, "/apply", &fp, json!({"key":"now-14","request":{"extension":"Order.payNow","input":{"id":14}}})).await;
    assert_eq!(extension["output"], json!({"id":14}), "{extension}");
    expected.push(paid("extension", None, 14));
    assert_eq!(audit_rows(&db, &audit).await, expected);

    // Audit rows are append-only even for the service's own database role.
    let edit = db.execute(&format!("UPDATE {audit} SET after = '{{}}'::jsonb"), &[]).await.unwrap_err();
    assert_eq!(edit.code().map(|c| c.code()), Some("42501"), "{edit}");
    server.shutdown().await.unwrap();
}
