//! 내구 쓰기 요청(`/jobs/enqueue`)의 COMMIT 분류(WBS 8.9). COMMIT을 보내기 전에 끊기면 DB_UNAVAILABLE(커밋 없음),
//! 보낸 뒤 끊기거나 응답을 해석할 수 없으면 COMMIT_UNKNOWN. COMMIT 호출 시점에 이미 닫힌 연결은 `durable.rs`의 단위 테스트가 본다.
use aip_test_fault_proxy::{start_after, Fault};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
};

const DATABASE: &str = aip_test_support::test_database_url!();
/// job 접수 INSERT에만 있는 열 나열이다. 이 문장을 보낸 연결에서만 COMMIT 단계 장애를 일으킨다.
const ENQUEUE_INSERT: &str = "idempotency_key,state,timeout_ms";

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
            // 인증 조회는 proxy를 거치지 않고 직접 연결한다. 장애는 쓰기 요청의 DB 연결에만 건다.
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

async fn post(addr: SocketAddr, path: &str, token: &str, fp: &str, body: Value) -> Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fp}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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

struct Fixture {
    direct: spike_v2_read::OwnedConnection,
    server: spike_v6_transport::server::Server,
    fingerprint: String,
    schema: String,
    fired: oneshot::Receiver<String>,
    _cleanup: SchemaCleanup,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new(fault: Fault) -> Self {
        let lock = SCHEMA_LOCK.lock().await;
        let schema = format!("aip_dur_commit_{}", std::process::id());
        sqlgen::set_schema(&schema);
        let cleanup = SchemaCleanup(schema.clone());
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
        let (address, fired) = match fault {
            Fault::CutAfterStatement(_) => aip_test_fault_proxy::start(fault).await,
            _ => start_after(ENQUEUE_INSERT, fault).await,
        };
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::DecimalString,
            "127.0.0.1:0".parse().unwrap(),
            aip_test_support::database_url_via_proxy(address.port()),
            Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
            ServerOptions::default(),
            false,
            Arc::new(Auth { schema: schema.clone(), iat }),
            Some(DeploymentFence::new(schema.clone(), spike_v1_fixture::digest(&facts)).unwrap()),
            None,
            false,
            Default::default(),
        )
        .await
        .unwrap();
        Self { direct, server, fingerprint, schema, fired, _cleanup: cleanup, _lock: lock }
    }

    async fn enqueue(&self, key: &str) -> Value {
        post(
            self.server.address,
            "/jobs/enqueue",
            "owner",
            &self.fingerprint,
            json!({"operation":"count","input":{"text":"abc"},"key":key,"delayMs":0}),
        )
        .await
    }

    async fn jobs(&self, key: &str) -> i64 {
        self.direct.query_one(&format!("SELECT count(*) FROM {}.aip_jobs WHERE idempotency_key=$1", self.schema), &[&key]).await.unwrap().get(0)
    }

    async fn phase(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(5), &mut self.fired)
            .await
            .expect("the proxy must have injected its fault")
            .expect("fault observation channel")
    }

    async fn finish(self) {
        self.server.shutdown().await.unwrap();
    }
}

/// COMMIT이 보내지기 전에 연결이 끊기면 아무것도 커밋되지 않았다. 같은 키로 다시 보내면 새로 접수된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_after_the_job_insert_before_commit_is_db_unavailable() {
    let mut f = Fixture::new(Fault::CutAfterStatement(ENQUEUE_INSERT)).await;
    let first = f.enqueue("cut-before-commit").await;
    let phase = f.phase().await;
    assert_eq!(first["code"], "WIRE.DB_UNAVAILABLE", "phase={phase}; body={first}");
    assert_eq!(f.jobs("cut-before-commit").await, 0, "nothing was committed");
    let retry = f.enqueue("cut-before-commit").await;
    assert_eq!(retry["ok"], true, "same key must be accepted again after a safe-retry error: {retry}");
    assert_eq!(f.jobs("cut-before-commit").await, 1);
    f.finish().await;
}

/// COMMIT을 보낸 뒤 응답을 받지 못하면, 서버에 닿았는지와 무관하게 이 연결로는 알 수 없다. 같은 키로 다시 보내면 한 건만 남는다.
async fn assert_commit_unknown(fault: Fault, committed: bool) {
    let mut f = Fixture::new(fault).await;
    let first = f.enqueue("cut-after-commit").await;
    let phase = f.phase().await;
    assert_eq!(first["code"], "WIRE.COMMIT_UNKNOWN", "{fault:?}: phase={phase}; body={first}");
    assert_eq!(f.jobs("cut-after-commit").await, i64::from(committed), "{fault:?}: phase={phase}");
    let retry = f.enqueue("cut-after-commit").await;
    assert_eq!(retry["ok"], true, "{fault:?}: same key must settle the unknown outcome: {retry}");
    assert_eq!(f.jobs("cut-after-commit").await, 1, "{fault:?}: the job exists exactly once");
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_withheld_from_postgres_is_commit_unknown() {
    assert_commit_unknown(Fault::CutOnCommitSent, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ack_dropped_with_fin_is_commit_unknown() {
    assert_commit_unknown(Fault::AckDropFin, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unparseable_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Fault::ReplaceCommitAck(b'x'), true).await;
}
