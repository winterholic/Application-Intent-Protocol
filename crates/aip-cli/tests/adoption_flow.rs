use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::FutureExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::{ServerOptions, listen_with_prototype_options};
use std::{
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const DB: &str = "host=localhost dbname=postgres";

async fn product_output(config: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aip"));
    command.arg("service");
    if args.first() == Some(&"principal") {
        command.arg("principal").arg("--config").arg(config).args(&args[1..]);
    } else {
        command.args(args).arg("--config").arg(config);
    }
    command.env("AIP_TEST_DATABASE", DB).output().await.expect("run product CLI")
}

async fn product(config: &Path, args: &[&str]) -> Value {
    let output = product_output(config, args).await;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("product CLI JSON")
}

async fn product_error(config: &Path, args: &[&str]) -> Value {
    let output = product_output(config, args).await;
    assert!(!output.status.success(), "product CLI unexpectedly succeeded: {}", String::from_utf8_lossy(&output.stdout));
    serde_json::from_slice(&output.stderr).expect("product CLI error JSON")
}

async fn post(address: &str, path: &str, token: &str, fingerprint: &str, origin: Option<&str>, body: Value) -> Value {
    let text = body.to_string();
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect to server");
    let contract_header = if fingerprint.is_empty() { String::new() } else { format!("x-aip-contract: {fingerprint}\r\n") };
    let origin_header = origin.map(|value| format!("Origin: {value}\r\n")).unwrap_or_default();
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nAuthorization: Bearer {token}\r\n{contract_header}{origin_header}Content-Length: {}\r\n\r\n{text}",
                text.len(),
            )
            .as_bytes(),
        )
        .await
        .expect("write HTTP request");
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(8), socket.read_to_string(&mut response)).await.expect("response deadline").expect("read response");
    serde_json::from_str(response.split_once("\r\n\r\n").expect("HTTP response body").1).expect("JSON response")
}

