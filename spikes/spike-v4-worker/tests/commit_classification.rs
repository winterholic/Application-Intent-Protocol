//! `invoke_write`의 연결 오류 분류(WBS 8.9). COMMIT을 보내기 전에 끊기면 DB_UNAVAILABLE(커밋 없음),
//! 보낸 뒤 끊기거나 응답을 해석할 수 없으면 COMMIT_UNKNOWN, 서버가 거절한 오류는 기존 코드를 유지한다.
use aip_test_fault_proxy::{start, Fault};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{Caller, Reject};
use spike_v2_read::{connect_owned_with_url, sqlgen, OwnedConnection};
use spike_v4_worker::write::invoke_write;
use spike_v4_worker::{Isolation, Lang, Worker, WorkerLimits};
use std::{fs, path::PathBuf, sync::LazyLock, time::Duration};
use tokio::sync::oneshot;

const DATABASE: &str = aip_test_support::test_database_url!();

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Event {
  fields { id: Id; member: Member; checked: Bool }
  rows read when member = actor
  transition mark { allow member = actor; from checked = false; to checked = true }
  expose apply mark { target id; bulk maxRows 2 }
  extension write run {
    input { id: Event.Id }
    output { done: Bool }
    access Event.mark
    effect db
    deadline 2s
    implementation "write.run"
  }
  extension write slow {
    input { id: Event.Id }
    output { done: Bool }
    access Event.mark
    effect db
    deadline 3s
    implementation "write.slow"
  }
}
"#;

const EXTENSION: &str = "export async function run(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); return {done:true}; }
export async function slow(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); await new Promise((resolve) => setTimeout(resolve, 1200)); return {done:true}; }
";

// sqlgen의 schema는 프로세스당 하나라서 테스트마다 같은 schema를 다시 만들고, 서로 겹치지 않게 순서대로 돌린다.
static SCHEMA_NAME: LazyLock<String> = LazyLock::new(|| {
    format!("aip_v4_commit_class_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
});
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn who() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() }
}

struct Fixture {
    direct: OwnedConnection,
    victim: OwnedConnection,
    worker: Worker,
    facts: Value,
    fired: oneshot::Receiver<String>,
    _dir: OwnedDir,
    _gate: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new(fault: Fault) -> Self {
        let gate = GATE.lock().await;
        let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let dir = std::env::temp_dir().join(format!("aip-v4-commit-class-{unique}"));
        fs::create_dir(&dir).unwrap();
        let dir = OwnedDir(dir);
        fs::write(dir.0.join("write.mjs"), EXTENSION).unwrap();
        sqlgen::set_schema(&SCHEMA_NAME);
        let facts = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
        let direct = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
        for statement in sqlgen::ddl(&facts).unwrap() {
            direct.batch_execute(&statement).await.unwrap();
        }
        let s = sqlgen::schema();
        direct
            .batch_execute(&format!(
                "INSERT INTO {s}.member(id) VALUES (1); INSERT INTO {s}.event(id,member_id,checked) VALUES (11,1,false),(21,1,false),(22,1,false),(23,1,false)"
            ))
            .await
            .unwrap();
        let (address, fired) = start(fault).await;
        let victim =
            connect_owned_with_url(&aip_test_support::database_url_via_proxy(address.port())).await.expect("connect through the fault proxy");
        let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
        let worker = Worker::try_start_with(Lang::Node, dir.0.to_str().unwrap(), isolation, WorkerLimits::default()).await.expect("official worker");
        Self { direct, victim, worker, facts, fired, _dir: dir, _gate: gate }
    }

    async fn checked(&self, id: i64) -> bool {
        self.direct.query_one(&format!("SELECT checked FROM {}.event WHERE id=$1", sqlgen::schema()), &[&id]).await.unwrap().get(0)
    }

    /// 장애가 실제로 일어난 단계를 돌려준다. 일어나지 않았다면 그 장애는 이 테스트가 겨냥한 단계에 닿지 못한 것이다.
    async fn phase(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(2), &mut self.fired)
            .await
            .expect("the proxy must have injected its fault")
            .expect("fault observation channel")
    }

    async fn write(&mut self, extension: &str, id: i64) -> Result<Value, Reject> {
        invoke_write(&mut self.victim, &mut self.worker, &self.facts, &format!("Event.{extension}"), &json!({ "id": id.to_string() }), &who()).await
    }

    async fn finish(self) {
        self.worker.stop().await;
        let schema = sqlgen::schema().to_string();
        drop(self.victim);
        self.direct.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    }
}

