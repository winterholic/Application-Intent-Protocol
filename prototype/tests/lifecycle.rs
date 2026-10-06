use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const LOCAL_DB: &str = "host=localhost dbname=postgres";
const UNREACHABLE_DB: &str = "host=127.0.0.1 port=1 dbname=postgres password=never_print_me connect_timeout=1";

#[test]
fn prototype_database_configuration_requires_local_hosts_and_supported_tls() {
    let schema = SchemaGuard::new();
    for url in [
        "host=203.0.113.1 password=never_print_me",
        "host=localhost hostaddr=203.0.113.1",
        "host=localhost sslmode=require",
        "not a connection string never_print_me",
    ] {
        let output = init(&fixture(), schema.name(), url);
        assert!(!output.status.success());
        assert_eq!(result_json(&output)["code"], "DB_CONFIG");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("never_print_me"));
    }
    assert!(!schema_exists(schema.name()));
}

static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("aip-prototype-lifecycle-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).expect("create isolated test directory");
        Self(path)
    }

    fn source(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).expect("write temporary AIP definition");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct SchemaGuard(String);

impl SchemaGuard {
    fn new() -> Self {
        let name = format!("aip_p{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed));
        assert!(valid_schema(&name));
        Self(name)
    }

    fn at_limit() -> Self {
        let prefix = format!("aip_p{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed));
        let name = format!("{prefix}{}", "x".repeat(63 - prefix.len()));
        assert_eq!(name.len(), 63);
        assert!(valid_schema(&name));
        Self(name)
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

fn valid_schema(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() <= 63
        && bytes.starts_with(b"aip_")
        && bytes.get(4).is_some_and(u8::is_ascii_lowercase)
        && bytes[5..].iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
}

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("example/app.aip")
}

fn init(source: &std::path::Path, schema: &str, db_url: &str) -> Output {
    binary().arg("init").arg(source).arg("--schema").arg(schema).arg("--db-url").arg(db_url).output().expect("run prototype init command")
}

fn psql(sql: &str) -> Output {
    Command::new("psql").args(["-X", "-q", "-d", LOCAL_DB, "-Atc", sql]).output().expect("psql is required for prototype lifecycle tests")
}

fn assert_local_postgres() {
    let output = psql("SELECT 1");
    assert!(output.status.success(), "local postgres unavailable: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
}

fn result_json(output: &Output) -> Value {
    let (stream, bytes) = if output.status.success() { ("stdout", &output.stdout) } else { ("stderr", &output.stderr) };
    serde_json::from_slice(bytes).unwrap_or_else(|error| panic!("CLI {stream} must be JSON, got {:?}: {error}", String::from_utf8_lossy(bytes)))
}

fn schema_exists(schema: &str) -> bool {
    let output = psql(&format!("SELECT to_regnamespace('{schema}') IS NOT NULL"));
    assert!(output.status.success(), "schema probe failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim() == "t"
}

#[test]
fn init_creates_fresh_schema_and_reinit_preserves_existing_data() {
    assert_local_postgres();
    let schema = SchemaGuard::at_limit();
    let output = init(&fixture(), schema.name(), LOCAL_DB);
    assert!(output.status.success(), "init failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(result_json(&output)["ok"], true);
    assert!(schema_exists(schema.name()));

    let ddl = psql(&format!("SELECT (to_regclass('{}.member') IS NOT NULL AND to_regclass('{}.event') IS NOT NULL)", schema.name(), schema.name()));
    assert!(ddl.status.success(), "table probe failed: {}", String::from_utf8_lossy(&ddl.stderr));
    assert_eq!(String::from_utf8_lossy(&ddl.stdout).trim(), "t", "init applies the sample tables");

    let marker = psql(&format!("CREATE TABLE {}.marker (value integer); INSERT INTO {}.marker VALUES (42)", schema.name(), schema.name()));
    assert!(marker.status.success(), "marker setup failed: {}", String::from_utf8_lossy(&marker.stderr));
    let second = init(&fixture(), schema.name(), LOCAL_DB);
    assert!(!second.status.success(), "an existing schema must not be reset");
    assert_eq!(result_json(&second)["code"], "SCHEMA_EXISTS");
    let preserved = psql(&format!("SELECT value FROM {}.marker", schema.name()));
    assert!(preserved.status.success(), "marker read failed: {}", String::from_utf8_lossy(&preserved.stderr));
    assert_eq!(String::from_utf8_lossy(&preserved.stdout).trim(), "42", "reinit preserves existing data");
}

#[test]
fn bad_schema_names_are_rejected_before_attempting_the_database_connection() {
    let invalid_names = ["public", "pg_catalog", "aip_1bad", "aip_Bad", "aip_has-dash", "aip_foo."];
    for name in invalid_names {
        let output = init(&fixture(), name, UNREACHABLE_DB);
        assert!(!output.status.success(), "schema {name:?} must be rejected");
        assert_eq!(result_json(&output)["code"], "BAD_SCHEMA", "schema validation must precede DB access: {name:?}");
        let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(!combined.contains("never_print_me"), "connection credentials must not appear in diagnostics");
    }

    let prefix = format!("aip_p{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed));
    let overlong = format!("{prefix}{}", "x".repeat(64 - prefix.len()));
    assert_eq!(overlong.len(), 64);
    let output = init(&fixture(), &overlong, UNREACHABLE_DB);
    assert!(!output.status.success());
    assert_eq!(result_json(&output)["code"], "BAD_SCHEMA");
}

#[test]
fn database_connection_failure_is_reported_without_falling_back_to_default_database() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    let output = init(&fixture(), schema.name(), UNREACHABLE_DB);
    assert!(!output.status.success());
    assert_eq!(result_json(&output)["code"], "DB_CONNECT");
    assert!(!schema_exists(schema.name()), "a failed explicit connection must not initialize a fallback database");
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(!combined.contains("never_print_me"), "connection credentials must not appear in diagnostics");
}

#[test]
fn invalid_definition_is_rejected_without_creating_the_requested_schema() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    let temp = TempDir::new();
    let source_text = fs::read_to_string(fixture()).expect("read canonical prototype fixture");
    let invalid_text = source_text.replace("phase: Phase", "phase: MissingPhase");
    assert_ne!(invalid_text, source_text, "invalid type mutation must match the fixture");
    let source = temp.source("invalid.aip", &invalid_text);

    let output = init(&source, schema.name(), LOCAL_DB);
    assert!(!output.status.success());
    assert_eq!(result_json(&output)["code"], "INVALID_DEFINITION");
    assert!(!schema_exists(schema.name()), "definition validation must happen before schema creation");
}

#[test]
fn reserved_table_collision_rolls_back_the_entire_new_schema() {
    assert_local_postgres();
    let schema = SchemaGuard::new();
    let temp = TempDir::new();
    let source_text = fs::read_to_string(fixture()).expect("read canonical prototype fixture");
    let collision_text = format!("{source_text}\nresource AipIdem {{ fields {{ id: Id }} }}\n");
    assert_ne!(collision_text, source_text, "reserved-table collision resource must be appended");
    let source = temp.source("reserved-table-collision.aip", &collision_text);

    let output = init(&source, schema.name(), LOCAL_DB);
    assert!(!output.status.success());
    assert_eq!(result_json(&output)["code"], "DB_INIT");
    assert!(!schema_exists(schema.name()), "DDL failure must roll back CREATE SCHEMA as well as tables");
}
