use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_extensions;
use spike_v6_transport::server::{listen_with_extensions, ReadExtensions, WorkerLang};
use std::{
    io,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const BAD_DB: &str = "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1";
const ROUTE: &str = "/extension";
static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);

fn facts(implementation: &str) -> Value {
    let mut facts = load_str(FIXTURE, Form::A).unwrap_or_else(|error| panic!("Recruitment fixture invalid: {error:?}")).execution;
    facts["resources"]["Recruitment"]["extensions"]["stats"]["implementation"] = json!(implementation);
    facts
}

fn schema_name() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_nanos();
    let id = NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed);
    format!("aip_ext_{}_{}_{}", std::process::id(), nanos, id)
}

fn worker_config(lang: WorkerLang) -> ReadExtensions {
    ReadExtensions { lang, dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-v4-worker/extensions")) }
}

fn id_wire_modes() -> [(IdWire, &'static str); 2] {
    [(IdWire::SafeNumber, "safe"), (IdWire::DecimalString, "decimal")]
}

fn langs() -> [(WorkerLang, &'static str); 2] {
    [(WorkerLang::Node, "Node"), (WorkerLang::Python, "Python")]
}

fn wire_id(wire: IdWire) -> Value {
    match wire {
        IdWire::SafeNumber => json!(10),
        IdWire::DecimalString => json!("10"),
        IdWire::Legacy => unreachable!("test covers only safe and decimal wires"),
    }
}

fn wrong_wire_id(wire: IdWire) -> Value {
    match wire {
        IdWire::SafeNumber => json!("10"),
        IdWire::DecimalString => json!(10),
        IdWire::Legacy => unreachable!("test covers only safe and decimal wires"),
    }
}

fn body(extension: &str, club_id: Value) -> Value {
    json!({ "extension": extension, "input": { "clubId": club_id } })
}

async fn post(address: SocketAddr, token: Option<&str>, contract: Option<&str>, body: &Value) -> Result<Value, String> {
    let mut socket = TcpStream::connect(address).await.map_err(|error| format!("connect: {:?}", error.kind()))?;
    let body = body.to_string();
    let authorization = token.map(|token| format!("authorization: Bearer {token}\r\n")).unwrap_or_default();
    let contract = contract.map(|fingerprint| format!("x-aip-contract: {fingerprint}\r\n")).unwrap_or_default();
    let request = format!("POST {ROUTE} HTTP/1.1\r\nhost: 127.0.0.1\r\n{authorization}{contract}content-length: {}\r\n\r\n{body}", body.len());
    socket.write_all(request.as_bytes()).await.map_err(|error| format!("write: {:?}", error.kind()))?;
    let mut response = Vec::new();
    timeout(Duration::from_secs(6), socket.read_to_end(&mut response))
        .await
        .map_err(|_| "HTTP response timeout".to_string())?
        .map_err(|error| format!("read: {:?}", error.kind()))?;
    let response = String::from_utf8(response).map_err(|_| "HTTP response is not UTF-8".to_string())?;
    let (_, response_body) = response.split_once("\r\n\r\n").ok_or_else(|| format!("HTTP response has no body: {response}"))?;
    serde_json::from_str(response_body).map_err(|error| format!("invalid JSON response: {error}"))
}

fn expect_code(failures: &mut Vec<String>, label: &str, result: Result<Value, String>, expected: &str) {
    match result {
        Ok(body) if body["code"] == expected => {}
        Ok(body) => failures.push(format!("{label}: expected {expected}, got {body}")),
        Err(error) => failures.push(format!("{label}: {error}")),
    }
}

async fn start_server(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: &str,
    extensions: Option<ReadExtensions>,
) -> io::Result<spike_v6_transport::server::Server> {
    listen_with_extensions(facts, wire, address, db_url.to_string(), extensions).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_extensions_cross_http_auth_contract_wire_and_worker_boundaries() {
    let schema = schema_name();
    sqlgen::set_schema(&schema);
    let base_facts = facts("recruitment.stats");
    let mut failures = Vec::new();
    let db = match connect_with_url(spike_v2_read::DB_URL).await {
        Ok(db) => db,
        Err(error) => panic!("local PostgreSQL connection failed: {error}"),
    };

    let ddl = match sqlgen::create_ddl(&base_facts) {
        Ok(ddl) => ddl,
        Err(error) => panic!("Recruitment DDL generation failed: {error}"),
    };
    let mut owns_schema = false;
    if let Some(create_schema) = ddl.first() {
        match db.batch_execute(create_schema).await {
            Ok(()) => owns_schema = true,
            Err(error) => failures.push(format!("create owned schema failed: {:?}", error.code())),
        }
    } else {
        failures.push("create_ddl returned no schema statement".into());
    }

    if owns_schema {
        for statement in ddl.iter().skip(1) {
            if let Err(error) = db.batch_execute(statement).await {
                failures.push(format!("Recruitment DDL failed: {:?}", error.code()));
                break;
            }
        }
        if failures.is_empty() {
            let seed = format!(
                "INSERT INTO {schema}.school (id, name) VALUES (1, 'Test');\
                 INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1);\
                 INSERT INTO {schema}.club (id, name, logo, school_id) VALUES (10, 'Club', NULL, 1);\
                 INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 2, 'MEMBER');\
                 INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note)\
                   VALUES (100, 'Open', '2099-01-01T00:00:00Z', 'PUBLISHED', 0, 10, NULL);\
                 INSERT INTO {schema}.apply (id, recruitment_id, status) VALUES (200, 100, 'APPROVE');"
            );
            if let Err(error) = db.batch_execute(&seed).await {
                failures.push(format!("Recruitment seed failed: {:?}", error.code()));
            }
        }
    }

    if owns_schema && failures.is_empty() {
        for (wire, wire_name) in id_wire_modes() {
            for (lang, lang_name) in langs() {
                let config = worker_config(lang);
                let contract = contract_fingerprint_with_extensions(&base_facts, wire);
                let extension_input = body("Recruitment.stats", wire_id(wire));

                // Missing configuration, authentication, contract, and typed-input failures must not reach the DB.
                let no_config = start_server(base_facts.clone(), wire, "127.0.0.1:0".parse().unwrap(), BAD_DB, None).await;
                match no_config {
                    Ok(server) => {
                        let response = post(server.address, None, None, &extension_input).await;
                        expect_code(&mut failures, &format!("{wire_name}/{lang_name} no worker config"), response, "NOT_FOUND");
                        if let Err(error) = server.shutdown().await {
                            failures.push(format!("no-config server shutdown failed: {:?}", error.kind()));
                        }
                    }
                    Err(error) => failures.push(format!("{wire_name}/{lang_name} no-config server startup failed: {:?}", error.kind())),
                }

                let preflight = start_server(base_facts.clone(), wire, "127.0.0.1:0".parse().unwrap(), BAD_DB, Some(config.clone())).await;
                match preflight {
                    Ok(server) => {
                        let token = server.keys.issue(1, 600);
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} missing token"),
                            post(server.address, None, Some(&contract), &extension_input).await,
                            "UNAUTHENTICATED",
                        );
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} missing contract"),
                            post(server.address, Some(&token), None, &extension_input).await,
                            "CONTRACT_MISMATCH",
                        );
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} wrong contract"),
                            post(server.address, Some(&token), Some(&"0".repeat(64)), &extension_input).await,
                            "CONTRACT_MISMATCH",
                        );
                        let mut extra = extension_input.clone();
                        extra["extra"] = json!(true);
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} extra body key"),
                            post(server.address, Some(&token), Some(&contract), &extra).await,
                            "BAD_REQUEST",
                        );
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} unknown extension"),
                            post(server.address, Some(&token), Some(&contract), &body("Recruitment.notDeclared", wire_id(wire))).await,
                            "NOT_EXPOSED",
                        );
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} wrong Id wire"),
                            post(server.address, Some(&token), Some(&contract), &body("Recruitment.stats", wrong_wire_id(wire))).await,
                            "BAD_VALUE",
                        );
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} valid request with unavailable DB"),
                            post(server.address, Some(&token), Some(&contract), &extension_input).await,
                            "DB_UNAVAILABLE",
                        );
                        if let Err(error) = server.shutdown().await {
                            failures.push(format!("preflight server shutdown failed: {:?}", error.kind()));
                        }
                    }
                    Err(error) => failures.push(format!("{wire_name}/{lang_name} preflight server startup failed: {:?}", error.kind())),
                }

                let live = start_server(base_facts.clone(), wire, "127.0.0.1:0".parse().unwrap(), spike_v2_read::DB_URL, Some(config.clone())).await;
                match live {
                    Ok(server) => {
                        let token = server.keys.issue(1, 600);
                        let response = post(server.address, Some(&token), Some(&contract), &extension_input).await;
                        match response {
                            Ok(body) => {
                                if body["ok"] != true || body["output"] != json!({ "approvedApplicants": 1 }) {
                                    failures.push(format!("{wire_name}/{lang_name} valid read result: {body}"));
                                }
                                if body["contractFingerprint"] != contract {
                                    failures.push(format!("{wire_name}/{lang_name} response fingerprint mismatch: {body}"));
                                }
                            }
                            Err(error) => failures.push(format!("{wire_name}/{lang_name} valid read: {error}")),
                        }
                        let other_token = server.keys.issue(2, 600);
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} unauthorized actor ctx"),
                            post(server.address, Some(&other_token), Some(&contract), &extension_input).await,
                            "EXTENSION_ERROR",
                        );
                        if let Err(error) = server.shutdown().await {
                            failures.push(format!("live server shutdown failed: {:?}", error.kind()));
                        }
                    }
                    Err(error) => failures.push(format!("{wire_name}/{lang_name} live server startup failed: {:?}", error.kind())),
                }

                let bad_output_facts = facts("recruitment.statsBadOutput");
                let bad_output =
                    start_server(bad_output_facts.clone(), wire, "127.0.0.1:0".parse().unwrap(), spike_v2_read::DB_URL, Some(config.clone())).await;
                match bad_output {
                    Ok(server) => {
                        let token = server.keys.issue(1, 600);
                        let response = post(server.address, Some(&token), Some(&contract), &extension_input).await;
                        expect_code(&mut failures, &format!("{wire_name}/{lang_name} invalid worker output"), response, "OUTPUT_INVALID");
                        if let Err(error) = server.shutdown().await {
                            failures.push(format!("bad-output server shutdown failed: {:?}", error.kind()));
                        }
                    }
                    Err(error) => failures.push(format!("{wire_name}/{lang_name} bad-output server startup failed: {:?}", error.kind())),
                }

                #[cfg(target_os = "macos")]
                {
                    let probe_facts = facts("recruitment.probeDb");
                    let probe = start_server(probe_facts, wire, "127.0.0.1:0".parse().unwrap(), spike_v2_read::DB_URL, Some(config.clone())).await;
                    match probe {
                        Ok(server) => {
                            let token = server.keys.issue(1, 600);
                            match post(server.address, Some(&token), Some(&contract), &extension_input).await {
                                Ok(body) if body["output"] == json!({ "approvedApplicants": 0 }) => {}
                                Ok(body) => failures.push(format!("{wire_name}/{lang_name} MacNetDeny TCP probe: expected 0, got {body}")),
                                Err(error) => failures.push(format!("{wire_name}/{lang_name} MacNetDeny TCP probe: {error}")),
                            }
                            if let Err(error) = server.shutdown().await {
                                failures.push(format!("probe server shutdown failed: {:?}", error.kind()));
                            }
                        }
                        Err(error) => failures.push(format!("{wire_name}/{lang_name} probe server startup failed: {:?}", error.kind())),
                    }
                }

                let slow_facts = facts("recruitment.statsSlow");
                let slow = start_server(slow_facts, wire, "127.0.0.1:0".parse().unwrap(), spike_v2_read::DB_URL, Some(config)).await;
                match slow {
                    Ok(server) => {
                        let token = server.keys.issue(1, 600);
                        let started = Instant::now();
                        expect_code(
                            &mut failures,
                            &format!("{wire_name}/{lang_name} bounded deadline"),
                            post(server.address, Some(&token), Some(&contract), &extension_input).await,
                            "DEADLINE_EXCEEDED",
                        );
                        if started.elapsed() > Duration::from_millis(2800) {
                            failures.push(format!("{wire_name}/{lang_name} deadline response took {:?}", started.elapsed()));
                        }
                        if let Err(error) = server.shutdown().await {
                            failures.push(format!("slow server shutdown failed: {:?}", error.kind()));
                        }
                    }
                    Err(error) => failures.push(format!("{wire_name}/{lang_name} slow server startup failed: {:?}", error.kind())),
                }
            }
        }
    }

    let invalid_facts = facts("../outside.stats");
    let reserved = TcpListener::bind("127.0.0.1:0").expect("reserve a loopback port");
    let invalid_path =
        start_server(invalid_facts, IdWire::SafeNumber, reserved.local_addr().unwrap(), BAD_DB, Some(worker_config(WorkerLang::Node))).await;
    match invalid_path {
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
        Err(error) => failures.push(format!("invalid worker implementation path: expected InvalidInput, got {:?}", error.kind())),
        Ok(server) => {
            failures.push("invalid worker implementation path was accepted".into());
            if let Err(error) = server.shutdown().await {
                failures.push(format!("invalid-path server shutdown failed: {:?}", error.kind()));
            }
        }
    }
    drop(reserved);

    if owns_schema {
        if let Err(error) = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await {
            failures.push(format!("owned schema cleanup failed: {:?}", error.code()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
