use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const LOCAL_DB: &str = "host=localhost dbname=postgres";
const UNREACHABLE_DB: &str = "host=127.0.0.1 port=1 dbname=postgres password=never_print_me connect_timeout=1";

static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);

struct SchemaGuard(String);

impl SchemaGuard {
    fn new() -> Self {
        Self(format!("aip_serve_p{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed)))
    }

    fn name(&self) -> &str {
        &self.0
    }
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        let _ = psql(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.0));
    }
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
}

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("example/app.aip")
}

fn psql(sql: &str) -> Output {
    Command::new("psql").args(["-X", "-q", "-d", LOCAL_DB, "-Atc", sql]).output().expect("psql is required for serve lifecycle tests")
}

fn assert_local_postgres() {
    let output = psql("SELECT 1");
    assert!(output.status.success(), "local postgres unavailable: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
}

fn schema_exists(schema: &str) -> bool {
    let output = psql(&format!("SELECT to_regnamespace('{schema}') IS NOT NULL"));
    assert!(output.status.success(), "schema probe failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim() == "t"
}

fn init(schema: &str) -> Output {
    binary().arg("init").arg(fixture()).arg("--schema").arg(schema).arg("--db-url").arg(LOCAL_DB).output().expect("run prototype init")
}

#[test]
#[cfg(unix)]
fn a_closed_startup_output_stops_the_server_without_a_panic_or_schema_reset() {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let name =
        format!("aip_report_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let initialized = init(&name);
    assert!(initialized.status.success(), "fresh schema owned by this test");
    let schema = SchemaGuard(name);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let fd: OwnedFd = writer.into();
    let output = serve_command(schema.name(), LOCAL_DB, &address.to_string()).stdout(Stdio::from(fd)).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(result_json(&output)["code"], "OUTPUT_IO");
    assert!(TcpStream::connect(address).is_err());
    assert!(schema_exists(schema.name()));
}

fn serve_command(schema: &str, db_url: &str, listen: &str) -> Command {
    serve_command_with_wire(schema, db_url, listen, "decimal")
}

fn serve_command_with_wire(schema: &str, db_url: &str, listen: &str, wire: &str) -> Command {
    let mut command = binary();
    command.arg("serve").arg(fixture()).arg("--schema").arg(schema).arg("--db-url").arg(db_url).arg("--listen").arg(listen).arg("--wire").arg(wire);
    command
}

fn result_json(output: &Output) -> Value {
    let bytes = if output.status.success() { &output.stdout } else { &output.stderr };
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "CLI result must be JSON, got stdout={:?}, stderr={:?}: {error}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn run_bounded(mut command: Command, timeout: Duration) -> Output {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("spawn bounded CLI process");
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll CLI process").is_some() {
            return child.wait_with_output().expect("collect CLI process output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI process exceeded {timeout:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

struct ServerChild {
    child: Child,
    events: Receiver<String>,
    stderr: Receiver<String>,
}

impl ServerChild {
    fn spawn(mut command: Command) -> Self {
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("spawn prototype server");
        let stdout = child.stdout.take().expect("piped server stdout");
        let stderr = child.stderr.take().expect("piped server stderr");
        let (event_tx, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if event_tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let (stderr_tx, stderr_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            let _ = stderr_tx.send(text);
        });
        Self { child, events, stderr: stderr_rx }
    }

    fn event(&self, timeout: Duration) -> Value {
        let line = self.events.recv_timeout(timeout).expect("timed out waiting for server event");
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("server stdout event must be JSON ({line:?}): {error}"))
    }

    fn terminate(&mut self) -> (Value, ExitStatus, String) {
        let pid = self.child.id().to_string();
        let signal = Command::new("kill").args(["-TERM", &pid]).status().expect("send SIGTERM to server");
        assert!(signal.success(), "SIGTERM should be delivered");
        let stopped = self.event(Duration::from_secs(5));
        assert_eq!(stopped["event"], "stopped");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll server exit") {
                break status;
            }
            assert!(Instant::now() < deadline, "server did not exit after SIGTERM");
            thread::sleep(Duration::from_millis(20));
        };
        let stderr = self.stderr.recv_timeout(Duration::from_secs(2)).expect("server stderr reader should finish");
        (stopped, status, stderr)
    }
}

impl Drop for ServerChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn post(address: &str, path: &str, token: Option<&str>, body: &Value) -> Value {
    let addr: SocketAddr = address.parse().expect("listening address must be a socket address");
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect to prototype server");
    stream.set_read_timeout(Some(Duration::from_secs(3))).expect("set HTTP read timeout");
    let body = body.to_string();
    let authorization = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    write!(stream, "POST {path} HTTP/1.1\r\nhost: {addr}\r\n{authorization}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
        .expect("write HTTP request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read HTTP response");
    let (headers, payload) = response.split_once("\r\n\r\n").expect("HTTP response separates headers and JSON body");
    assert!(headers.starts_with("HTTP/1.1 200"), "transport returns protocol result in JSON: {headers}");
    serde_json::from_str(payload).unwrap_or_else(|error| panic!("HTTP response body must be JSON ({payload:?}): {error}"))
}

#[test]
fn serve_rejects_unready_schema_bad_db_and_non_loopback_before_listening() {
    assert_local_postgres();
    let schema = SchemaGuard::new();

    let not_ready = run_bounded(serve_command(schema.name(), LOCAL_DB, "127.0.0.1:0"), Duration::from_secs(5));
    assert!(!not_ready.status.success());
    assert_eq!(result_json(&not_ready)["code"], "SCHEMA_NOT_READY");
    assert!(!schema_exists(schema.name()));

    let bad_db = run_bounded(serve_command(schema.name(), UNREACHABLE_DB, "127.0.0.1:0"), Duration::from_secs(5));
    assert!(!bad_db.status.success());
    assert_eq!(result_json(&bad_db)["code"], "DB_CONNECT");
    let bad_db_text = format!("{}{}", String::from_utf8_lossy(&bad_db.stdout), String::from_utf8_lossy(&bad_db.stderr));
    assert!(!bad_db_text.contains("never_print_me"), "DB diagnostics must not reveal URL credentials");

    let bad_listen = run_bounded(serve_command(schema.name(), UNREACHABLE_DB, "0.0.0.0:0"), Duration::from_secs(5));
    assert!(!bad_listen.status.success());
    assert_eq!(result_json(&bad_listen)["code"], "BAD_LISTEN", "listen address is validated before DB access");
}

#[test]
fn serve_runs_the_token_read_and_shutdown_lifecycle_on_the_requested_socket() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    let initialized = init(schema.name());
    assert!(initialized.status.success(), "init failed: {}", String::from_utf8_lossy(&initialized.stderr));
    let seed = psql(&format!(
        "INSERT INTO {}.member (id) VALUES (1); INSERT INTO {}.event (id, member_id, checked, title, link, phase, date) VALUES (1, 1, false, 'hello', 'https://example.test', 'READY', '2026-10-05T00:00:00Z')",
        schema.name(),
        schema.name()
    ));
    assert!(seed.status.success(), "test seed failed: {}", String::from_utf8_lossy(&seed.stderr));

    let mut command = serve_command(schema.name(), LOCAL_DB, "127.0.0.1:0");
    command.arg("--dev-actor").arg("1").arg("--dev-token-ttl").arg("60");
    let mut server = ServerChild::spawn(command);
    let listening = server.event(Duration::from_secs(5));
    assert_eq!(listening["ok"], true);
    assert_eq!(listening["event"], "listening");
    assert_eq!(listening["idWire"], "decimal-string-v13");
    let address = listening["address"].as_str().expect("listening event includes bound address");
    let fingerprint = listening["fingerprint"].as_str().expect("listening event includes contract fingerprint");
    assert_eq!(fingerprint.len(), 64);
    let token = listening["devToken"].as_str().expect("explicit dev actor issues a signed token");
    let session = post(address, "/session", Some(token), &json!({}));
    assert_eq!(session["ok"], true);
    assert_eq!(session["principal"]["actorId"], "1");
    let rows = post(address, "/read", Some(token), &json!({ "query": { "read": "Event", "select": ["id", "title"] } }));
    assert_eq!(rows["ok"], true);
    assert_eq!(rows["rows"], json!([{ "id": "1", "title": "hello" }]));

    let competing = run_bounded(serve_command(schema.name(), LOCAL_DB, address), Duration::from_secs(5));
    assert!(!competing.status.success());
    assert_eq!(result_json(&competing)["code"], "LISTEN_FAILED");

    let (stopped, status, stderr) = server.terminate();
    assert_eq!(stopped["ok"], true);
    assert!(status.success(), "SIGTERM shutdown exits cleanly: {status}");
    assert!(!stderr.contains(token), "the dev token is emitted only in the explicit startup JSON");
    let addr: SocketAddr = address.parse().unwrap();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err(), "the port closes after shutdown");
}

#[test]
fn dev_actor_and_wire_options_are_validated_before_server_start() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    let initialized = init(schema.name());
    assert!(initialized.status.success(), "init failed: {}", String::from_utf8_lossy(&initialized.stderr));

    let mut missing_actor = serve_command(schema.name(), LOCAL_DB, "127.0.0.1:0");
    missing_actor.arg("--dev-actor").arg("999");
    let missing_actor = run_bounded(missing_actor, Duration::from_secs(5));
    assert!(!missing_actor.status.success());
    assert_eq!(result_json(&missing_actor)["code"], "BAD_DEV_SESSION");

    for ttl in ["0", "301"] {
        let mut invalid_ttl = serve_command(schema.name(), UNREACHABLE_DB, "127.0.0.1:0");
        invalid_ttl.arg("--dev-actor").arg("1").arg("--dev-token-ttl").arg(ttl);
        let invalid_ttl = run_bounded(invalid_ttl, Duration::from_secs(5));
        assert!(!invalid_ttl.status.success());
        assert_eq!(result_json(&invalid_ttl)["code"], "BAD_DEV_SESSION", "TTL must be checked before DB access");
    }

    let mut missing_wire = binary();
    missing_wire.arg("serve").arg(fixture()).arg("--schema").arg(schema.name()).arg("--db-url").arg(LOCAL_DB).arg("--listen").arg("127.0.0.1:0");
    assert!(!run_bounded(missing_wire, Duration::from_secs(5)).status.success());

    let unknown_wire = serve_command_with_wire(schema.name(), LOCAL_DB, "127.0.0.1:0", "mystery");
    assert!(!run_bounded(unknown_wire, Duration::from_secs(5)).status.success());

    let mut unknown_option = serve_command(schema.name(), LOCAL_DB, "127.0.0.1:0");
    unknown_option.arg("--not-a-real-option");
    assert!(!run_bounded(unknown_option, Duration::from_secs(5)).status.success());
}

