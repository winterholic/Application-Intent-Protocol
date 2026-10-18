use aip_auth::{AuthenticatorImpl, Config, JwksSource};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rand::thread_rng;
use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    infrastructure::Infrastructure,
    otlp::OtlpConfig,
    server::{listen_with_infrastructure, ServerOptions},
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

const ISSUER: &str = "https://issuer.example";
const SUBJECT: &str = "wbs83-api-fixture";

fn jwt_fixture() -> (tempfile::TempDir, Config, EncodingKey) {
    // Mirrors the existing aip-auth JWT integration-test issuer helper and API access-token profile.
    let dir = tempfile::tempdir().expect("JWT fixture tempdir");
    let private = RsaPrivateKey::new(&mut thread_rng(), 2048).expect("JWT fixture signing key");
    let public = private.to_public_key();
    let jwks = json!({"keys":[{"kty":"RSA","kid":"current","alg":"RS256","use":"sig",
        "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]});
    let path = dir.path().join("jwks.json");
    std::fs::write(&path, jwks.to_string()).expect("write fixture JWKS");
    let der = private.to_pkcs1_der().expect("fixture private key DER");
    (
        dir,
        Config { issuer: ISSUER.into(), audience: "aip-api".into(), jwks: JwksSource::File { path } },
        EncodingKey::from_rsa_der(der.as_bytes()),
    )
}

fn issue_jwt(key: &EncodingKey) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs();
    let claims = json!({"iss":ISSUER,"aud":"aip-api","sub":SUBJECT,"iat":now,"nbf":now,"exp":now+300});
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("at+jwt".into());
    header.kid = Some("current".into());
    encode(&header, &claims, key).expect("issue fixture access token")
}

async fn collector(delay: Duration) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind owned loopback collector");
    let addr = listener.local_addr().unwrap();
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let seen = payloads.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let payloads = seen.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buf = [0_u8; 4096];
                let (header_end, content_length) = loop {
                    let n = match socket.read(&mut buf).await { Ok(n) if n > 0 => n, _ => return };
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let end = end + 4;
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let length = head.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                        }).unwrap_or(0);
                        break (end, length);
                    }
                };
                while bytes.len().saturating_sub(header_end) < content_length {
                    let n = match socket.read(&mut buf).await { Ok(n) if n > 0 => n, _ => return };
                    bytes.extend_from_slice(&buf[..n]);
                }
                let body = String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).to_string();
                payloads.lock().await.push(body);
                tokio::time::sleep(delay).await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}").await;
            });
        }
    });
    (addr, payloads)
}