fn code(outcome: &Result<Value, Reject>) -> Result<(), &'static str> {
    outcome.as_ref().map(|_| ()).map_err(|e| e.code)
}

/// COMMIT이 보내지기 전에 연결이 끊기면 아무것도 커밋되지 않았다.
async fn assert_cut_before_commit(fault: Fault) {
    let mut f = Fixture::new(fault).await;
    let outcome = f.write("run", 11).await;
    let phase = f.phase().await;
    assert_eq!(code(&outcome), Err("WIRE.DB_UNAVAILABLE"), "{fault:?}: phase={phase}; {outcome:?}");
    assert!(!f.checked(11).await, "{fault:?}: nothing was committed");
    f.finish().await;
}

/// COMMIT을 보낸 뒤 응답을 받지 못하면, 서버에 닿았는지와 무관하게 이 연결로는 알 수 없다.
async fn assert_commit_unknown(fault: Fault, committed: bool) {
    let mut f = Fixture::new(fault).await;
    let outcome = f.write("run", 11).await;
    let phase = f.phase().await;
    assert_eq!(code(&outcome), Err("WIRE.COMMIT_UNKNOWN"), "{fault:?}: phase={phase}; {outcome:?}");
    assert_eq!(f.checked(11).await, committed, "{fault:?}: phase={phase}");
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_during_the_extension_write_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("UPDATE")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_while_beginning_the_transaction_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("START TRANSACTION")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_before_the_transaction_settings_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("SET LOCAL statement_timeout")).await;
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
async fn connection_reset_after_commit_ack_is_commit_unknown() {
    assert_commit_unknown(Fault::AckDropReset, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unparseable_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Fault::ReplaceCommitAck(b'x'), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unexpected_message_in_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Fault::ReplaceCommitAck(b'n'), true).await;
}

/// 확장이 끝난 뒤 COMMIT 호출 전에 backend가 끝나면 COMMIT은 나가지 못한다. 서버는 열린 트랜잭션을 rollback하므로 결과가 불확실하지 않다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_on_an_already_closed_connection_is_db_unavailable() {
    // 발동하지 않는 장애(COMMIT 성공 태그 교체는 COMMIT이 나가야 일어난다)로 두고, 확장이 기다리는 동안 backend를 종료한다.
    let mut f = Fixture::new(Fault::ReplaceCommitAck(b'x')).await;
    let pid: i32 = f.victim.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let killer = tokio::spawn(async move {
        let control = connect_owned_with_url(DATABASE).await.expect("control connection");
        tokio::time::sleep(Duration::from_millis(700)).await;
        control.execute("SELECT pg_terminate_backend($1)", &[&pid]).await.unwrap();
    });
    let outcome = f.write("slow", 11).await;
    killer.await.unwrap();
    assert_eq!(code(&outcome), Err("WIRE.DB_UNAVAILABLE"), "COMMIT never left the client; {outcome:?}");
    assert!(!f.checked(11).await);
    f.finish().await;
}

/// 서버가 거절한 COMMIT(지연 제약 위반)은 rollback이 확정이다. COMMIT_UNKNOWN이나 DB_UNAVAILABLE로 넓히지 않는다.
/// proxy 장애는 COMMIT 성공 태그에만 걸리므로 거절된 COMMIT에는 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn definite_commit_rejections_keep_their_codes() {
    let mut f = Fixture::new(Fault::AckDropFin).await;
    let s = sqlgen::schema();
    f.direct
        .batch_execute(&format!(
            "CREATE FUNCTION {s}.reject_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id=21 THEN RAISE EXCEPTION 'deadlock fixture' USING ERRCODE='40P01'; ELSIF NEW.id=22 THEN RAISE EXCEPTION 'check fixture' USING ERRCODE='23514'; ELSIF NEW.id=23 THEN RAISE EXCEPTION 'unique fixture' USING ERRCODE='23505'; END IF; RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER reject_commit AFTER UPDATE ON {s}.event DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.reject_commit()"
        ))
        .await
        .unwrap();
    for (id, expected) in [(21, "WIRE.CONFLICT"), (22, "WIRE.BAD_VALUE"), (23, "WIRE.ALREADY_EXISTS")] {
        let outcome = f.write("run", id).await;
        assert_eq!(code(&outcome), Err(expected), "id {id}: a rejected COMMIT is definite: {outcome:?}");
        assert!(!f.checked(id).await, "id {id}: rollback");
    }
    f.finish().await;
}
