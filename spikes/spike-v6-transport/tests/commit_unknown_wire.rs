use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen, OwnedConnection};
use spike_v6_transport::server::{listen_with_apply_contract, Server};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc, LazyLock,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{oneshot, Notify},
};
use tokio_postgres::Client;

const SOURCE: &str = include_str!("../fixture/v14.aip");
const DATABASE: &str = aip_test_support::test_database_url!();

// sqlgen의 schema는 프로세스당 하나라서 테스트마다 같은 schema를 다시 만들고, 서로 겹치지 않게 순서대로 돌린다.
static SCHEMA_NAME: LazyLock<String> = LazyLock::new(|| {
    format!("aip_commit_unknown_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
});
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn raw_apply(address: SocketAddr, token: &str, fingerprint: &str, key: &str, id: &str) -> (String, Value) {
    let body = json!({"key": key, "request": {"apply": "Inbox.mark", "target": {"ids": [id]}}}).to_string();
    let mut socket = TcpStream::connect(address).await.expect("HTTP server connection");
    socket.write_all(format!(
        "POST /apply HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()
    ).as_bytes()).await.expect("write apply request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), socket.read_to_end(&mut response))
        .await
        .expect("read HTTP response deadline")
        .expect("read HTTP response");
    let text = String::from_utf8(response).expect("HTTP response UTF-8");
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response headers");
    (head.lines().next().unwrap_or_default().to_string(), serde_json::from_str(body).expect("HTTP JSON response"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// 서버가 COMMIT 성공 태그를 보낸 뒤, 응답을 막고 FIN으로 닫는다.
    AckDropFin,
    /// 같은 시점에 RST로 끊는다.
    AckDropReset,
    /// 트랜잭션 시작(START TRANSACTION)이 서버에서 끝난 직후 응답을 막고 닫는다. 문장은 하나도 보내지지 않았다.
    CutAtBegin,
    /// 전이의 UPDATE가 서버에서 끝난 직후 응답을 막고 닫는다. COMMIT은 아직 보내지지 않았다.
    CutAfterUpdate,
    /// 멱등 기록 INSERT가 서버에서 끝난 직후 응답을 막고 닫는다. COMMIT 바로 앞이며 COMMIT은 보내지지 않았다.
    CutAfterIdemInsert,
    /// 클라이언트가 COMMIT을 보내면 서버에 전달하지 않고 닫는다.
    CutOnCommitSent,
    /// 클라이언트가 COMMIT을 보내면 서버에 전달하지 않고 서버 backend를 pg_terminate_backend로 끝낸다(FATAL 57P01).
    TerminateAtCommit,
    /// 서버의 COMMIT 성공 태그를 주어진 태그의 빈 메시지로 바꿔 전달한다.
    ReplaceCommitAck(u8),
}

#[derive(PartialEq)]
enum Frontend {
    Commit,
    IdemInsert,
}

fn scan_frontend(pending: &mut Vec<u8>, startup: &mut bool) -> Vec<Frontend> {
    let mut events = Vec::new();
    let mut offset = 0;
    while pending.len().saturating_sub(offset) >= if *startup { 5 } else { 4 } {
        if !*startup {
            let len = u32::from_be_bytes(pending[offset..offset + 4].try_into().unwrap()) as usize;
            if len < 8 || pending.len() - offset < len {
                break;
            }
            offset += len;
            *startup = true;
            continue;
        }
        let len = u32::from_be_bytes(pending[offset + 1..offset + 5].try_into().unwrap()) as usize;
        if len < 4 || pending.len() - offset < len + 1 {
            break;
        }
        let body = &pending[offset + 5..offset + 1 + len];
        match pending[offset] {
            b'Q' if body.iter().copied().take_while(|byte| *byte != 0).collect::<Vec<_>>().eq_ignore_ascii_case(b"COMMIT") => {
                events.push(Frontend::Commit)
            }
            b'P' if contains(body, b"INSERT INTO") && contains(body, b"aip_idem") => events.push(Frontend::IdemInsert),
            _ => {}
        }
        offset += len + 1;
    }
    if offset > 0 {
        pending.drain(..offset);
    }
    events
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

struct Armed {
    fault: Fault,
    armed: AtomicBool,
    fired: tokio::sync::Mutex<Option<oneshot::Sender<String>>>,
    control: OwnedConnection,
}

impl Armed {
    /// 장애는 한 번만 일으킨다. 같은 키로 다시 보낸 요청은 정상 경로로 간다.
    fn take(&self) -> bool {
        self.armed.swap(false, Ordering::SeqCst)
    }

    async fn fire(&self, phase: &str) {
        if let Some(sender) = self.fired.lock().await.take() {
            let _ = sender.send(phase.to_owned());
        }
    }
}

async fn proxy_connection(caller: TcpStream, backend_address: SocketAddr, state: Arc<Armed>) {
    let Ok(backend) = TcpStream::connect(backend_address).await else {
        return;
    };
    // 프록시는 메시지를 헤더·본문 두 번에 나눠 쓴다. Nagle이 켜져 있으면 Linux에서 상대의 지연 ACK(약 40ms)를 기다려 메시지마다 멈춘다.
    let _ = caller.set_nodelay(true);
    let _ = backend.set_nodelay(true);
    let (mut caller_read, mut caller_write) = caller.into_split();
    let (mut backend_read, mut backend_write) = backend.into_split();
    let pid = Arc::new(AtomicI32::new(0));
    let idem_insert_sent = Arc::new(AtomicBool::new(false));
    let cut = Arc::new(Notify::new());
    let front = tokio::spawn({
        let (state, pid, idem_insert_sent, cut) = (state.clone(), pid.clone(), idem_insert_sent.clone(), cut.clone());
        async move {
            let (mut pending, mut startup) = (Vec::new(), false);
            let mut buf = [0u8; 8192];
            loop {
                let Ok(n) = caller_read.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                pending.extend_from_slice(&buf[..n]);
                let events = scan_frontend(&mut pending, &mut startup);
                if events.contains(&Frontend::IdemInsert) {
                    idem_insert_sent.store(true, Ordering::SeqCst);
                }
                // COMMIT은 따로 한 번의 쓰기로 온다(batch_execute). 가로챈 COMMIT은 서버에 전달하지 않는다.
                if events.contains(&Frontend::Commit) {
                    if state.fault == Fault::CutOnCommitSent && state.take() {
                        state.fire("COMMIT was sent by the client and withheld from PostgreSQL; connection closed").await;
                        cut.notify_one();
                        return;
                    }
                    if state.fault == Fault::TerminateAtCommit && state.take() {
                        let target = pid.load(Ordering::SeqCst);
                        // 백엔드가 닫히면 아래 루프가 front를 abort한다. 종료 요청과 fire가 거기서 잘리지 않게 떼어 실행한다.
                        let state = state.clone();
                        let _ = tokio::spawn(async move {
                            state.control.execute("SELECT pg_terminate_backend($1)", &[&target]).await.expect("terminate the proxied backend");
                            state.fire("COMMIT was sent by the client and withheld; backend terminated with pg_terminate_backend").await;
                        })
                        .await;
                        continue;
                    }
                }
                if backend_write.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        }
    });
    loop {
        let mut header = [0u8; 5];
        let read = tokio::select! {
            read = backend_read.read_exact(&mut header) => read,
            _ = cut.notified() => break,
        };
        if read.is_err() {
            break;
        }
        let len = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
        if !(4..=16 * 1024 * 1024).contains(&len) {
            break;
        }
        let mut body = vec![0u8; len - 4];
        if backend_read.read_exact(&mut body).await.is_err() {
            break;
        }
        if header[0] == b'K' && body.len() == 8 {
            pid.store(i32::from_be_bytes(body[..4].try_into().unwrap()), Ordering::SeqCst);
        }
        let command_complete = header[0] == b'C';
        let is_commit_complete = command_complete && body.strip_suffix(&[0]).is_some_and(|tag| tag == b"COMMIT");
        match state.fault {
            Fault::AckDropFin | Fault::AckDropReset if is_commit_complete && state.take() => {
                state.fire("CommandComplete(COMMIT) observed from owned PostgreSQL; response withheld").await;
                front.abort();
                if state.fault == Fault::AckDropReset {
                    // 닫을 때 RST를 보내려면 SO_LINGER 0이 필요하다. 테스트 proxy라 닫기 지연은 문제가 아니다.
                    #[allow(deprecated)]
                    let _ = caller_write.as_ref().set_linger(Some(Duration::ZERO));
                } else {
                    let _ = caller_write.shutdown().await;
                }
                return;
            }
            Fault::CutAtBegin if command_complete && body.starts_with(b"START TRANSACTION") && state.take() => {
                state.fire("CommandComplete(START TRANSACTION) observed from owned PostgreSQL; response withheld before any statement").await;
                front.abort();
                let _ = caller_write.shutdown().await;
                return;
            }
            Fault::CutAfterUpdate if command_complete && body.starts_with(b"UPDATE") && state.take() => {
                state.fire("CommandComplete(UPDATE) observed from owned PostgreSQL; response withheld before COMMIT").await;
                front.abort();
                let _ = caller_write.shutdown().await;
                return;
            }
            Fault::CutAfterIdemInsert
                if command_complete && body.starts_with(b"INSERT") && idem_insert_sent.load(Ordering::SeqCst) && state.take() =>
            {
                state.fire("CommandComplete(INSERT aip_idem) observed from owned PostgreSQL; response withheld before COMMIT").await;
                front.abort();
                let _ = caller_write.shutdown().await;
                return;
            }
            Fault::ReplaceCommitAck(tag) if is_commit_complete && state.take() => {
                state.fire("CommandComplete(COMMIT) observed from owned PostgreSQL; replaced with an unexpected message").await;
                header = [tag, 0, 0, 0, 4];
                body.clear();
            }
            _ => {}
        }
        if caller_write.write_all(&header).await.is_err() || caller_write.write_all(&body).await.is_err() {
            break;
        }
    }
    front.abort();
}

async fn start_proxy(fault: Fault) -> (SocketAddr, oneshot::Receiver<String>) {
    let (backend_host, backend_port) = aip_test_support::endpoint(DATABASE).unwrap();
    let backend_address = std::net::ToSocketAddrs::to_socket_addrs(&(backend_host.as_str(), backend_port))
        .ok()
        .and_then(|mut addresses| addresses.next())
        .expect("resolve the test database endpoint");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = oneshot::channel();
    let state = Arc::new(Armed {
        fault,
        armed: AtomicBool::new(true),
        fired: tokio::sync::Mutex::new(Some(sender)),
        control: connect_owned_with_url(DATABASE).await.expect("control connection to run-owned PostgreSQL only"),
    });
    tokio::spawn(async move {
        loop {
            let Ok((caller, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(proxy_connection(caller, backend_address, state.clone()));
        }
    });
    (address, receiver)
}

async fn seed(db: &Client, schema: &str, facts: &Value) {
    for statement in sqlgen::ddl(facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(db).await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.inbox(id,member_id,checked,count,title) VALUES (42,1,false,1,'commit unknown'),(43,1,false,1,'serialization error'),(44,1,false,1,'deadlock error'),(45,1,false,1,'check violation'),(46,1,false,1,'unique violation')")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_reports_commit_unknown_when_postgres_ack_is_cut_after_commit_and_same_key_replays() {
    let _gate = GATE.lock().await;
    sqlgen::set_schema(&SCHEMA_NAME);
    let schema = sqlgen::schema();
    let mut facts = load_str(SOURCE, Form::A).unwrap().execution;
    facts["resources"]["Inbox"]["transitions"]["mark"]["isolation"] = json!("serializable");
    let direct = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
    seed(&direct, schema, &facts).await;

    let (proxy_address, commit_observed) = start_proxy(Fault::AckDropFin).await;
    let proxy_url = aip_test_support::database_url_via_proxy(proxy_address.port());
    let server = listen_with_apply_contract(facts.clone(), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), proxy_url).await.unwrap();
    let token = server.keys.issue(1, 600);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&facts, IdWire::DecimalString);
    let (status, first) = raw_apply(server.address, &token, &fingerprint, "commit-cut-once", "42").await;
    let phase = tokio::time::timeout(Duration::from_secs(2), commit_observed)
        .await
        .expect("proxy must observe the PostgreSQL COMMIT success tag before closing")
        .expect("COMMIT observation channel");
    let changed: bool = direct.query_one(&format!("SELECT checked FROM {schema}.inbox WHERE id=42"), &[]).await.unwrap().get(0);
    let records: i64 = direct.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get(0);
    eprintln!("commit proxy phase={phase}; HTTP={status}; response={first}; committed_effect={changed}; idem_rows={records}");
    assert!(status.starts_with("HTTP/1.1 200"), "commit outcome should be an HTTP success envelope; phase={phase}; status={status}; body={first}");
    assert_eq!(first["code"], "WIRE.COMMIT_UNKNOWN", "phase={phase}; actual wire outcome={first}");
    assert!(changed, "COMMIT success tag was observed but the effect is absent");
    assert_eq!(records, 1, "commit result reservation must be written exactly once");

    let (retry_status, retry) = raw_apply(server.address, &token, &fingerprint, "commit-cut-once", "42").await;
    assert!(retry_status.starts_with("HTTP/1.1 200"));
    assert_eq!(retry["ok"], true);
    assert_eq!(retry["replayed"], true, "same idempotency key must replay committed result");
    assert_eq!(direct.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get::<_, i64>(0), 1);
    assert_eq!(direct.query_one(&format!("SELECT count(*) FROM {schema}.inbox WHERE id=42 AND checked"), &[]).await.unwrap().get::<_, i64>(0), 1);

    direct.batch_execute(&format!("CREATE SEQUENCE {schema}.commit_attempts; CREATE FUNCTION {schema}.reject_commit_concurrency() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id IN (43,44) THEN PERFORM nextval('{schema}.commit_attempts'); IF NEW.id=43 THEN RAISE EXCEPTION 'commit serialization fixture' USING ERRCODE='40001'; ELSE RAISE EXCEPTION 'commit deadlock fixture' USING ERRCODE='40P01'; END IF; END IF; RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER reject_commit_concurrency AFTER UPDATE ON {schema}.inbox DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {schema}.reject_commit_concurrency()")).await.unwrap();
    let (conflict_status, conflict) = raw_apply(server.address, &token, &fingerprint, "commit-serialization", "43").await;
    assert!(conflict_status.starts_with("HTTP/1.1 200"));
    assert_eq!(conflict["code"], "WIRE.CONFLICT", "PostgreSQL SQLSTATE 40001 confirms rollback and remains server-retryable: {conflict}");
    assert!(conflict["msg"].as_str().unwrap_or_default().ends_with("다시 시도 가능"), "serialization conflict must retain retry marker: {conflict}");
    assert!(
        !direct.query_one(&format!("SELECT checked FROM {schema}.inbox WHERE id=43"), &[]).await.unwrap().get::<_, bool>(0),
        "failed COMMIT must leave the row unchanged"
    );
    assert_eq!(
        direct.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get::<_, i64>(0),
        1,
        "rollback must not leave an idempotency reservation"
    );
    assert_eq!(
        direct.query_one(&format!("SELECT last_value FROM {schema}.commit_attempts"), &[]).await.unwrap().get::<_, i64>(0),
        4,
        "40001 is retried exactly three times after the initial attempt"
    );
    let (deadlock_status, deadlock) = raw_apply(server.address, &token, &fingerprint, "commit-deadlock", "44").await;
    assert!(deadlock_status.starts_with("HTTP/1.1 200"));
    assert_eq!(deadlock["code"], "WIRE.CONFLICT", "PostgreSQL SQLSTATE 40P01 confirms rollback and remains server-retryable: {deadlock}");
    assert!(deadlock["msg"].as_str().unwrap_or_default().ends_with("다시 시도 가능"), "deadlock conflict must retain retry marker: {deadlock}");
    assert!(!direct.query_one(&format!("SELECT checked FROM {schema}.inbox WHERE id=44"), &[]).await.unwrap().get::<_, bool>(0));
    assert_eq!(direct.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get::<_, i64>(0), 1);
    assert_eq!(
        direct.query_one(&format!("SELECT last_value FROM {schema}.commit_attempts"), &[]).await.unwrap().get::<_, i64>(0),
        8,
        "40P01 is retried exactly three times after the initial attempt"
    );
    server.shutdown().await.unwrap();
    direct.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

struct Fixture {
    direct: OwnedConnection,
    server: Server,
    token: String,
    fingerprint: String,
    fired: oneshot::Receiver<String>,
    _gate: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new(fault: Fault) -> Self {
        let gate = GATE.lock().await;
        sqlgen::set_schema(&SCHEMA_NAME);
        let facts = load_str(SOURCE, Form::A).unwrap().execution;
        let direct = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
        seed(&direct, sqlgen::schema(), &facts).await;
        let (proxy_address, fired) = start_proxy(fault).await;
        let proxy_url = aip_test_support::database_url_via_proxy(proxy_address.port());
        let server = listen_with_apply_contract(facts.clone(), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), proxy_url).await.unwrap();
        let token = server.keys.issue(1, 600);
        let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&facts, IdWire::DecimalString);
        Self { direct, server, token, fingerprint, fired, _gate: gate }
    }

    async fn apply(&self, key: &str, id: &str) -> Value {
        let (status, body) = raw_apply(self.server.address, &self.token, &self.fingerprint, key, id).await;
        assert!(status.starts_with("HTTP/1.1 200"), "wire outcome is an HTTP success envelope; status={status}; body={body}");
        body
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

    async fn finish(self) {
        self.server.shutdown().await.unwrap();
        self.direct.batch_execute(&format!("DROP SCHEMA {} CASCADE", sqlgen::schema())).await.unwrap();
    }
}

const MARKED: &str = "SELECT count(*) FROM {schema}.inbox WHERE id=42 AND checked";
const IDEM_ROWS: &str = "SELECT count(*) FROM {schema}.aip_idem";

/// COMMIT이 보내지기 전에 연결이 끊기면 아무것도 커밋되지 않았다. 안전하게 다시 보낼 수 있는 오류여야 하고, 같은 키로 다시 보내면 새로 실행된다.
async fn assert_cut_before_commit_is_safe_to_retry(fault: Fault) {
    let mut fixture = Fixture::new(fault).await;
    let first = fixture.apply("cut-before-commit", "42").await;
    let phase = fixture.phase().await;
    assert_eq!(
        first["code"], "WIRE.DB_UNAVAILABLE",
        "{fault:?}: disconnect before COMMIT was sent must be a safe retry, not an unknown outcome; phase={phase}; body={first}"
    );
    assert_eq!(fixture.number(MARKED).await, 0, "{fault:?}: nothing was committed");
    assert_eq!(fixture.number(IDEM_ROWS).await, 0, "{fault:?}: no idempotency record without a commit");
    let retry = fixture.apply("cut-before-commit", "42").await;
    assert_eq!(retry["ok"], true, "{fault:?}: same key must run again after a safe-retry error: {retry}");
    assert!(retry.get("replayed").is_none(), "{fault:?}: nothing was stored, so this is a fresh execution: {retry}");
    assert_eq!(fixture.number(MARKED).await, 1);
    assert_eq!(fixture.number(IDEM_ROWS).await, 1);
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_while_beginning_the_transaction_is_a_safe_retry() {
    assert_cut_before_commit_is_safe_to_retry(Fault::CutAtBegin).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_after_update_before_commit_is_a_safe_retry() {
    assert_cut_before_commit_is_safe_to_retry(Fault::CutAfterUpdate).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_after_idempotency_insert_before_commit_is_a_safe_retry() {
    assert_cut_before_commit_is_safe_to_retry(Fault::CutAfterIdemInsert).await;
}

/// 이미 닫힌 연결에는 COMMIT이 나가지 못한다. 서버는 끊긴 연결의 열린 트랜잭션을 rollback하므로 결과가 불확실하지 않다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_on_an_already_closed_connection_is_not_commit_unknown() {
    let _gate = GATE.lock().await;
    let control = connect_owned_with_url(DATABASE).await.expect("control connection");
    let mut victim = connect_owned_with_url(DATABASE).await.expect("victim connection");
    let tx = victim.transaction().await.unwrap();
    let pid: i32 = tx.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    control.execute("SELECT pg_terminate_backend($1)", &[&pid]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !tx.client().is_closed() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client must notice that its backend was terminated");
    let commit_sent = AtomicBool::new(false);
    let outcome = spike_v6_transport::commit_attempt(tx, &commit_sent, json!({"ok": true})).await;
    assert_eq!(outcome["code"], "WIRE.DB_UNAVAILABLE", "COMMIT never left the client, so the outcome is known: {outcome}");
    assert!(!commit_sent.load(Ordering::SeqCst), "COMMIT-sent marker must stay down when COMMIT was not sent");
}

/// COMMIT을 보낸 뒤 응답을 받지 못하면, 서버에 닿았는지와 무관하게 이 연결로는 알 수 없다.
async fn assert_commit_unknown(fault: Fault, committed: bool) {
    let mut fixture = Fixture::new(fault).await;
    let first = fixture.apply("cut-after-commit", "42").await;
    let phase = fixture.phase().await;
    assert_eq!(first["code"], "WIRE.COMMIT_UNKNOWN", "{fault:?}: phase={phase}; body={first}");
    assert_eq!(fixture.number(MARKED).await, i64::from(committed), "{fault:?}: phase={phase}");
    assert_eq!(fixture.number(IDEM_ROWS).await, i64::from(committed), "{fault:?}: phase={phase}");
    let retry = fixture.apply("cut-after-commit", "42").await;
    assert_eq!(retry["ok"], true, "{fault:?}: same key must settle the unknown outcome: {retry}");
    assert_eq!(retry["replayed"] == true, committed, "{fault:?}: stored result is replayed only if the first COMMIT reached PostgreSQL: {retry}");
    assert_eq!(fixture.number(MARKED).await, 1, "{fault:?}: the change exists exactly once");
    assert_eq!(fixture.number(IDEM_ROWS).await, 1);
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_withheld_from_postgres_is_commit_unknown_and_same_key_runs_once() {
    assert_commit_unknown(Fault::CutOnCommitSent, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_reset_after_commit_ack_is_commit_unknown() {
    assert_commit_unknown(Fault::AckDropReset, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backend_terminated_while_commit_is_pending_is_commit_unknown() {
    assert_commit_unknown(Fault::TerminateAtCommit, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unparseable_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Fault::ReplaceCommitAck(b'x'), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unexpected_message_in_commit_reply_is_commit_unknown() {
    assert_commit_unknown(Fault::ReplaceCommitAck(b'n'), true).await;
}

/// 서버가 거절한 COMMIT(지연 제약 위반)은 rollback이 확정이다. COMMIT_UNKNOWN으로 넓히지 않는다.
/// 이 테스트의 proxy 장애는 COMMIT 성공 태그에만 걸리므로 두 요청 모두 장애 없이 서버 거절 경로를 탄다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn definite_commit_rejections_are_not_widened_to_commit_unknown() {
    let fixture = Fixture::new(Fault::AckDropFin).await;
    let schema = sqlgen::schema();
    fixture
        .direct
        .batch_execute(&format!(
            "CREATE FUNCTION {schema}.reject_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id=45 THEN RAISE EXCEPTION 'check fixture' USING ERRCODE='23514'; ELSIF NEW.id=46 THEN RAISE EXCEPTION 'unique fixture' USING ERRCODE='23505'; END IF; RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER reject_commit AFTER UPDATE ON {schema}.inbox DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {schema}.reject_commit()"
        ))
        .await
        .unwrap();
    let check = fixture.apply("commit-check-violation", "45").await;
    assert_eq!(check["code"], "WIRE.BAD_VALUE", "SQLSTATE 23514 at COMMIT is a definite rejection: {check}");
    let unique = fixture.apply("commit-unique-violation", "46").await;
    assert_eq!(unique["code"], "WIRE.ALREADY_EXISTS", "SQLSTATE 23505 at COMMIT is a definite rejection: {unique}");
    assert_eq!(fixture.number("SELECT count(*) FROM {schema}.inbox WHERE id IN (45,46) AND checked").await, 0);
    assert_eq!(fixture.number(IDEM_ROWS).await, 0);
    fixture.finish().await;
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

/// `Kind::Io`는 호출자의 COMMIT 호출에서는 만들 수 없다(연결 task에서 나고 호출자에게는 Closed로 전달된다).
/// 대신 같은 종류의 오류를 접속 단계에서 실제로 만들어, 판정이 Io 문구를 연결 오류로 읽는지 고정한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn io_error_during_startup_is_a_connection_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut startup = [0u8; 8];
        let _ = socket.read(&mut startup).await;
        // 닫을 때 RST를 보내려면 SO_LINGER 0이 필요하다.
        #[allow(deprecated)]
        socket.as_ref().set_linger(Some(Duration::ZERO)).unwrap();
    });
    let config = format!("host=127.0.0.1 port={} user=probe dbname=probe connect_timeout=5", address.port());
    let error = match tokio_postgres::connect(&config, tokio_postgres::NoTls).await {
        Ok(_) => panic!("a server that resets during startup must not yield a connection"),
        Err(error) => error,
    };
    server.await.unwrap();
    assert!(error.code().is_none());
    assert!(spike_v3_write::connection_failed(&error), "I/O failure without SQLSTATE is a connection failure: {error}");
}
