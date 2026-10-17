//! `write_extension::apply`의 연결 오류 분류(WBS 8.9). COMMIT을 보내기 전에 끊기면 DB_UNAVAILABLE(커밋 없음, 같은 키 재시도 안전),
//! 보낸 뒤 끊기거나 응답을 해석할 수 없으면 COMMIT_UNKNOWN(같은 키로 확정), 서버가 거절한 오류는 기존 코드를 유지한다.
use aip_test_fault_proxy::{start, Fault};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, plan::Caller, sqlgen, OwnedConnection};
use spike_v6_transport::{
    idempotency_ddl,
    server::{ReadExtensions, WorkerLang},
    write_extension,
};
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
}
"#;

const EXTENSION: &str = "export async function run(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); return {done:true}; }\n";

// sqlgen의 schema는 프로세스당 하나라서 테스트마다 같은 schema를 다시 만들고, 서로 겹치지 않게 순서대로 돌린다.
static SCHEMA_NAME: LazyLock<String> = LazyLock::new(|| {
    format!("aip_v6_extcommit_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
});
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    direct: OwnedConnection,
    proxy_url: String,
    config: ReadExtensions,
    facts: Value,
    fired: oneshot::Receiver<String>,
    _dir: OwnedDir,
    _gate: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new(fault: Fault) -> Self {
        let gate = GATE.lock().await;
        let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let dir = std::env::temp_dir().join(format!("aip-v6-extcommit-{unique}"));
        fs::create_dir(&dir).unwrap();
        let dir = OwnedDir(dir);
        fs::write(dir.0.join("write.mjs"), EXTENSION).unwrap();
        sqlgen::set_schema(&SCHEMA_NAME);
        let facts = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
        let direct = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
        for statement in sqlgen::ddl(&facts).unwrap() {
            direct.batch_execute(&statement).await.unwrap();
        }
        direct.batch_execute(&idempotency_ddl()).await.unwrap();
        let s = sqlgen::schema();
        direct
            .batch_execute(&format!(
                "INSERT INTO {s}.member(id) VALUES (1); INSERT INTO {s}.event(id,member_id,checked) VALUES (11,1,false),(21,1,false),(22,1,false),(23,1,false)"
            ))
            .await
            .unwrap();
        let (address, fired) = start(fault).await;
        let proxy_url = aip_test_support::database_url_via_proxy(address.port());
        let config = ReadExtensions { lang: WorkerLang::Node, dir: dir.0.clone() };
        Self { direct, proxy_url, config, facts, fired, _dir: dir, _gate: gate }
    }

    async fn number(&self, sql: &str) -> i64 {
        self.direct.query_one(&sql.replace("{schema}", sqlgen::schema()), &[]).await.unwrap().get(0)
    }

    /// 장애가 실제로 일어난 단계를 돌려준다. 일어나지 않았다면 그 장애는 이 테스트가 겨냥한 단계에 닿지 못한 것이다.
    async fn phase(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(2), &mut self.fired)
            .await
            .expect("the proxy must have injected its fault")
            .expect("fault observation channel")
    }

    /// 요청마다 proxy를 거치는 새 연결을 쓴다. 끊긴 연결은 다시 쓸 수 없고, 서버도 요청마다 연결을 새로 얻는다.
    async fn write(&mut self, key: &str, id: i64) -> Value {
        let mut victim = connect_owned_with_url(&self.proxy_url).await.expect("connect through the fault proxy");
        let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
        let body = json!({"key": key, "request": {"extension": "Event.run", "input": {"id": id.to_string()}}});
        write_extension::apply(&mut victim, &self.facts, &body, &caller, IdWire::DecimalString, &self.config).await
    }

    async fn finish(self) {
        let schema = sqlgen::schema().to_string();
        self.direct.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    }
}

const CHECKED_11: &str = "SELECT count(*) FROM {schema}.event WHERE id=11 AND checked";
const IDEM_ROWS: &str = "SELECT count(*) FROM {schema}.aip_idem";

/// COMMIT이 보내지기 전에 연결이 끊기면 아무것도 커밋되지 않았다. 같은 키로 다시 보내면 새로 실행된다.
async fn assert_cut_before_commit(fault: Fault) {
    let mut f = Fixture::new(fault).await;
    let first = f.write("cut-before-commit", 11).await;
    let phase = f.phase().await;
    assert_eq!(first["code"], "WIRE.DB_UNAVAILABLE", "{fault:?}: phase={phase}; body={first}");
    assert_eq!(f.number(CHECKED_11).await, 0, "{fault:?}: nothing was committed");
    assert_eq!(f.number(IDEM_ROWS).await, 0, "{fault:?}: no idempotency record without a commit");
    let retry = f.write("cut-before-commit", 11).await;
    assert_eq!(retry["ok"], true, "{fault:?}: same key must run again after a safe-retry error: {retry}");
    assert!(retry.get("replayed").is_none(), "{fault:?}: nothing was stored, so this is a fresh execution: {retry}");
    assert_eq!(f.number(CHECKED_11).await, 1);
    f.finish().await;
}

/// COMMIT을 보낸 뒤 응답을 받지 못하면, 서버에 닿았는지와 무관하게 이 연결로는 알 수 없다.
async fn assert_commit_unknown(fault: Fault, committed: bool) {
    let mut f = Fixture::new(fault).await;
    let first = f.write("cut-after-commit", 11).await;
    let phase = f.phase().await;
    assert_eq!(first["code"], "WIRE.COMMIT_UNKNOWN", "{fault:?}: phase={phase}; body={first}");
    assert_eq!(f.number(CHECKED_11).await, i64::from(committed), "{fault:?}: phase={phase}");
    assert_eq!(f.number(IDEM_ROWS).await, i64::from(committed), "{fault:?}: phase={phase}");
    let retry = f.write("cut-after-commit", 11).await;
    assert_eq!(retry["ok"], true, "{fault:?}: same key must settle the unknown outcome: {retry}");
    assert_eq!(retry["replayed"] == true, committed, "{fault:?}: stored result is replayed only if the first COMMIT reached PostgreSQL: {retry}");
    assert_eq!(f.number(CHECKED_11).await, 1, "{fault:?}: the change exists exactly once");
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_while_beginning_the_transaction_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("START TRANSACTION")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_before_the_transaction_settings_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("SET LOCAL statement_timeout")).await;
}

/// 확장 실행(v4 run_write_in)이 자기 트랜잭션 설정을 보내는 문장이다. `write_extension` 자신의 설정 문장과 달리 `=` 앞뒤에 공백이 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_during_the_extension_session_settings_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("statement_timeout = ")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_after_the_advisory_lock_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("pg_advisory_xact_lock")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_during_the_extension_write_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("UPDATE")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_after_the_idempotency_insert_before_commit_is_db_unavailable() {
    assert_cut_before_commit(Fault::CutAfterStatement("aip_idem(principal,key,request,result)")).await;
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
    for (id, code) in [(21, "WIRE.CONFLICT"), (22, "WIRE.BAD_VALUE"), (23, "WIRE.ALREADY_EXISTS")] {
        let outcome = f.write(&format!("rejected-{id}"), id).await;
        assert_eq!(outcome["code"], code, "id {id}: a rejected COMMIT is definite: {outcome}");
    }
    assert_eq!(f.number("SELECT count(*) FROM {schema}.event WHERE id IN (21,22,23) AND checked").await, 0);
    assert_eq!(f.number(IDEM_ROWS).await, 0);
    f.finish().await;
}
