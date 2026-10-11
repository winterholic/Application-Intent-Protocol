use aip_execution::objects::{LocalConfig, LocalStore};
use serde_json::Value;
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    hosting::{MountAccess, StaticMount},
    infrastructure::{Infrastructure, ObjectStorage},
    server::{listen_with_infrastructure, DeploymentFence, ServerOptions},
};
use std::io::{self, Read, Write};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

struct Source {
    remaining: usize,
    reads: Arc<AtomicUsize>,
}
impl Read for Source {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let n = self.remaining.min(out.len()).min(16 * 1024);
        out[..n].fill(0x5a);
        self.remaining -= n;
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(n)
    }
}

struct Verify {
    bytes: usize,
    valid: bool,
}
impl Write for Verify {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes += bytes.len();
        self.valid &= bytes.iter().all(|byte| *byte == 0x5a);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn owner_namespaces_stream_objects_larger_than_the_json_limit() {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("aip-object-transport-{}-{nonce}", std::process::id()));
    let base = LocalStore::new(LocalConfig::new(&root).with_max_bytes(8 * 1024 * 1024)).unwrap();
    let alice = base.for_owner("issuer|alice").unwrap();
    let bob = base.for_owner("issuer|bob").unwrap();
    let source_reads = Arc::new(AtomicUsize::new(0));
    let size = 2 * 1024 * 1024 + 31;
    let (id, metadata) = alice.put(Source { remaining: size, reads: source_reads.clone() }, "application/octet-stream").unwrap();
    assert_eq!(metadata.size, size as u64);
    assert!(source_reads.load(Ordering::Relaxed) > 100);

    let mut output = Verify { bytes: 0, valid: true };
    assert_eq!(alice.get(&id, &mut output).unwrap(), metadata);
    assert_eq!(output.bytes, size);
    assert!(output.valid);
    assert_eq!(bob.stat(&id).unwrap(), None);
    std::fs::remove_dir_all(root).unwrap();
}

static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Auth {
    schema: String,
    iat: i64,
}
impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let grant = DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: 1, issued_at: self.iat, min_iat: 0 };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.iat + 600)
        })
    }
    fn verify_durable<'a>(
        &'a self,
        grant: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.map_err(|_| AuthError::Unavailable)?;
            let row = db
                .query_opt(
                    &format!("SELECT enabled,min_iat FROM {}.aip_principals WHERE issuer=$1 AND subject=$2 AND actor_id=$3", self.schema),
                    &[&grant.issuer, &grant.subject, &grant.actor_id],
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            if row.is_some_and(|row| row.get::<_, bool>(0) && row.get::<_, i64>(1) == grant.min_iat) {
                Ok(())
            } else {
                Err(AuthError::BadSignature)
            }
        })
    }
}

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::Builder::new().name("aip-object-test-schema-cleanup".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        if let Ok(cleanup) = cleanup {
            let _ = cleanup.join();
        }
    }
}

async fn raw_request(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    fingerprint: Option<&str>,
    declared_length: usize,
    payload: &[u8],
) -> Vec<u8> {
    raw_request_with_origin(address, method, path, token, fingerprint, declared_length, payload, None).await
}

#[allow(clippy::too_many_arguments)]
async fn raw_request_with_origin(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    fingerprint: Option<&str>,
    declared_length: usize,
    payload: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let contract_header = fingerprint.map(|value| format!("x-aip-contract: {value}\r\n")).unwrap_or_default();
    let origin_header = origin.map(|value| format!("origin: {value}\r\n")).unwrap_or_default();
    let authorization_header = if token.is_empty() { String::new() } else { format!("authorization: Bearer {token}\r\n") };
    let content_type = if method == "POST" { "content-type: application/octet-stream\r\n" } else { "" };
    let headers = format!(
        "{method} {path} HTTP/1.1\r\nhost: {address}\r\n{origin_header}{authorization_header}{contract_header}{content_type}content-length: {declared_length}\r\nconnection: close\r\n\r\n"
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(payload).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut response)).await.unwrap().unwrap();
    response
}

async fn preflight(address: std::net::SocketAddr, path: &str, origin: &str, requested_method: &str, requested_headers: &str) -> Vec<u8> {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "OPTIONS {path} HTTP/1.1\r\nhost: {address}\r\norigin: {origin}\r\naccess-control-request-method: {requested_method}\r\naccess-control-request-headers: {requested_headers}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut response)).await.unwrap().unwrap();
    response
}

async fn json_control_request(address: std::net::SocketAddr, path: &str, token: &str, fingerprint: &str, origin: &str) -> Vec<u8> {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let body = r#"{"method":"PUT","size":16,"contentType":"text/plain"}"#;
    let request = format!(
        "POST {path} HTTP/1.1\r\nhost: {address}\r\norigin: {origin}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut response)).await.unwrap().unwrap();
    response
}

fn response_parts(response: &[u8]) -> (u16, Vec<(String, String)>, &[u8]) {
    let split = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
    let headers = std::str::from_utf8(&response[..split]).unwrap();
    let mut lines = headers.split("\r\n");
    let status = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|line| line.split_once(':')).map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned())).collect();
    (status, headers, &response[split + 4..])
}

