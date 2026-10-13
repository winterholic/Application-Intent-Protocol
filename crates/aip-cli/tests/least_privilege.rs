//! O-22: the migration role owns the schema; the runtime role only holds the grants documented in
//! `product/DATABASE-ROLES.md`. This runs the documented grants end to end so the guide cannot drift from the code.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::FutureExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    panic::AssertUnwindSafe,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const MIGRATE_DB: &str = "host=localhost dbname=postgres";
const APP: &str = include_str!("../../../prototype/example/app.aip");

async fn service(config: &Path, runtime_db: &str, args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"));
    child.arg("service");
    if args.first() == Some(&"principal") {
        child.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        child.args(args).arg("--config").arg(config);
    }
    child.env("AIP_LP_MIGRATE", MIGRATE_DB).env("AIP_LP_RUNTIME", runtime_db).output().await.unwrap()
}
async fn ok(config: &Path, runtime_db: &str, args: &[&str]) -> Value {
    let output = service(config, runtime_db, args).await;
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}
async fn post(address: &str, path: &str, token: &str, contract: &str, body: Value) -> Value {
    let text = body.to_string();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let extra = if contract.is_empty() { String::new() } else { format!("x-aip-contract: {contract}\r\n") };
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nAuthorization: Bearer {token}\r\n{extra}Origin: https://app.example.com\r\nContent-Length: {}\r\n\r\n{text}",
                text.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut result = String::new();
    tokio::time::timeout(Duration::from_secs(8), socket.read_to_string(&mut result)).await.unwrap().unwrap();
    serde_json::from_str(result.split_once("\r\n\r\n").unwrap().1).unwrap()
}
async fn start(config: &Path, file: &Path, runtime_db: &str) -> Result<(tokio::process::Child, Value), String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"))
        .args(["service", "serve"])
        .arg(file)
        .arg("--config")
        .arg(config)
        .env("AIP_LP_MIGRATE", MIGRATE_DB)
        .env("AIP_LP_RUNTIME", runtime_db)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let count = tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line)).await.unwrap().unwrap();
    if count == 0 {
        let output = child.wait_with_output().await.unwrap();
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["event"], "listening");
    Ok((child, value))
}
async fn stop(mut child: tokio::process::Child) {
    let id = child.id().unwrap();
    assert!(std::process::Command::new("kill").args(["-TERM", &id.to_string()]).status().unwrap().success());
    assert!(tokio::time::timeout(Duration::from_secs(8), child.wait()).await.unwrap().unwrap().success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runtime_role_with_only_the_documented_grants_serves_while_the_migration_role_owns_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    let unique = uuid::Uuid::new_v4().simple().to_string();
    let schema = format!("aip_lp_{unique}");
    let role = format!("aip_lp_rt_{}", &unique[..12]);
    let runtime_db = format!("host=localhost dbname=postgres user={role}");
    let config = dir.path().join("service.json");
    let file = dir.path().join("app.aip");
    std::fs::write(&file, APP).unwrap();
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    std::fs::write(
        dir.path().join("jwks.json"),
        json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]})
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        &config,
        json!({"schema":schema,"database_url_env":"AIP_LP_RUNTIME","migrate_database_url_env":"AIP_LP_MIGRATE","listen":"127.0.0.1:0","wire":"decimal",
            "allowed_origins":["https://app.example.com"],"auth":{"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}})
        .to_string(),
    )
    .unwrap();

    let admin = spike_v2_read::connect_owned_with_url(MIGRATE_DB).await.unwrap();
    // `aip_execution::infrastructure_schema`: aip_infra_ plus the first 52 hex digits of the SHA-256 of the schema name.
    let infra: String =
        admin.query_one("SELECT 'aip_infra_' || left(encode(sha256(convert_to($1::text, 'UTF8')), 'hex'), 52)", &[&schema]).await.unwrap().get(0);
    admin.batch_execute(&format!("CREATE ROLE {role} LOGIN")).await.unwrap();
    let outcome = AssertUnwindSafe(async {
        // The migration role creates and owns everything.
        ok(&config, &runtime_db, &["init", file.to_str().unwrap()]).await;
        admin
            .batch_execute(&format!(
                "INSERT INTO {schema}.member(id) VALUES(42),(99); INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES(11,42,false,'owned','https://example.com','READY','2026-10-05Z'),(12,99,false,'other','https://example.com','READY','2026-10-05Z')"
            ))
            .await
            .unwrap();
        // The documented grants (product/DATABASE-ROLES.md). The runtime role gets no CREATE anywhere, no ownership,
        // and the audit table is insert-only.
        admin
            .batch_execute(&format!(
                "GRANT CONNECT ON DATABASE postgres TO {role}; \
                 GRANT USAGE ON SCHEMA {schema} TO {role}; \
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA {schema} TO {role}; \
                 GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO {role};"
            ))
            .await
            .unwrap();

        // Without the infrastructure objects the runtime role cannot start, and it says what to do about it.
        let failure = start(&config, &file, &runtime_db).await.err().expect("serve must refuse to start without provisioned infrastructure");
        assert!(failure.contains("aip service provision"), "the startup error must tell the operator how to fix it: {failure}");

        // Provisioning under the migration role fixes that; then the runtime grants for the infrastructure schema follow.
        let provisioned = ok(&config, &runtime_db, &["provision"]).await;
        assert_eq!(provisioned["ok"], true, "{provisioned}");
        admin
            .batch_execute(&format!(
                "GRANT USAGE ON SCHEMA {infra} TO {role}; \
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA {infra} TO {role}; \
                 REVOKE UPDATE, DELETE, TRUNCATE ON {infra}.aip_audit FROM {role}; \
                 GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA {infra} TO {role};"
            ))
            .await
            .unwrap();

        ok(&config, &runtime_db, &["principal", "bind", "--subject", "user-42", "--actor", "42"]).await;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some("current".into());
        let der = private.to_pkcs1_der().unwrap();
        let key = EncodingKey::from_rsa_der(der.as_bytes());
        let token = encode(&header, &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":now,"nbf":now,"exp":now+300}), &key).unwrap();
        let (child, listening) = start(&config, &file, &runtime_db).await.unwrap_or_else(|failure| panic!("runtime role could not start: {failure}"));
        let address = listening["address"].as_str().unwrap();
        let fingerprint = listening["fingerprint"].as_str().unwrap();
        let read = post(address, "/read", &token, fingerprint, json!({"query":{"read":"Event","select":["id","title"]}})).await;
        assert_eq!(read["rows"], json!([{"id":"11","title":"owned"}]), "{read}");
        let written = post(address, "/apply", &token, fingerprint, json!({"key":"lp-mark","request":{"apply":"Event.mark","target":{"ids":["11"]}}})).await;
        assert_eq!(written["ok"], true, "{written}");
        let audited: i64 = admin.query_one(&format!("SELECT count(*) FROM {infra}.aip_audit"), &[]).await.unwrap().get(0);
        assert_eq!(audited, 1, "the write was audited through the runtime role's insert-only grant");
        stop(child).await;

        // What the runtime role must not be able to do, checked as that role.
        let rt = spike_v2_read::connect_owned_with_url(&runtime_db).await.unwrap();
        for forbidden in [
            format!("DROP TABLE {schema}.event"),
            format!("ALTER TABLE {schema}.event ADD COLUMN sneaky int"),
            format!("CREATE TABLE {schema}.sneaky(id int)"),
            format!("CREATE SCHEMA sneaky_{unique}"),
            format!("TRUNCATE {schema}.event"),
            format!("UPDATE {infra}.aip_audit SET via='x'"),
            format!("DELETE FROM {infra}.aip_audit"),
            format!("ALTER TABLE {infra}.aip_audit DISABLE TRIGGER ALL"),
        ] {
            assert!(rt.batch_execute(&forbidden).await.is_err(), "the runtime role must not be able to run `{forbidden}`");
        }
        // And it cannot run the migration commands: it is a different role from the one that owns the schema.
        let owner: String = admin
            .query_one("SELECT pg_get_userbyid(c.relowner) FROM pg_class c WHERE c.oid=($1::text)::regclass", &[&format!("{schema}.event")])
            .await
            .unwrap()
            .get(0);
        assert_ne!(owner, role);
    })
    .catch_unwind()
    .await;
    let _ = admin
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {infra} CASCADE; DROP OWNED BY {role}; DROP ROLE {role}"
        ))
        .await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