async fn http(address: SocketAddr, path: &str, token: &str, contract: &str, trace_id: &str, body: Value) -> (u16, Value, Duration) {
    let started = Instant::now();
    let mut socket = TcpStream::connect(address).await.expect("connect API loopback");
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {contract}\r\ntraceparent: 00-{trace_id}-aabbccddeeff0011-01\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(request.as_bytes()).await.expect("write API request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut response)).await.expect("test harness 10s hang guard").expect("read API response");
    let text = String::from_utf8(response).expect("HTTP response UTF-8");
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response separator");
    let status = head.split_whitespace().nth(1).expect("HTTP status").parse().expect("numeric HTTP status");
    (status, serde_json::from_str(body).expect("JSON response"), started.elapsed())
}

fn validate_dsn(dsn: &str) {
    let fields: HashMap<_, _> = dsn.split_whitespace().filter_map(|part| part.split_once('=')).collect();
    assert_eq!(fields.get("host"), Some(&"127.0.0.1"), "must use explicit loopback DSN");
    assert_eq!(fields.get("user"), Some(&"runner"), "must use this run's DB identity");
    assert_eq!(fields.get("dbname"), Some(&"aip_test_wbs83_api"), "must use this run's database");
    let port: u16 = fields.get("port").expect("explicit PostgreSQL port").parse().expect("numeric port");
    assert_ne!(port, 5432, "refuse default/shared port");
    assert_ne!(port, 25432, "refuse shared test port");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_read_and_apply_remain_real_db_operations_across_collector_states() {
    let dsn = std::env::var("AIP_TEST_DATABASE").expect("explicit run-owned PostgreSQL DSN required");
    validate_dsn(&dsn);
    let facts = load_str(include_str!("../../../spikes/spike-v6-transport/tests/fixtures/job-data.aip"), Form::A)
        .expect("existing product fixture parses").execution;
    let cases = ["normal", "connection_refused", "delay_2s"];
    for (index, scenario) in cases.iter().enumerate() {
        let unique = format!("{}_{}_{}", std::process::id(), index, SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        let schema = format!("aip_test_wbs83_{unique}");
        sqlgen::try_set_schema(&schema).expect("isolated schema name");
        let db = spike_v2_read::connect_owned_with_url(&dsn).await.expect("connect only to the explicit run-owned DSN");
        for statement in sqlgen::create_ddl(&facts).expect("fixture DDL") {
            db.batch_execute(&statement).await.expect("create fixture business schema");
        }
        spike_v6_transport::prepare(&db).await;
        db.batch_execute(&format!(
            "CREATE TABLE {schema}.aip_principals(issuer text NOT NULL,subject text NOT NULL,actor_id bigint NOT NULL,enabled boolean NOT NULL,min_iat bigint NOT NULL DEFAULT 0,PRIMARY KEY(issuer,subject));\
             INSERT INTO {schema}.aip_principals(issuer,subject,actor_id,enabled) VALUES ('{ISSUER}','{SUBJECT}',42,true);\
             INSERT INTO {schema}.member(id) VALUES (42);\
             INSERT INTO {schema}.\"order\"(id,member_id,paid) VALUES (101,42,false)"
        )).await.expect("seed only the isolated fixture schema");

        let (_jwks_dir, auth_config, signing_key) = jwt_fixture();
        let jwt = issue_jwt(&signing_key);
        let auth = Arc::new(AuthenticatorImpl::new(auth_config, dsn.clone(), schema.clone()).await.expect("real JWT verifier configuration"));

        let (endpoint, collector_payloads) = match *scenario {
            "normal" => {
                let (addr, hits) = collector(Duration::ZERO).await;
                (addr, Some(hits))
            }
            "connection_refused" => {
                let socket = TcpListener::bind("127.0.0.1:0").await.expect("reserve refusal port");
                let addr = socket.local_addr().unwrap();
                drop(socket);
                (addr, None)
            }
            "delay_2s" => {
                let (addr, hits) = collector(Duration::from_secs(2)).await;
                (addr, Some(hits))
            }
            _ => unreachable!(),
        };
        let infra = Infrastructure {
            otlp: Some(OtlpConfig { endpoint: format!("http://{endpoint}"), headers: HashMap::new(), timeout_ms: 300 }),
            metrics_token: Some("wbs83-api-metrics-fixture-token".into()),
            ..Infrastructure::default()
        };
        let server = listen_with_infrastructure(
            facts.clone(), IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), dsn.clone(), None,
            ServerOptions::default(), false, auth, None, None, false, infra,
        ).await.expect("supported authenticated server with no worker extension configured");
        let contract = contract_fingerprint_with_all_extensions(&facts, IdWire::DecimalString);
        let session = http(server.address, "/session", &jwt, &contract, &format!("{:032x}", 0x1000 + index), json!({})).await;
        eprintln!("RESULT scenario={scenario} route=/session http_status={} elapsed_ms={} body={}", session.0, session.2.as_millis(), session.1);
        assert_eq!(session.0, 200);
        assert_eq!(session.1["ok"], true, "valid existing JWT fixture must authenticate through the product API");
        assert_eq!(session.1["principal"]["actorId"], "42");

        let read_trace = format!("{:032x}", 0x2000 + index);
        let read = http(server.address, "/read", &jwt, &contract, &read_trace, json!({"query":{"read":"Order","select":["id","paid"]}})).await;
        eprintln!("RESULT scenario={scenario} route=/read trace_id={read_trace} http_status={} elapsed_ms={} body={}", read.0, read.2.as_millis(), read.1);
        assert_eq!(read.0, 200);
        assert_eq!(read.1["ok"], true, "authorized read must execute the real DB query");
        assert_eq!(read.1["rows"], json!([{"id":"101","paid":false}]));

        let apply_trace = format!("{:032x}", 0x3000 + index);
        let apply = http(server.address, "/apply", &jwt, &contract, &apply_trace, json!({
            "key": format!("collector-{scenario}-{unique}"),
            "request":{"apply":"Order.pay","target":{"ids":["101"]}}
        })).await;
        eprintln!("RESULT scenario={scenario} route=/apply trace_id={apply_trace} http_status={} elapsed_ms={} body={}", apply.0, apply.2.as_millis(), apply.1);
        assert_eq!(apply.0, 200);
        assert_eq!(apply.1["ok"], true, "authorized direct apply must commit its real DB effect");
        assert_eq!(apply.1["changed"], json!(["101"]));
        let row = db.query_one(&format!("SELECT paid,(SELECT count(*) FROM {schema}.aip_idem) FROM {schema}.\"order\" WHERE id=101"), &[]).await.unwrap();
        assert!(row.get::<_, bool>(0), "database effect must persist after HTTP apply");
        assert_eq!(row.get::<_, i64>(1), 1, "successful apply must persist one idempotency result");
        eprintln!("DB_EFFECT scenario={scenario} schema={schema} order_id=101 paid={} aip_idem_rows={}", row.get::<_, bool>(0), row.get::<_, i64>(1));

        if let Some(payloads) = collector_payloads {
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                let bodies = payloads.lock().await.clone();
                let all = bodies.join("\n");
                if all.contains(&read_trace) && all.contains(&apply_trace) {
                    let value: Value = serde_json::from_str(&bodies.join("\n")).unwrap_or(Value::Null);
                    eprintln!("COLLECTOR_MATCH scenario={scenario} hits={} read_trace={read_trace} apply_trace={apply_trace} spans_json={}", bodies.len(), all);
                    assert!(all.contains("HTTP read") && all.contains("HTTP apply"), "matched spans should identify both API routes");
                    let _ = value;
                    break;
                }
                assert!(Instant::now() < deadline, "collector did not receive both API trace ids; captured bodies={bodies:?}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(400)).await;
            eprintln!("COLLECTOR_MATCH scenario={scenario} expected_hits=0 because the reserved loopback port was closed before API start");
        }
        server.shutdown().await.expect("shutdown API server");
        db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.expect("drop this case's isolated schema");
        eprintln!("SCENARIO_COMPLETE scenario={scenario} schema_dropped=true");
    }
}