fn decode_chunks(mut bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    loop {
        let line_end = bytes.windows(2).position(|part| part == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&bytes[..line_end]).unwrap(), 16).unwrap();
        bytes = &bytes[line_end + 2..];
        if size == 0 {
            break;
        }
        output.extend_from_slice(&bytes[..size]);
        bytes = &bytes[size + 2..];
    }
    output
}

fn json_body(response: &[u8]) -> Value {
    let (_, _, body) = response_parts(response);
    serde_json::from_slice(body).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_http_object_routes_stream_large_bytes_and_enforce_owner_contract_and_limits() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let schema = format!("aip_object_transport_{}_{}", std::process::id(), nonce);
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES (1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES ('test','owner',1),('test','other',1)"
    ))
    .await
    .unwrap();
    let issued_at: i64 = db.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let root = std::env::temp_dir().join(format!("aip-object-http-{}-{nonce}", std::process::id()));
    let store = LocalStore::new(LocalConfig::new(&root).with_max_bytes(3 * 1024 * 1024).with_owner_quota(8 * 1024 * 1024, 4)).unwrap();
    let assets = root.with_extension("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("hello.txt"), b"asset").unwrap();
    let mut infrastructure = Infrastructure { objects: Some(ObjectStorage::Local(Arc::new(store))), ..Infrastructure::default() };
    infrastructure.mounts.push(StaticMount::new("/assets".into(), assets.clone(), MountAccess::Public).unwrap());
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), iat: issued_at }),
        Some(DeploymentFence::new(schema.clone(), digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();
    let origin = format!("http://{}", server.address);

    let preflight_get = preflight(server.address, "/objects/example", &origin, "GET", "authorization,x-aip-contract").await;
    let (status, headers, _) = response_parts(&preflight_get);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&preflight_get));
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.contains(&("access-control-allow-methods".into(), "GET, HEAD, DELETE".into())));
    assert!(headers.iter().any(|(name, value)| name == "vary" && value.contains("Access-Control-Request-Method")));

    let preflight_post = preflight(server.address, "/objects/put", &origin, "POST", "authorization,content-type,x-aip-contract").await;
    let (status, headers, _) = response_parts(&preflight_post);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&preflight_post));
    assert!(headers.contains(&("access-control-allow-methods".into(), "POST".into())));

    let preflight_mount = preflight(server.address, "/assets/hello.txt", &origin, "GET", "authorization").await;
    let (status, headers, _) = response_parts(&preflight_mount);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&preflight_mount));
    assert!(headers.contains(&("access-control-allow-methods".into(), "GET, HEAD".into())));

    let asset_response = raw_request_with_origin(server.address, "GET", "/assets/hello.txt", "", None, 0, &[], Some(&origin)).await;
    let (_, headers, body) = response_parts(&asset_response);
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.iter().any(|(name, value)| name == "vary" && value.eq_ignore_ascii_case("Origin")));
    assert!(headers.contains(&("x-content-type-options".into(), "nosniff".into())), "{headers:?}");
    assert_eq!(body, b"asset");
    let denied_origin = raw_request_with_origin(server.address, "GET", "/assets/hello.txt", "", None, 0, &[], Some("https://attacker.example")).await;
    let (_, headers, _) = response_parts(&denied_origin);
    assert_eq!(json_body(&denied_origin)["code"], "ORIGIN_NOT_ALLOWED");
    assert!(!headers.iter().any(|(name, _)| name == "access-control-allow-origin"));

    let size = 2 * 1024 * 1024 + 31;
    let payload = vec![0x5a; size];
    let uploaded = raw_request_with_origin(server.address, "POST", "/objects/put", "owner", Some(&fingerprint), size, &payload, Some(&origin)).await;
    let (status, headers, _) = response_parts(&uploaded);
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&uploaded));
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.contains(&("cache-control".into(), "private, no-store".into())));
    assert!(headers.contains(&("x-content-type-options".into(), "nosniff".into())));
    let object_id = json_body(&uploaded)["id"].as_str().unwrap().to_owned();

    let preflight_delete = preflight(server.address, &format!("/objects/{object_id}"), &origin, "DELETE", "authorization,x-aip-contract").await;
    let (status, headers, _) = response_parts(&preflight_delete);
    assert_eq!(status, 200);
    assert!(headers.contains(&("access-control-allow-methods".into(), "GET, HEAD, DELETE".into())));
    let disallowed_preflight = preflight(server.address, &format!("/objects/{object_id}"), &origin, "POST", "authorization,x-aip-contract").await;
    let (status, headers, body) = response_parts(&disallowed_preflight);
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_slice::<Value>(body).unwrap()["code"], "BAD_REQUEST");
    assert!(!headers.iter().any(|(name, _)| name == "access-control-allow-methods"));

    let downloaded =
        raw_request_with_origin(server.address, "GET", &format!("/objects/{object_id}"), "owner", Some(&fingerprint), 0, &[], Some(&origin)).await;
    let (status, headers, body) = response_parts(&downloaded);
    assert_eq!(status, 200);
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.contains(&("cache-control".into(), "private, no-store".into())));
    assert!(headers.contains(&("x-content-type-options".into(), "nosniff".into())));
    assert!(headers.contains(&("content-disposition".into(), "attachment".into())));
    assert_eq!(headers.iter().find(|(name, _)| name == "transfer-encoding").unwrap().1, "chunked");
    assert_eq!(decode_chunks(body), payload);

    let headed =
        raw_request_with_origin(server.address, "HEAD", &format!("/objects/{object_id}"), "owner", Some(&fingerprint), 0, &[], Some(&origin)).await;
    let (status, headers, body) = response_parts(&headed);
    assert_eq!(status, 200);
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.contains(&("cache-control".into(), "private, no-store".into())));
    assert!(headers.contains(&("x-content-type-options".into(), "nosniff".into())));
    assert!(headers.contains(&("content-disposition".into(), "attachment".into())));
    assert_eq!(headers.iter().find(|(name, _)| name == "content-length").unwrap().1, size.to_string());
    assert!(body.is_empty());

    let foreign = raw_request(server.address, "GET", &format!("/objects/{object_id}"), "other", Some(&fingerprint), 0, &[]).await;
    assert_eq!(response_parts(&foreign).0, 404, "another principal must not read this namespace");
    let foreign_head = raw_request(server.address, "HEAD", &format!("/objects/{object_id}"), "other", Some(&fingerprint), 0, &[]).await;
    let (status, _, body) = response_parts(&foreign_head);
    assert_eq!(status, 404, "another principal must not read this namespace");
    assert!(body.is_empty(), "HEAD responses must not contain an error body");

    let missing_head =
        raw_request(server.address, "HEAD", "/objects/00000000-0000-0000-0000-000000000000", "owner", Some(&fingerprint), 0, &[]).await;
    let (status, _, body) = response_parts(&missing_head);
    assert_eq!(status, 404);
    assert!(body.is_empty(), "missing-object HEAD responses must not contain an error body");

    let contract_head = raw_request(server.address, "HEAD", &format!("/objects/{object_id}"), "owner", None, 0, &[]).await;
    let (status, headers, body) = response_parts(&contract_head);
    assert_eq!(status, 200);
    assert!(headers.iter().any(|(name, value)| name == "cache-control" && value == "private, no-store"));
    assert!(body.is_empty(), "contract-error HEAD responses must not contain a body");
    let unauthorized_head =
        raw_request(server.address, "HEAD", &format!("/objects/{object_id}"), "revoked-or-unknown", Some(&fingerprint), 0, &[]).await;
    let (status, headers, body) = response_parts(&unauthorized_head);
    assert_eq!(status, 200);
    assert!(headers.iter().any(|(name, value)| name == "cache-control" && value == "private, no-store"));
    assert!(body.is_empty(), "authentication-error HEAD responses must not contain a body");

    let no_contract = raw_request_with_origin(server.address, "POST", "/objects/put", "owner", None, 10, &[], Some(&origin)).await;
    let (_, headers, _) = response_parts(&no_contract);
    assert_eq!(json_body(&no_contract)["code"], "CONTRACT_MISMATCH");
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.contains(&("cache-control".into(), "private, no-store".into())));
    assert!(headers.contains(&("x-content-type-options".into(), "nosniff".into())));

    let control = json_control_request(server.address, "/objects/sign", "owner", &fingerprint, &origin).await;
    let (_, headers, _) = response_parts(&control);
    assert_eq!(json_body(&control)["code"], "NOT_AVAILABLE");
    assert!(headers.contains(&("access-control-allow-origin".into(), origin.clone())));
    assert!(headers.contains(&("cache-control".into(), "private, no-store".into())));
    assert!(headers.contains(&("x-content-type-options".into(), "nosniff".into())));

    let oversized_length = 3 * 1024 * 1024 + 1;
    let oversized = raw_request(server.address, "POST", "/objects/put", "owner", Some(&fingerprint), oversized_length, &[]).await;
    assert_eq!(json_body(&oversized)["code"], "PAYLOAD_TOO_LARGE");

    let deleted = raw_request(server.address, "DELETE", &format!("/objects/{object_id}"), "owner", Some(&fingerprint), 0, &[]).await;
    assert_eq!(response_parts(&deleted).0, 204);
    let missing = raw_request(server.address, "GET", &format!("/objects/{object_id}"), "owner", Some(&fingerprint), 0, &[]).await;
    assert_eq!(response_parts(&missing).0, 404);
    server.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(assets).unwrap();
}