#[test]
fn explicit_token_ttl_requires_an_actor_instead_of_being_ignored() {
    let schema = SchemaGuard::new();
    let mut command = serve_command(schema.name(), UNREACHABLE_DB, "127.0.0.1:0");
    command.args(["--dev-token-ttl", "60"]);
    let output = run_bounded(command, Duration::from_secs(5));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--dev-actor"), "unused token configuration must be rejected by the CLI");
}

#[test]
fn schema_changes_and_missing_tables_are_rejected_without_ddl_or_data_reset() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    assert!(init(schema.name()).status.success());
    assert!(psql(&format!("INSERT INTO {}.member(id) VALUES(42)", schema.name())).status.success());
    let dir = std::env::temp_dir().join(format!("aip-serve-schema-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let source = std::fs::read_to_string(fixture()).unwrap();
    let modified = source.replace("title: Text;", "title: Text; extra: Text;");
    assert_ne!(modified, source);
    let file = dir.join("changed.aip");
    std::fs::write(&file, modified).unwrap();
    let mut changed = binary();
    changed.arg("serve").arg(&file).args(["--schema", schema.name(), "--db-url", LOCAL_DB, "--listen", "127.0.0.1:0", "--wire", "decimal"]);
    let output = run_bounded(changed, Duration::from_secs(5));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(result_json(&output)["code"], "SCHEMA_MISMATCH");
    assert!(psql(&format!("DROP TABLE {}.aip_idem", schema.name())).status.success());
    let missing = run_bounded(serve_command(schema.name(), LOCAL_DB, "127.0.0.1:0"), Duration::from_secs(5));
    assert_eq!(result_json(&missing)["code"], "SCHEMA_NOT_READY");
    assert_eq!(String::from_utf8_lossy(&psql(&format!("SELECT id FROM {}.member", schema.name())).stdout).trim(), "42");
    assert_eq!(
        String::from_utf8_lossy(&psql(&format!("SELECT to_regclass('{}.aip_idem') IS NULL", schema.name())).stdout).trim(),
        "t",
        "serve never recreates a missing table"
    );
}

#[test]
fn same_named_objects_with_changed_structure_are_rejected_before_binding() {
    assert_local_postgres();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = occupied.local_addr().unwrap().to_string();
    let mutations = [
        "ALTER TABLE {s}.event ALTER COLUMN checked TYPE text USING checked::text",
        "ALTER TABLE {s}.event ALTER COLUMN checked DROP NOT NULL",
        "ALTER TABLE {s}.event ALTER COLUMN checked SET DEFAULT true",
        "ALTER TABLE {s}.event ALTER COLUMN id DROP IDENTITY",
        "ALTER TABLE {s}.event DROP CONSTRAINT event_phase_check",
        "ALTER TABLE {s}.event DROP CONSTRAINT event_member_id_fkey",
        "ALTER TABLE {s}.aip_idem DROP CONSTRAINT aip_idem_pkey",
        "ALTER TABLE {s}.aip_idem DROP CONSTRAINT aip_idem_pkey; ALTER TABLE {s}.aip_idem ADD PRIMARY KEY (principal)",
        "CREATE UNIQUE INDEX unexpected_partial ON {s}.event (member_id) WHERE checked=true",
        "ALTER SEQUENCE {s}.event_id_seq INCREMENT BY 2",
        "ALTER TABLE {s}.event ENABLE ROW LEVEL SECURITY",
        "DROP TABLE {s}.aip_idem; CREATE TABLE {s}.aip_idem (wrong integer)",
        "DROP TABLE {s}.aip_idem; CREATE VIEW {s}.aip_idem AS SELECT 1 AS wrong",
    ];
    let mut failures = vec![];
    for mutation in mutations {
        let schema = SchemaGuard::new();
        assert!(init(schema.name()).status.success());
        let setup = format!("INSERT INTO {}.member(id) VALUES(42); {}", schema.name(), mutation.replace("{s}", schema.name()));
        let altered = psql(&setup);
        assert!(altered.status.success(), "mutation failed: {mutation}: {}", String::from_utf8_lossy(&altered.stderr));
        let output = run_bounded(serve_command(schema.name(), LOCAL_DB, &address), Duration::from_secs(5));
        let result = result_json(&output);
        if result["code"] != "SCHEMA_MISMATCH" {
            failures.push(format!("{mutation}: {result}"));
        }
        let retained = psql(&format!("SELECT id FROM {}.member", schema.name()));
        assert_eq!(String::from_utf8_lossy(&retained.stdout).trim(), "42", "serve must preserve rows after drift");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn rows_sequence_position_and_equivalent_index_recreation_do_not_change_structure() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    assert!(init(schema.name()).status.success());
    let setup = psql(&format!("INSERT INTO {s}.member(id) VALUES(42); SELECT setval('{s}.event_id_seq',900); ALTER TABLE {s}.event DROP CONSTRAINT event_pkey; ALTER TABLE {s}.event ADD PRIMARY KEY (id)",s=schema.name()));
    assert!(setup.status.success(), "{}", String::from_utf8_lossy(&setup.stderr));
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = run_bounded(serve_command(schema.name(), LOCAL_DB, &occupied.local_addr().unwrap().to_string()), Duration::from_secs(5));
    assert_eq!(result_json(&output)["code"], "LISTEN_FAILED", "normal state changes and equivalent physical objects pass schema validation");
    assert_eq!(String::from_utf8_lossy(&psql(&format!("SELECT id FROM {}.member", schema.name())).stdout).trim(), "42");
}

#[test]
fn legacy_marker_schema_is_preserved_and_rejected_without_an_automatic_migration() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    assert!(init(schema.name()).status.success());
    let setup = psql(&format!(
        "INSERT INTO {s}.member(id) VALUES(42); ALTER TABLE {s}.aip_proto_meta DROP COLUMN IF EXISTS structure_digest",
        s = schema.name()
    ));
    assert!(setup.status.success());
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = run_bounded(serve_command(schema.name(), LOCAL_DB, &occupied.local_addr().unwrap().to_string()), Duration::from_secs(5));
    assert_eq!(result_json(&output)["code"], "SCHEMA_NOT_READY");
    assert_eq!(String::from_utf8_lossy(&psql(&format!("SELECT id FROM {}.member", schema.name())).stdout).trim(), "42");
    assert_eq!(String::from_utf8_lossy(&psql(&format!("SELECT count(*) FROM information_schema.columns WHERE table_schema='{}' AND table_name='aip_proto_meta' AND column_name='structure_digest'",schema.name())).stdout).trim(),"0");
}

#[test]
fn inheritance_from_outside_the_managed_schema_is_rejected_before_binding() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    assert!(init(schema.name()).status.success());
    let external = SchemaGuard::new();
    let setup = psql(&format!("CREATE SCHEMA {e}; INSERT INTO {s}.member(id) VALUES(42); CREATE TABLE {e}.inherited () INHERITS ({s}.event); INSERT INTO {e}.inherited(id,member_id,checked,title,link,phase,date) VALUES(11,42,false,'child','https://example.test','READY','2026-10-05Z')",s=schema.name(),e=external.name()));
    assert!(setup.status.success(), "{}", String::from_utf8_lossy(&setup.stderr));
    assert_eq!(
        String::from_utf8_lossy(&psql(&format!("SELECT count(*) FROM {}.event", schema.name())).stdout).trim(),
        "1",
        "a managed table read includes the external child row"
    );
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = run_bounded(serve_command(schema.name(), LOCAL_DB, &occupied.local_addr().unwrap().to_string()), Duration::from_secs(5));
    assert_eq!(result_json(&output)["code"], "SCHEMA_MISMATCH");
    assert_eq!(String::from_utf8_lossy(&psql(&format!("SELECT count(*) FROM {}.inherited", external.name())).stdout).trim(), "1");
}
