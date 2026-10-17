//! 내구 job의 데이터 쓰기 COMMIT 손실(WBS 8.9). COMMIT을 보낸 뒤 응답을 잃거나 해석하지 못하면 job은 확장 오류(`EXTENSION_ERROR`)가
//! 아니라 `WIRE.COMMIT_UNKNOWN`으로 끝난다. 쓰기가 커밋됐을 수 있기 때문이고, 자동으로 다시 실행하지 않는다.
use aip_test_fault_proxy::{start_after, Fault};
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
    for _ in 0..200 {
        let status = post(addr, "/jobs/status", fp, json!({"id":id})).await;
        if !["QUEUED", "RUNNING", "CANCEL_REQUESTED"].contains(&status["run"]["state"].as_str().unwrap_or("")) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not settle");
}

// sqlgen의 schema는 프로세스당 하나라서 테스트를 순서대로 돌린다.
static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 데이터 apply의 journal 기록 UPDATE. 이 문장을 보낸 연결에서만 COMMIT 단계 장애를 일으킨다.
const JOURNAL_SUCCESS: &str = "state='SUCCEEDED',result=$3";
/// 전이 UPDATE 문장에만 있는 조각이다. 이 문장이 끝난 직후 연결을 끊는다(COMMIT 전).
const TRANSITION_UPDATE: &str = ".order t SET";

struct Outcome {
    status: Value,
    phase: String,
    paid: Vec<(i64, bool)>,
    journal: i64,
}

/// `settle` job을 한 번 접수해 끝날 때까지 기다린다. 장애는 job의 DB 연결에만 건다(인증 조회는 직접 연결).
async fn run_settle(fault: Fault) -> Outcome {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_job_commit_loss_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/job-data.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1); INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES(11,1,false),(12,1,false),(99,2,false)"
    ))
    .await
    .unwrap();
    let iat = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let digest = spike_v1_fixture::digest(&facts);
    let fp = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let (proxy, fired) = match fault {
        Fault::CutAfterStatement(_) => aip_test_fault_proxy::start(fault).await,
        _ => start_after(JOURNAL_SUCCESS, fault).await,
    };
    let server = listen_with_managed(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        aip_test_support::database_url_via_proxy(proxy.port()),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job-data").into() }),
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), iat }),
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        None,
        false,
    )
    .await
    .unwrap();
    let queued = post(server.address, "/jobs/enqueue", &fp, json!({"operation":"settle","input":{"other":99},"key":"settle-loss","delayMs":0})).await;
    let id = queued["run"]["id"].as_str().unwrap_or_else(|| panic!("enqueue: {queued}")).to_string();
    let status = settle(server.address, &fp, &id).await;
    let phase = tokio::time::timeout(Duration::from_secs(1), fired)
        .await
        .unwrap_or_else(|_| panic!("the proxy must have injected its fault; status={status}"))
        .unwrap();
    let paid: Vec<(i64, bool)> = db
        .query(&format!("SELECT id,paid FROM {schema}.\"order\" ORDER BY id"), &[])
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    let journal: i64 = db
        .query_one(&format!("SELECT count(*) FROM {schema}.aip_effects WHERE run_id=$1::text::uuid AND state='SUCCEEDED'"), &[&id])
        .await
        .unwrap()
        .get(0);
    server.shutdown().await.unwrap();
    Outcome { status, phase, paid, journal }
}

/// job이 데이터를 쓰는 COMMIT의 응답을 잃거나 COMMIT이 서버에 닿지 못하면 job은 WIRE.COMMIT_UNKNOWN으로 끝난다.
/// 커밋 여부는 DB가 알려준다(committed). 어느 쪽이든 job은 자동으로 재시도되지 않고 두 번째 쓰기는 일어나지 않는다.
async fn assert_commit_unknown(fault: Fault, committed: bool) {
    let Outcome { status, phase, paid, journal } = run_settle(fault).await;
    assert_eq!(status["run"]["state"], "FAILED", "{fault:?}: phase={phase}; {status}");
    assert_eq!(status["run"]["error"], "WIRE.COMMIT_UNKNOWN", "{fault:?}: the job must not report a plain extension failure: {status}");
    assert_eq!(status["run"]["attempt"], 1, "{fault:?}: an unknown COMMIT is not retried automatically: {status}");
    assert_eq!(paid, vec![(11, committed), (12, false), (99, false)], "{fault:?}: only the first transition may have been committed");
    assert_eq!(journal, i64::from(committed), "{fault:?}: the journal entry commits together with the transition");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_ends_commit_unknown_when_the_commit_ack_is_lost() {
    assert_commit_unknown(Fault::AckDropFin, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_ends_commit_unknown_when_the_commit_never_reaches_postgres() {
    assert_commit_unknown(Fault::CutOnCommitSent, false).await;
}

/// COMMIT 전에 연결을 잃으면 아무것도 커밋되지 않았다. job은 DB_UNAVAILABLE로 재시도되고 두 번째 시도가 끝까지 쓴다.
/// (managed execution 문서: DB 장애는 아무것도 commit하지 않았으므로 기록하지 않고 다시 시도할 수 있다.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_is_retried_when_the_connection_is_lost_before_commit() {
    let Outcome { status, phase, paid, journal } = run_settle(Fault::CutAfterStatement(TRANSITION_UPDATE)).await;
    assert_eq!(status["run"]["state"], "SUCCEEDED", "phase={phase}; {status}");
    assert_eq!(status["run"]["attempt"], 2, "the lost connection must cost exactly one retry: {status}");
    assert_eq!(paid, vec![(11, true), (12, true), (99, false)], "each transition happens exactly once");
    assert_eq!(journal, 2, "one journal entry per transition, none from the lost first attempt");
}
