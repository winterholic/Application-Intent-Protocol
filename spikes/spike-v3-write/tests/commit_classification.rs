//! COMMIT 경로의 연결 오류 분류(WBS 8.9). COMMIT을 보내기 전에 끊기면 DB_UNAVAILABLE(커밋 없음),
//! 보낸 뒤 끊기거나 응답을 해석할 수 없으면 COMMIT_UNKNOWN, 서버가 거절한 오류는 기존 코드를 유지한다.
use aip_test_fault_proxy::{start, Fault};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{Caller, Reject};
use spike_v2_read::{connect_owned_with_url, sqlgen, OwnedConnection};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::{sync::LazyLock, time::Duration};
use tokio::sync::oneshot;

const DATABASE: &str = aip_test_support::test_database_url!();

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; likes: Int }
  rows read when true
  transition like {
    allow actor != null
    from true
    to likes = this.likes + 1
  }
  expose read { select id, likes; budget { rows 10; depth 1; deadline 1s; cost 100 } }
  expose apply like { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 5; transitions like }
}
";

// sqlgen의 schema는 프로세스당 하나라서 테스트마다 같은 schema를 다시 만들고, 서로 겹치지 않게 순서대로 돌린다.
static SCHEMA_NAME: LazyLock<String> = LazyLock::new(|| {
    format!("aip_v3_commit_class_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
});
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn who() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() }
}

fn like(id: i64) -> Value {
    json!({ "apply": "Product.like", "target": { "ids": [id.to_string()] } })
}

#[derive(Clone, Copy, Debug)]
enum Path {
    Apply,
    Bundle,
    Compose,
}

struct Fixture {
    direct: OwnedConnection,
    victim: OwnedConnection,
    facts: Value,
    fired: oneshot::Receiver<String>,
    _gate: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new(fault: Fault) -> Self {
        let gate = GATE.lock().await;
        sqlgen::set_schema(&SCHEMA_NAME);
        let facts = load_str(DEF, Form::A).unwrap().execution;
        let direct = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
        for statement in sqlgen::ddl(&facts).unwrap() {
            direct.batch_execute(&statement).await.unwrap();
        }
        let s = sqlgen::schema();
        direct
            .batch_execute(&format!(
                "INSERT INTO {s}.member (id) VALUES (1); INSERT INTO {s}.product (id, likes) VALUES (42, 0), (43, 0), (44, 0), (45, 0)"
            ))
            .await
            .unwrap();
        let (address, fired) = start(fault).await;
        let victim =
            connect_owned_with_url(&aip_test_support::database_url_via_proxy(address.port())).await.expect("connect through the fault proxy");
        Self { direct, victim, facts, fired, _gate: gate }
    }

    async fn likes(&self, id: i64) -> i64 {
        self.direct.query_one(&format!("SELECT likes FROM {}.product WHERE id=$1", sqlgen::schema()), &[&id]).await.unwrap().get(0)
    }

    /// 장애가 실제로 일어난 단계를 돌려준다. 일어나지 않았다면 그 장애는 이 테스트가 겨냥한 단계에 닿지 못한 것이다.
    async fn phase(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(2), &mut self.fired)
            .await
            .expect("the proxy must have injected its fault")
            .expect("fault observation channel")
    }

    async fn run(&mut self, path: Path, id: i64, knobs: &Knobs) -> Result<(), Reject> {
        match path {
            Path::Apply => apply(&mut self.victim, &self.facts, &like(id), &who(), knobs).await.map(|_| ()),
            Path::Bundle => bundle(&mut self.victim, &self.facts, &json!({ "atomic": [like(id)] }), &who(), knobs).await.map(|_| ()),
            Path::Compose => {
                let request = json!({ "compose": "Product", "targets": { "ids": [id.to_string()] }, "steps": [{ "transition": "like" }] });
                compose(&mut self.victim, &self.facts, &request, &who(), knobs).await.map(|_| ())
            }
        }
    }

    async fn finish(self) {
        let schema = sqlgen::schema().to_string();
        drop(self.victim);
        self.direct.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    }
}

/// COMMIT이 보내지기 전에 연결이 끊기면 아무것도 커밋되지 않았다.
async fn assert_cut_before_commit(path: Path, fault: Fault) {
    let mut f = Fixture::new(fault).await;
    let outcome = f.run(path, 42, &Knobs::default()).await;
    let phase = f.phase().await;
    assert_eq!(outcome.as_ref().map_err(|e| e.code), Err("WIRE.DB_UNAVAILABLE"), "{path:?} {fault:?}: phase={phase}; {outcome:?}");
    assert_eq!(f.likes(42).await, 0, "{path:?} {fault:?}: nothing was committed");
    f.finish().await;
}

