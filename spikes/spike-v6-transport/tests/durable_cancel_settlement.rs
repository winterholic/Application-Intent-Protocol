//! 실행 중인 durable job을 취소한 뒤 worker가 멈추면, 서버는 그 job을 `CANCELLED`로 확정하는 트랜잭션을 연다
//! (`durable.rs`의 `execute_inner` 마무리). 이 트랜잭션의 어느 단계에서 DB 오류·연결 끊김이 나도 취소 확인이 조용히 버려지면 안 된다.
//! 버려지면 job은 lease가 끝날 때까지 `CANCEL_REQUESTED`로 남고, 그 뒤 `CANCELLATION_UNCONFIRMED`로 실패한다.
//! worker는 이미 멈췄고 걸린 효과도 없었는데도 그렇게 된다(plan-docs/stabilization-20261122/track-b.md B2).
//! 장애는 `aip-test-fault-proxy`로 job의 DB 연결에만 건다. 인증 조회는 proxy를 거치지 않는다.
use aip_test_fault_proxy::{start, start_after, Fault};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v4_worker::{Isolation, WorkerLaunchConfig};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const DATABASE: &str = aip_test_support::test_database_url!();
/// 취소 확인의 첫 문장(`SELECT state FROM aip_jobs WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 FOR UPDATE`)에만 있는 조각이다.
const CANCEL_LOOKUP: &str = "lease_owner=$3 FOR UPDATE";
/// `Store::acknowledge_cancel`의 UPDATE에만 있는 조각이다. 이 문장을 보낸 연결에서만 COMMIT 단계 장애를 일으킨다.
const CANCEL_ACK: &str = "state='CANCELLED'";

static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::Builder::new().name("aip-test-schema-cleanup".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(DATABASE).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        if let Ok(cleanup) = cleanup {
            let _ = cleanup.join();
        }
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
            let db = spike_v2_read::connect_owned_with_url(DATABASE).await.map_err(|_| AuthError::Unavailable)?;
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

async fn post(addr: SocketAddr, path: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer owner\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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

/// 취소한 `slow` job(worker가 5초 잠든다)의 마지막 상태. 장애 뒤에도 몇 초 안에 끝나지 않으면 마지막으로 본 상태를 돌려준다.
async fn cancel_slow_job(fault: Fault, needle: Option<&'static str>) -> (Value, String) {
    let _lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_dur_cancel_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init(DATABASE, &schema, &facts).await.unwrap();
    let direct = spike_v2_read::connect_owned_with_url(DATABASE).await.unwrap();
    direct
        .batch_execute(&format!(
            "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1)"
        ))
        .await
        .unwrap();
    let iat: i64 = direct.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
    let (proxy, fired) = match needle {
        Some(needle) => start_after(needle, fault).await,
        None => start(fault).await,
    };
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::DecimalString,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::database_url_via_proxy(proxy.port()),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), iat }),
        Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
        None,
        false,
        // Cancelling does not depend on the worker sandbox (`Auto` needs bwrap on Linux).
        Infrastructure { worker_launch: WorkerLaunchConfig { isolation: Isolation::None, ..Default::default() }, ..Default::default() },
    )
    .await
    .unwrap();
    let queued = post(server.address, "/jobs/enqueue", &fingerprint, json!({"operation":"slow","input":{},"key":"cancel-me","delayMs":0})).await;
    let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("enqueue: {queued}")).to_string();
    // The worker is running its 5s sleep once the job is RUNNING; cancelling earlier would just cancel a queued job.
    for _ in 0..100 {
        if post(server.address, "/jobs/status", &fingerprint, json!({"id":id})).await["run"]["state"] == "RUNNING" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancelled = post(server.address, "/jobs/cancel", &fingerprint, json!({"id":id})).await;
    assert_eq!(cancelled["ok"], true, "{cancelled}");
    let mut status = Value::Null;
    for _ in 0..160 {
        status = post(server.address, "/jobs/status", &fingerprint, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&status["run"]["state"].as_str().unwrap_or("")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let phase = tokio::time::timeout(Duration::from_secs(1), fired)
        .await
        .unwrap_or_else(|_| panic!("the proxy must have injected its fault; status={status}"))
        .unwrap();
    server.shutdown().await.unwrap();
    (status, phase)
}

/// 취소는 사용자가 요청했고 worker는 멈췄고 걸린 효과도 없다. 확정 단계의 일시 장애가 이 사실을 바꾸지 않는다.
fn assert_cancelled(status: &Value, phase: &str) {
    assert_eq!(
        status["run"]["state"], "CANCELLED",
        "the cancel confirmation was lost instead of retried (phase: {phase}); the job would wait for its lease and then fail as CANCELLATION_UNCONFIRMED: {status}"
    );
    assert_eq!(status["run"]["attempt"], 1, "a cancelled job is not run again: {status}");
}

/// 취소 확인의 `FOR UPDATE` 조회 결과가 오기 전에 연결이 끊긴다. `if let Ok(Some(row))`는 이 오류를 "취소 요청 없음"처럼 넘겼다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_is_confirmed_after_the_job_row_lookup_loses_its_connection() {
    let (status, phase) = cancel_slow_job(Fault::CutAfterStatement(CANCEL_LOOKUP), None).await;
    assert_cancelled(&status, &phase);
}

/// 확인 UPDATE 뒤 COMMIT이 PostgreSQL에 닿지 않는다. `let _ = tx.commit()`은 이 실패를 버렸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_is_confirmed_after_the_commit_never_reaches_postgres() {
    let (status, phase) = cancel_slow_job(Fault::CutOnCommitSent, Some(CANCEL_ACK)).await;
    assert_cancelled(&status, &phase);
}

/// COMMIT은 PostgreSQL이 받았지만 응답을 잃었다. 확인은 이미 저장됐으므로 다시 확인하거나 다른 상태로 바꾸지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stays_cancelled_when_the_commit_ack_is_lost() {
    let (status, phase) = cancel_slow_job(Fault::AckDropFin, Some(CANCEL_ACK)).await;
    assert_cancelled(&status, &phase);
}