async fn start_product(config: &Path, file: &Path) -> (Child, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"))
        .args(["service", "serve"])
        .arg(file)
        .arg("--config")
        .arg(config)
        .env("AIP_TEST_DATABASE", DB)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start product CLI");
    let mut line = String::new();
    let mut reader = BufReader::new(child.stdout.take().expect("product stdout"));
    let count = tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("product startup deadline")
        .expect("read product startup");
    if count == 0 {
        let output = child.wait_with_output().await.expect("product startup output");
        panic!("product serve failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let event: Value = serde_json::from_str(&line).expect("product listening event");
    assert_eq!(event["event"], "listening");
    assert!(event.get("devToken").is_none());
    (child, event)
}

async fn stop_product(mut child: Child) {
    let pid = child.id().expect("product PID");
    let status = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status().expect("send TERM");
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("product shutdown deadline").expect("product process").success()
    );
}

async fn expect_wire_rejection(config: &Path, file: &Path) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aip"))
        .args(["service", "serve"])
        .arg(file)
        .arg("--config")
        .arg(config)
        .env("AIP_TEST_DATABASE", DB)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start mismatched product CLI");
    let mut line = String::new();
    let mut reader = BufReader::new(child.stdout.take().expect("product stdout"));
    let count = tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("mismatched preflight deadline")
        .expect("read startup");
    if count > 0 {
        panic!("serve started with a mismatched wire mode: {line}");
    }
    drop(reader);
    let output = child.wait_with_output().await.expect("mismatched serve output");
    assert!(!output.status.success(), "wire mismatch must refuse product startup");
    let error: Value = serde_json::from_slice(&output.stderr).expect("wire mismatch JSON error");
    assert_eq!(error["code"], "WIRE_MODE_MISMATCH", "{error}");
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_secs()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adoption_preserves_legacy_decimal_idempotency_and_fences_wire_mode() {
    let directory = tempfile::tempdir().expect("temporary owned files");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let file = root.join("../../prototype/example/app.aip");
    let schema = format!("aip_adopt_{}", uuid::Uuid::new_v4().simple());
    let config = directory.path().join("service.json");
    let wrong_wire_config = directory.path().join("service-safe.json");
    let jwks = directory.path().join("jwks.json");
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("ephemeral test signing key");
    let public = private.to_public_key();
    std::fs::write(
        &jwks,
        json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]}).to_string(),
    )
    .expect("write ephemeral public JWKS");
    let config_value = json!({
        "schema": schema,
        "database_url_env": "AIP_TEST_DATABASE",
        "listen": "127.0.0.1:0",
        "wire": "decimal",
        "allowed_origins": ["https://app.example.com"],
        "auth": {"issuer":"https://issuer.example","audience":"aip-api","jwks":{"kind":"file","path":"jwks.json"}}
    });
    std::fs::write(&config, config_value.to_string()).expect("write product config");
    let mut safe_config = config_value;
    safe_config["wire"] = json!("safe");
    std::fs::write(&wrong_wire_config, safe_config.to_string()).expect("write mismatched wire config");

    let facts = aip_service::load(&file, false).expect("load prototype definition").output.execution;
    spike_v2_read::sqlgen::set_schema(&schema);
    let mut prototype_ddl = spike_v2_read::sqlgen::create_ddl(&facts).expect("prototype schema DDL");
    prototype_ddl.push(spike_v6_transport::idempotency_ddl());
    let prototype_ddl_digest = spike_v1_fixture::digest(&json!(prototype_ddl));
    let mut database = spike_v2_read::connect_owned_with_url(DB).await.expect("local PostgreSQL");
    let mut legacy_server = None;
    let mut schema_created = false;
    let operation = AssertUnwindSafe(async {
        let tx = database.transaction().await.expect("prototype DDL transaction");
        for statement in &prototype_ddl {
            tx.batch_execute(statement).await.expect("apply prototype DDL");
            if statement.starts_with("CREATE SCHEMA ") {
                schema_created = true;
            }
        }
        tx.batch_execute(&format!("CREATE TABLE {schema}.aip_proto_meta (singleton boolean PRIMARY KEY CHECK(singleton), ddl_digest text NOT NULL, structure_digest text NOT NULL)"))
            .await
            .expect("create real prototype marker");
        tx.batch_execute("SET LOCAL search_path TO pg_catalog").await.expect("catalog search path");
        let catalog: String = tx.query_one(include_str!("../../../prototype/src/schema_catalog.sql"), &[&schema]).await.expect("prototype catalog").get(0);
        let structure: Value = serde_json::from_str(&catalog).expect("prototype catalog JSON");
        let structure_digest = spike_v1_fixture::digest(&json!({"format":"prototype-catalog-v1","catalog":structure}));
        tx.execute(&format!("INSERT INTO {schema}.aip_proto_meta VALUES (true,$1,$2)"), &[&prototype_ddl_digest, &structure_digest])
            .await
            .expect("write verified prototype marker");
        tx.batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (42),(99); INSERT INTO {schema}.event(id,member_id,checked,title,link,phase,date) VALUES (11,42,false,'owned','https://example.com','READY','2026-10-05Z'),(12,99,false,'other','https://example.com','READY','2026-10-05Z')"
        ))
        .await
        .expect("seed actors and owned application rows");
        tx.commit().await.expect("commit prototype catalog");

        let server = listen_with_prototype_options(
            facts.clone(),
            IdWire::DecimalString,
            "127.0.0.1:0".parse().unwrap(),
            DB.to_string(),
            None,
            ServerOptions::default(),
            false,
        )
        .await
        .expect("start real legacy V6 prototype transport");
        let legacy_address = server.address.to_string();
        let binding_path = directory.path().join("contract.ts");
        let generated = Command::new(env!("CARGO_BIN_EXE_aip"))
            .args(["service", "gen"])
            .arg(&file)
            .args(["--wire", "decimal", "--out"])
            .arg(&binding_path)
            .output()
            .await
            .expect("generate product contract fingerprint");
        assert!(generated.status.success(), "{}", String::from_utf8_lossy(&generated.stderr));
        let generated: Value = serde_json::from_slice(&generated.stdout).expect("contract generation JSON");
        let legacy_fingerprint = generated["fingerprint"].as_str().unwrap();
        let legacy_token = server.keys.issue(42, 120);
        legacy_server = Some(server);
        let apply = json!({"key":"product-mark","request":{"apply":"Event.mark","target":{"ids":["11"]}}});
        let original = post(&legacy_address, "/apply", &legacy_token, &legacy_fingerprint, None, apply.clone()).await;
        assert_eq!(original["ok"], true, "{original}");
        assert_eq!(original["changed"], json!(["11"]), "the legacy server must commit the actual decimal-ID write");
        legacy_server.take().unwrap().shutdown().await.expect("stop legacy server before adoption");

        let wrong_wire_plan = product_error(&wrong_wire_config, &["adopt", file.to_str().unwrap()]).await;
        assert_eq!(wrong_wire_plan["code"], "WIRE_MODE_MISMATCH", "{wrong_wire_plan}");
        let legacy_marker = format!("{schema}.aip_proto_meta");
        let product_meta = format!("{schema}.aip_migrate_meta");
        assert!(database.query_one("SELECT to_regclass($1) IS NOT NULL", &[&legacy_marker]).await.unwrap().get::<_, bool>(0), "failed wire review must leave the prototype marker intact");
        assert!(!database.query_one("SELECT to_regclass($1) IS NOT NULL", &[&product_meta]).await.unwrap().get::<_, bool>(0), "failed wire review must not create product metadata");

        let plan = product(&config, &["adopt", file.to_str().unwrap()]).await;
        assert_eq!(plan["ok"], true, "{plan}");
        assert_eq!(plan["blocked"], false, "{plan}");
        let acknowledgement = plan["digest"].as_str().expect("adoption plan digest");
        let adopted = product(&config, &["adopt", file.to_str().unwrap(), "--apply", acknowledgement]).await;
        assert_eq!(adopted["adopted"], true, "{adopted}");
        product(&config, &["principal", "bind", "--subject", "user-42", "--actor", "42"]).await;

        let key = private.to_pkcs1_der().expect("encode ephemeral private key");
        let encoding_key = EncodingKey::from_rsa_der(key.as_bytes());
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("at+jwt".into());
        header.kid = Some("current".into());
        let issued_at = now();
        let token = encode(
            &header,
            &json!({"iss":"https://issuer.example","aud":"aip-api","sub":"user-42","iat":issued_at,"nbf":issued_at,"exp":issued_at+300}),
            &encoding_key,
        )
        .expect("sign product test JWT");

        let (child, listening) = start_product(&config, &file).await;
        let address = listening["address"].as_str().unwrap();
        let fingerprint = listening["fingerprint"].as_str().unwrap();
        assert_eq!(listening["idWire"], "decimal-string-v13");
        assert_eq!(post(address, "/session", &token, "", Some("https://app.example.com"), json!({})).await["principal"]["actorId"], "42");
        let mut replayed = post(address, "/apply", &token, fingerprint, Some("https://app.example.com"), apply.clone()).await;
        assert_eq!(replayed.as_object_mut().unwrap().remove("replayed"), Some(json!(true)), "adoption must retain the legacy idempotency row: {replayed}");
        assert_eq!(replayed, original, "the committed legacy result must replay byte-for-byte as JSON");
        let new_key = json!({"key":"product-mark-new","request":{"apply":"Event.mark","target":{"ids":["11"]}}});
        let already_applied = post(address, "/apply", &token, fingerprint, Some("https://app.example.com"), new_key).await;
        assert_eq!(already_applied["changed"], json!([]), "a fresh key would expose a lost legacy replay row");
        assert_eq!(already_applied["unchanged"], json!(["11"]));
        stop_product(child).await;

        expect_wire_rejection(&wrong_wire_config, &file).await;
    })
    .catch_unwind()
    .await;
    if let Some(server) = legacy_server.take() {
        server.shutdown().await.expect("stop legacy server during cleanup");
    }
    if schema_created {
        database.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.expect("drop owned UUID schema");
    }
    if let Err(panic) = operation {
        std::panic::resume_unwind(panic);
    }
}
