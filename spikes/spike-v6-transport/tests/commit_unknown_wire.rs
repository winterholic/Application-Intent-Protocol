use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_owned_with_url, id_wire::IdWire, sqlgen};
use spike_v6_transport::server::listen_with_apply_contract;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use tokio_postgres::Client;

const SOURCE: &str = include_str!("../fixture/v14.aip");
const DATABASE: &str = aip_test_support::test_database_url!();

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

fn parse_frontend_commit(pending: &mut Vec<u8>, startup: &mut bool) -> bool {
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
        if pending[offset] == b'Q' {
            let query = &pending[offset + 5..offset + 1 + len];
            if query.iter().copied().take_while(|byte| *byte != 0).collect::<Vec<_>>().eq_ignore_ascii_case(b"COMMIT") {
                offset += len + 1;
                pending.drain(..offset);
                return true;
            }
        }
        offset += len + 1;
    }
    if offset > 0 {
        pending.drain(..offset);
    }
    false
}

async fn proxy_connection(
    caller: TcpStream,
    backend_address: SocketAddr,
    drop_once: Arc<AtomicBool>,
    committed: Arc<tokio::sync::Mutex<Option<oneshot::Sender<String>>>>,
) {
    let Ok(backend) = TcpStream::connect(backend_address).await else {
        return;
    };
    let (mut caller_read, mut caller_write) = caller.into_split();
    let (mut backend_read, mut backend_write) = backend.into_split();
    let front = tokio::spawn(async move {
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
            let _ = parse_frontend_commit(&mut pending, &mut startup);
            if backend_write.write_all(&buf[..n]).await.is_err() {
                return;
            }
        }
    });
    loop {
        let mut header = [0u8; 5];
        if backend_read.read_exact(&mut header).await.is_err() {
            break;
        }
        let len = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
        if len < 4 || len > 16 * 1024 * 1024 {
            break;
        }
        let mut body = vec![0u8; len - 4];
        if backend_read.read_exact(&mut body).await.is_err() {
            break;
        }
        let is_commit_complete = header[0] == b'C' && body.strip_suffix(&[0]).is_some_and(|tag| tag == b"COMMIT");
        if is_commit_complete && drop_once.swap(false, Ordering::SeqCst) {
            if let Some(sender) = committed.lock().await.take() {
                let _ = sender.send("CommandComplete(COMMIT) observed from owned PostgreSQL; response withheld".into());
            }
            front.abort();
            let _ = caller_write.shutdown().await;
            return;
        }
        if caller_write.write_all(&header).await.is_err() || caller_write.write_all(&body).await.is_err() {
            break;
        }
    }
    front.abort();
}

async fn start_commit_proxy() -> (SocketAddr, oneshot::Receiver<String>) {
    let (backend_host, backend_port) = aip_test_support::endpoint(DATABASE).unwrap();
    let backend_address = std::net::ToSocketAddrs::to_socket_addrs(&(backend_host.as_str(), backend_port))
        .ok()
        .and_then(|mut addresses| addresses.next())
        .expect("resolve the test database endpoint");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let drop_once = Arc::new(AtomicBool::new(true));
    let (sender, receiver) = oneshot::channel();
    let committed = Arc::new(tokio::sync::Mutex::new(Some(sender)));
    tokio::spawn(async move {
        loop {
            let Ok((caller, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(proxy_connection(caller, backend_address, drop_once.clone(), committed.clone()));
        }
    });
    (address, receiver)
}

async fn seed(db: &Client, schema: &str, facts: &Value) {
    for statement in sqlgen::ddl(facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(db).await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.inbox(id,member_id,checked,count,title) VALUES (42,1,false,1,'commit unknown'),(43,1,false,1,'serialization error'),(44,1,false,1,'deadlock error')")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_reports_commit_unknown_when_postgres_ack_is_cut_after_commit_and_same_key_replays() {
    let unique = format!(
        "aip_commit_unknown_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    );
    sqlgen::set_schema(&unique);
    let schema = sqlgen::schema();
    let mut facts = load_str(SOURCE, Form::A).unwrap().execution;
    facts["resources"]["Inbox"]["transitions"]["mark"]["isolation"] = json!("serializable");
    let direct = connect_owned_with_url(DATABASE).await.expect("connect to run-owned PostgreSQL only");
    seed(&direct, schema, &facts).await;

    let (proxy_address, commit_observed) = start_commit_proxy().await;
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
    assert_eq!(
        direct.query_one(&format!("SELECT checked FROM {schema}.inbox WHERE id=43"), &[]).await.unwrap().get::<_, bool>(0),
        false,
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
    assert_eq!(direct.query_one(&format!("SELECT checked FROM {schema}.inbox WHERE id=44"), &[]).await.unwrap().get::<_, bool>(0), false);
    assert_eq!(direct.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get::<_, i64>(0), 1);
    assert_eq!(
        direct.query_one(&format!("SELECT last_value FROM {schema}.commit_attempts"), &[]).await.unwrap().get::<_, i64>(0),
        8,
        "40P01 is retried exactly three times after the initial attempt"
    );
    server.shutdown().await.unwrap();
    direct.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