/// COMMIT을 보낸 뒤 응답을 받지 못하면, 서버에 닿았는지와 무관하게 이 연결로는 알 수 없다.
async fn assert_commit_unknown(path: Path, fault: Fault, committed: bool) {
    let mut f = Fixture::new(fault).await;
    let outcome = f.run(path, 42, &Knobs::default()).await;
    let phase = f.phase().await;
    assert_eq!(outcome.as_ref().map_err(|e| e.code), Err("WIRE.COMMIT_UNKNOWN"), "{path:?} {fault:?}: phase={phase}; {outcome:?}");
    assert_eq!(f.likes(42).await, i64::from(committed), "{path:?} {fault:?}: phase={phase}");
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_disconnect_while_beginning_the_transaction_is_db_unavailable() {
    assert_cut_before_commit(Path::Apply, Fault::CutAfterStatement("START TRANSACTION")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_disconnect_before_the_transaction_settings_is_db_unavailable() {
    assert_cut_before_commit(Path::Apply, Fault::CutAfterStatement("SET LOCAL statement_timeout")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_disconnect_after_update_before_commit_is_db_unavailable() {
    assert_cut_before_commit(Path::Apply, Fault::CutAfterStatement("UPDATE")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_disconnect_after_update_before_commit_is_db_unavailable() {
    assert_cut_before_commit(Path::Bundle, Fault::CutAfterStatement("UPDATE")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_commit_withheld_from_postgres_is_commit_unknown() {
    assert_commit_unknown(Path::Apply, Fault::CutOnCommitSent, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_ack_dropped_with_fin_is_commit_unknown() {
    assert_commit_unknown(Path::Apply, Fault::AckDropFin, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_connection_reset_after_commit_ack_is_commit_unknown() {
    assert_commit_unknown(Path::Apply, Fault::AckDropReset, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_unparseable_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Path::Apply, Fault::ReplaceCommitAck(b'x'), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_unexpected_message_in_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Path::Apply, Fault::ReplaceCommitAck(b'n'), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_commit_withheld_from_postgres_is_commit_unknown() {
    assert_commit_unknown(Path::Bundle, Fault::CutOnCommitSent, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_ack_dropped_with_fin_is_commit_unknown() {
    assert_commit_unknown(Path::Bundle, Fault::AckDropFin, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compose_disconnect_after_update_before_commit_is_db_unavailable() {
    assert_cut_before_commit(Path::Compose, Fault::CutAfterStatement("UPDATE")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compose_ack_dropped_with_fin_is_commit_unknown() {
    assert_commit_unknown(Path::Compose, Fault::AckDropFin, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compose_commit_on_an_already_closed_connection_is_db_unavailable() {
    assert_closed_before_commit_is_db_unavailable(Path::Compose).await;
}

/// 이미 닫힌 연결에는 COMMIT이 나가지 못한다. 서버는 끊긴 연결의 열린 트랜잭션을 rollback하므로 결과가 불확실하지 않다.
async fn assert_closed_before_commit_is_db_unavailable(path: Path) {
    // 발동하지 않는 장애(COMMIT 성공 태그 교체는 COMMIT이 나가야 일어난다)로 두고, 쓰기가 commit 직전에 멈춘 동안 backend를 종료한다.
    let mut f = Fixture::new(Fault::ReplaceCommitAck(b'x')).await;
    let pid: i32 = f.victim.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let killer = tokio::spawn(async move {
        let control = connect_owned_with_url(DATABASE).await.expect("control connection");
        tokio::time::sleep(Duration::from_millis(400)).await;
        control.execute("SELECT pg_terminate_backend($1)", &[&pid]).await.unwrap();
    });
    let knobs = Knobs { pause_before_commit_ms: 1200, ..Knobs::default() };
    let outcome = f.run(path, 42, &knobs).await;
    killer.await.unwrap();
    assert_eq!(outcome.as_ref().map_err(|e| e.code), Err("WIRE.DB_UNAVAILABLE"), "{path:?}: COMMIT never left the client; {outcome:?}");
    assert_eq!(f.likes(42).await, 0);
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_commit_on_an_already_closed_connection_is_db_unavailable() {
    assert_closed_before_commit_is_db_unavailable(Path::Apply).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_commit_on_an_already_closed_connection_is_db_unavailable() {
    assert_closed_before_commit_is_db_unavailable(Path::Bundle).await;
}

/// 서버가 거절한 COMMIT(지연 제약 위반)은 rollback이 확정이다. COMMIT_UNKNOWN이나 DB_UNAVAILABLE로 넓히지 않는다.
/// proxy 장애는 COMMIT 성공 태그에만 걸리므로 거절된 COMMIT에는 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn definite_commit_rejections_keep_their_codes() {
    let mut f = Fixture::new(Fault::AckDropFin).await;
    let s = sqlgen::schema();
    f.direct
        .batch_execute(&format!(
            "CREATE FUNCTION {s}.reject_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id=43 THEN RAISE EXCEPTION 'deadlock fixture' USING ERRCODE='40P01'; ELSIF NEW.id=44 THEN RAISE EXCEPTION 'check fixture' USING ERRCODE='23514'; ELSIF NEW.id=45 THEN RAISE EXCEPTION 'unique fixture' USING ERRCODE='23505'; END IF; RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER reject_commit AFTER UPDATE ON {s}.product DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.reject_commit()"
        ))
        .await
        .unwrap();
    for (id, code) in [(43, "WIRE.CONFLICT"), (44, "WIRE.BAD_VALUE"), (45, "WIRE.ALREADY_EXISTS")] {
        let outcome = f.run(Path::Apply, id, &Knobs::default()).await;
        assert_eq!(outcome.as_ref().map_err(|e| e.code), Err(code), "id {id}: a rejected COMMIT is definite: {outcome:?}");
        assert_eq!(f.likes(id).await, 0, "id {id}: rollback");
    }
    f.finish().await;
}

/// 연결 오류 판정은 서버가 돌려준 오류나 로컬 직렬화 오류까지 넓히지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_failure_predicate_ignores_server_and_local_errors() {
    let db = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
    let to_sql = db.query_one("SELECT $1::int4", &[&"not an integer"]).await.expect_err("a text parameter cannot be encoded as int4");
    assert!(!spike_v3_write::connection_failed(&to_sql), "parameter serialization failure is a local error: {to_sql}");
    let division = db.query_one("SELECT 1/0", &[]).await.expect_err("division by zero");
    assert!(!spike_v3_write::connection_failed(&division), "server error whose SQLSTATE means rollback: {division}");
    let syntax = db.batch_execute("SELECT 1 WHERE").await.expect_err("syntax error");
    assert!(!spike_v3_write::connection_failed(&syntax), "syntax error: {syntax}");
}
