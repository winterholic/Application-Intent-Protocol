//! Per-owner quota and expired-upload cleanup for S3 transfers (WBS 8.5), through the real HTTP server and PostgreSQL.
//!
//! The first group never reaches an object store: signing a URL is local. The `live_*` group needs the TLS MinIO fixture
//! started by `scripts/test-minio-object-storage.sh` and is ignored otherwise.

use aip_execution::objects::{ObjectKey, S3Config, S3Store};
use serde_json::{json, Value};
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::{Infrastructure, ObjectStorage},
    server::{listen_with_infrastructure, DeploymentFence, Server, ServerOptions},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const DATABASE: &str = aip_test_support::test_database_url!();
const MIB: u64 = 1024 * 1024;
static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
// sqlgen keeps one schema per process, so every test recreates the same one, serially.
static SCHEMA: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("aip_object_ledger_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
});

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
            let db = spike_v2_read::connect_owned_with_url(DATABASE).await.map_err(|_| AuthError::Unavailable)?;
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
        let cleanup = std::thread::Builder::new().name("aip-ledger-test-schema-cleanup".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(DATABASE).await {
                    let infra = aip_execution::infrastructure_schema(&schema).unwrap();
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {infra} CASCADE")).await;
                }
            });
        });
        if let Ok(cleanup) = cleanup {
            let _ = cleanup.join();
        }
    }
}

struct Fixture {
    server: Server,
    fingerprint: String,
    infra: String,
    db: spike_v2_read::OwnedConnection,
    _cleanup: SchemaCleanup,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    async fn start(store: S3Store) -> Self {
        let lock = SCHEMA_LOCK.lock().await;
        let schema = SCHEMA.clone();
        sqlgen::set_schema(&schema);
        let cleanup = SchemaCleanup(schema.clone());
        let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
        aip_migrate::init_with_wire(DATABASE, &schema, &facts, "safe").await.unwrap();
        let db = spike_v2_read::connect_owned_with_url(DATABASE).await.unwrap();
        db.batch_execute(&format!(
            "INSERT INTO {schema}.aip_actors VALUES (1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES ('test','alice',1),('test','bob',1)"
        ))
        .await
        .unwrap();
        let issued_at: i64 = db.query_one("SELECT floor(extract(epoch from clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
        let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
        let infrastructure = Infrastructure {
            objects: Some(ObjectStorage::S3(Arc::new(store))),
            object_reconcile_interval: Duration::from_millis(200),
            ..Infrastructure::default()
        };
        let server = listen_with_infrastructure(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            DATABASE.into(),
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
        let infra = aip_execution::infrastructure_schema(&schema).unwrap();
        Self { server, fingerprint, infra, db, _cleanup: cleanup, _lock: lock }
    }

    fn address(&self) -> SocketAddr {
        self.server.address
    }

    async fn control(&self, token: &str, path: &str, body: Value) -> Value {
        let mut socket = TcpStream::connect(self.address()).await.unwrap();
        let body = body.to_string();
        let request = format!(
            "POST {path} HTTP/1.1\r\nhost: {}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            self.address(),
            self.fingerprint,
            body.len()
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), socket.read_to_end(&mut response)).await.unwrap().unwrap();
        let split = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
        serde_json::from_slice(&response[split + 4..]).unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&response)))
    }

    async fn number(&self, sql: &str) -> i64 {
        self.db.query_one(&sql.replace("{infra}", &self.infra), &[]).await.unwrap().get(0)
    }

    async fn make_all_due(&self) {
        self.db
            .batch_execute(&format!(
                "UPDATE {}.aip_object_ledger SET expires_at=clock_timestamp()-interval '1 second' WHERE expires_at IS NOT NULL",
                self.infra
            ))
            .await
            .unwrap();
    }

    async fn wait_until(&self, what: &str, sql: &str, expected: i64) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let value = self.number(sql).await;
            if value == expected {
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "{what}: wanted {expected}, still {value}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn offline_store(max_bytes: u64, max_objects: usize) -> S3Store {
    S3Store::new(S3Config::new("https://objects.example.test", "bucket", "us-east-1", "ak", "sk").with_owner_quota(max_bytes, max_objects)).unwrap()
}

fn put_request(size: u64) -> Value {
    json!({"method":"PUT","size":size,"contentType":"application/octet-stream"})
}

const BYTES: &str = "SELECT COALESCE(sum(bytes),0)::bigint FROM {infra}.aip_object_ledger";
const ROWS: &str = "SELECT count(*) FROM {infra}.aip_object_ledger";
const HELD: &str = "SELECT count(*) FROM {infra}.aip_object_ledger WHERE settling";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_uploads_is_refused_once_the_owner_quota_is_signed_for() {
    let fixture = Fixture::start(offline_store(10 * MIB, 3)).await;
    let first = fixture.control("alice", "/objects/sign", put_request(6 * MIB)).await;
    assert_eq!(first["ok"], true, "{first}");
    let over = fixture.control("alice", "/objects/sign", put_request(5 * MIB)).await;
    assert_eq!((over["ok"].clone(), over["code"].clone()), (json!(false), json!("WIRE.QUOTA_EXCEEDED")), "{over}");
    assert!(over.get("url").is_none(), "a refused upload must not leak a usable URL: {over}");
    assert_eq!(fixture.number(BYTES).await, 6 * MIB as i64, "a refusal reserves nothing");

    // Quota is per owner.
    assert_eq!(fixture.control("bob", "/objects/sign", put_request(10 * MIB)).await["ok"], true);

    // Signing the same object again replaces its reservation instead of adding to it.
    let id = first["id"].as_str().unwrap();
    let replaced =
        fixture.control("alice", "/objects/sign", json!({"method":"PUT","id":id,"size":2 * MIB,"contentType":"application/octet-stream"})).await;
    assert_eq!(replaced["ok"], true, "{replaced}");
    assert_eq!(fixture.control("alice", "/objects/sign", put_request(8 * MIB)).await["ok"], true, "the released 4 MiB makes room");
}

/// Many simultaneous HTTP sign requests from one owner cannot jointly sign more than the quota, and another owner is unaffected.
/// This pins the contract through the server. It is NOT a race detector: the server serializes signing enough that removing the owner lock
/// does not fail it (measured). The lock itself is covered by `concurrent_reservations_cannot_jointly_exceed_the_quota` in aip-execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_sign_requests_cannot_jointly_exceed_the_owner_quota() {
    let fixture = Fixture::start(offline_store(10 * MIB, 100)).await;
    let attempts = futures_util::future::join_all((0..16).map(|_| fixture.control("alice", "/objects/sign", put_request(3 * MIB)))).await;
    // The server also bounds concurrent signing (WIRE.OBJECT_CAPACITY); the quota must hold whichever way an attempt is answered.
    for body in &attempts {
        let code = body["code"].as_str().unwrap_or("");
        assert!(body["ok"] == true || code == "WIRE.QUOTA_EXCEEDED" || code == "WIRE.OBJECT_CAPACITY", "unexpected answer: {body}");
    }
    let signed = attempts.iter().filter(|body| body["ok"] == true).count() as i64;
    assert!((1..=3).contains(&signed), "10 MiB holds at most three 3 MiB uploads, signed {signed}");
    assert_eq!(fixture.number(BYTES).await, signed * 3 * MIB as i64, "the ledger holds exactly what was signed");
    // Serially, the remaining room is used and then refused.
    for _ in signed..3 {
        assert_eq!(fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await["ok"], true);
    }
    assert_eq!(fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await["code"], "WIRE.QUOTA_EXCEEDED");
    assert_eq!(fixture.number(BYTES).await, 9 * MIB as i64);
    assert_eq!(fixture.control("bob", "/objects/sign", put_request(10 * MIB)).await["ok"], true, "quota is per owner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn object_count_ceiling_applies_to_signed_uploads() {
    let fixture = Fixture::start(offline_store(100 * MIB, 2)).await;
    assert_eq!(fixture.control("alice", "/objects/sign", put_request(1)).await["ok"], true);
    assert_eq!(fixture.control("alice", "/objects/sign", put_request(1)).await["ok"], true);
    let third = fixture.control("alice", "/objects/sign", put_request(1)).await;
    assert_eq!(third["code"], "WIRE.QUOTA_EXCEEDED", "{third}");
    assert_eq!(fixture.number(ROWS).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multipart_parts_and_completion_need_an_upload_the_server_started() {
    let fixture = Fixture::start(offline_store(10 * MIB, 10)).await;
    let id = ObjectKey::generate().to_string();
    let part = fixture.control("alice", "/objects/multipart/part", json!({"id":id,"uploadId":"made-up","partNumber":1,"size":MIB})).await;
    assert_eq!(part["code"], "WIRE.NOT_FOUND", "an invented upload id is not charged or signed: {part}");
    assert!(part.get("url").is_none());
    let complete =
        fixture.control("alice", "/objects/multipart/complete", json!({"id":id,"uploadId":"made-up","parts":[{"number":1,"etag":"\"x\""}]})).await;
    assert_eq!(complete["code"], "WIRE.NOT_FOUND", "{complete}");
    assert_eq!(fixture.number(ROWS).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_delete_for_an_unknown_object_leaves_the_ledger_alone() {
    let fixture = Fixture::start(offline_store(10 * MIB, 10)).await;
    let id = ObjectKey::generate().to_string();
    let signed = fixture.control("alice", "/objects/sign", json!({"method":"DELETE","id":id})).await;
    assert_eq!(signed["ok"], true, "{signed}");
    assert_eq!(fixture.number(ROWS).await, 0);
}

// ---- live MinIO -------------------------------------------------------------------------------------------------------

fn live_store(max_bytes: u64, max_objects: usize) -> (S3Store, reqwest::Client) {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("required live-test variable {name} is missing"));
    let ca = std::fs::read(var("AIP_MINIO_CA_PEM")).expect("read MinIO test CA");
    let store = S3Store::new(
        S3Config::new(var("AIP_MINIO_ENDPOINT"), var("AIP_MINIO_BUCKET"), "us-east-1", var("AIP_MINIO_ACCESS_KEY"), var("AIP_MINIO_SECRET_KEY"))
            .with_root_certificate_pem(&ca)
            .with_owner_quota(max_bytes, max_objects),
    )
    .unwrap();
    let client =
        reqwest::Client::builder().https_only(true).no_proxy().add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap()).build().unwrap();
    (store, client)
}

async fn put_to(client: &reqwest::Client, signed: &Value, bytes: Vec<u8>) -> reqwest::StatusCode {
    let mut request = client.put(signed["url"].as_str().unwrap());
    for (name, value) in signed["headers"].as_object().unwrap() {
        request = request.header(name, value.as_str().unwrap());
    }
    request.body(bytes).send().await.unwrap().status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-minio-object-storage.sh with the TLS MinIO fixture"]
async fn live_expired_signed_put_is_checked_against_the_store() {
    let (store, client) = live_store(10 * MIB, 10);
    let fixture = Fixture::start(store).await;

    let used = fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await;
    assert_eq!(used["ok"], true, "{used}");
    assert!(put_to(&client, &used, vec![7; (3 * MIB) as usize]).await.is_success(), "upload through the signed URL");
    let abandoned = fixture.control("alice", "/objects/sign", put_request(5 * MIB)).await;
    assert_eq!(abandoned["ok"], true, "{abandoned}");
    assert_eq!(fixture.number(BYTES).await, 8 * MIB as i64, "both signed uploads are charged before any timer ends");
    assert_eq!(fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await["code"], "WIRE.QUOTA_EXCEEDED");

    fixture.make_all_due().await;
    fixture.wait_until("the missing object is held for a second look", HELD, 1).await;
    assert_eq!(fixture.number(BYTES).await, 8 * MIB as i64, "one missing HEAD does not release the charge");
    fixture.make_all_due().await;
    fixture.wait_until("the unused 5 MiB reservation is released", BYTES, 3 * MIB as i64).await;
    assert_eq!(fixture.number(ROWS).await, 1, "the uploaded object stays, at its real size");
    assert_eq!(fixture.number("SELECT count(*) FROM {infra}.aip_object_ledger WHERE state='COMPLETE' AND expires_at IS NULL").await, 1);
    assert_eq!(fixture.control("alice", "/objects/sign", put_request(7 * MIB)).await["ok"], true, "released bytes can be signed again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-minio-object-storage.sh with the TLS MinIO fixture"]
async fn live_signed_delete_frees_the_quota_only_once_the_object_is_gone() {
    let (store, client) = live_store(10 * MIB, 10);
    let fixture = Fixture::start(store).await;
    let signed = fixture.control("alice", "/objects/sign", put_request(2 * MIB)).await;
    let id = signed["id"].as_str().unwrap().to_owned();
    assert!(put_to(&client, &signed, vec![1; (2 * MIB) as usize]).await.is_success());
    fixture.make_all_due().await;
    fixture.wait_until("the upload is confirmed", "SELECT count(*) FROM {infra}.aip_object_ledger WHERE state='COMPLETE'", 1).await;

    // A DELETE URL that is never used must not free the quota.
    let unused = fixture.control("alice", "/objects/sign", json!({"method":"DELETE","id":id})).await;
    assert_eq!(unused["ok"], true, "{unused}");
    assert_eq!(fixture.number(BYTES).await, 2 * MIB as i64);
    fixture.make_all_due().await;
    fixture
        .wait_until("an unused delete URL returns the object to COMPLETE", "SELECT count(*) FROM {infra}.aip_object_ledger WHERE state='COMPLETE'", 1)
        .await;

    let used = fixture.control("alice", "/objects/sign", json!({"method":"DELETE","id":id})).await;
    assert!(client.delete(used["url"].as_str().unwrap()).send().await.unwrap().status().is_success());
    fixture.make_all_due().await;
    fixture.wait_until("the deleted object's charge is released", ROWS, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-minio-object-storage.sh with the TLS MinIO fixture"]
async fn live_multipart_is_charged_completed_and_an_idle_upload_is_aborted_in_the_store() {
    let (store, client) = live_store(20 * MIB, 10);
    let probe = store.clone();
    let fixture = Fixture::start(store).await;

    // Completed upload: two parts, the first at the 5 MiB minimum of S3 multipart.
    let started = fixture.control("alice", "/objects/multipart/start", json!({"contentType":"application/octet-stream"})).await;
    assert_eq!(started["ok"], true, "{started}");
    let (id, upload_id) = (started["id"].as_str().unwrap().to_owned(), started["uploadId"].as_str().unwrap().to_owned());
    let mut parts = Vec::new();
    for (number, size) in [(1u64, 5 * MIB), (2, 300)] {
        let signed = fixture.control("alice", "/objects/multipart/part", json!({"id":id,"uploadId":upload_id,"partNumber":number,"size":size})).await;
        assert_eq!(signed["ok"], true, "{signed}");
        let mut request = client.put(signed["url"].as_str().unwrap());
        for (name, value) in signed["headers"].as_object().unwrap() {
            request = request.header(name, value.as_str().unwrap());
        }
        let response = request.body(vec![9; size as usize]).send().await.unwrap();
        assert!(response.status().is_success());
        parts.push(json!({"number":number,"etag":response.headers().get("etag").unwrap().to_str().unwrap()}));
    }
    assert_eq!(fixture.number(BYTES).await, (5 * MIB + 300) as i64);
    let done = fixture.control("alice", "/objects/multipart/complete", json!({"id":id,"uploadId":upload_id,"parts":parts})).await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(fixture.number("SELECT count(*) FROM {infra}.aip_object_ledger WHERE state='COMPLETE'").await, 1);
    assert_eq!(fixture.number("SELECT count(*) FROM {infra}.aip_object_parts").await, 0);

    // Idle upload: a part is signed and uploaded, then nothing happens until the idle timer ends.
    let idle = fixture.control("alice", "/objects/multipart/start", json!({"contentType":"application/octet-stream"})).await;
    let (idle_id, idle_upload) = (idle["id"].as_str().unwrap().to_owned(), idle["uploadId"].as_str().unwrap().to_owned());
    let part = fixture.control("alice", "/objects/multipart/part", json!({"id":idle_id,"uploadId":idle_upload,"partNumber":1,"size":6 * MIB})).await;
    assert_eq!(part["ok"], true, "{part}");
    assert_eq!(fixture.number(BYTES).await, (5 * MIB + 300 + 6 * MIB) as i64);
    fixture
        .db
        .batch_execute(&format!(
            "UPDATE {}.aip_object_ledger SET expires_at=clock_timestamp()-interval '1 second' WHERE state='PENDING'",
            fixture.infra
        ))
        .await
        .unwrap();
    fixture.wait_until("the idle upload's charge is released", BYTES, (5 * MIB + 300) as i64).await;
    // The upload is gone from the store: a part signed earlier can no longer be uploaded to it.
    let late = put_to(&client, &part, vec![5; (6 * MIB) as usize]).await;
    assert_eq!(late, reqwest::StatusCode::NOT_FOUND, "the idle upload was aborted in the store");
    // The completed upload is unaffected.
    assert!(probe
        .for_owner(&owner_of(&fixture).await)
        .unwrap()
        .head_object(&ObjectKey::parse(&id).unwrap())
        .await
        .unwrap()
        .is_some_and(|size| size == 5 * MIB + 300));
}

async fn owner_of(fixture: &Fixture) -> String {
    fixture.db.query_one(&format!("SELECT owner FROM {}.aip_object_ledger LIMIT 1", fixture.infra), &[]).await.unwrap().get(0)
}

/// GUARANTEE: an upload that is still running when the first check finds nothing is still counted when it lands. The first miss keeps
/// the charge for a second look instead of handing the bytes to someone else. `make_all_due` forces the timers; this reproduces the
/// state transitions with a valid signed URL and a real MinIO, not a transfer that really lasted 65 minutes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-minio-object-storage.sh with the TLS MinIO fixture"]
async fn live_an_upload_finishing_after_the_first_check_is_still_counted() {
    let (store, client) = live_store(10 * MIB, 10);
    let fixture = Fixture::start(store).await;
    let signed = fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await;
    assert_eq!(signed["ok"], true, "{signed}");

    // The reservation timer ends before the upload does.
    fixture.make_all_due().await;
    fixture.wait_until("the unfinished upload is held, not released", HELD, 1).await;
    assert_eq!(fixture.number(BYTES).await, 3 * MIB as i64);
    assert_eq!(
        fixture.control("alice", "/objects/sign", put_request(8 * MIB)).await["code"],
        "WIRE.QUOTA_EXCEEDED",
        "the held bytes are not available to another upload"
    );

    // The signed URL is still valid, so the transfer that was "still running" completes now.
    assert!(put_to(&client, &signed, vec![3; (3 * MIB) as usize]).await.is_success());
    fixture.make_all_due().await;
    fixture
        .wait_until("the late object is confirmed", "SELECT count(*) FROM {infra}.aip_object_ledger WHERE state='COMPLETE' AND NOT settling", 1)
        .await;
    assert_eq!((fixture.number(BYTES).await, fixture.number(ROWS).await), (3 * MIB as i64, 1), "the object exists and the ledger counts it");
}

/// KNOWN LIMIT, pinned on purpose: after the second look the reservation is released. An upload that is still running even then lands
/// in the bucket without a ledger row, so the owner quota does not cover it. The quota is a bound on signed-for and confirmed bytes, not a
/// hard cap on what an owner can leave in the bucket; the bucket's own lifecycle and capacity controls have to cover the rest
/// (INFRASTRUCTURE.md). If the ledger ever learns to adopt such objects, this test should fail and be rewritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-minio-object-storage.sh with the TLS MinIO fixture"]
async fn live_known_limit_an_upload_finishing_after_the_hold_ended_is_not_counted() {
    let (store, client) = live_store(10 * MIB, 10);
    let fixture = Fixture::start(store).await;
    let signed = fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await;
    assert_eq!(signed["ok"], true, "{signed}");
    assert_eq!(fixture.number(BYTES).await, 3 * MIB as i64);

    fixture.make_all_due().await;
    fixture.wait_until("the unfinished upload is held", HELD, 1).await;
    fixture.make_all_due().await;
    fixture.wait_until("the hold ended and the reservation is released", ROWS, 0).await;

    assert!(put_to(&client, &signed, vec![3; (3 * MIB) as usize]).await.is_success());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(fixture.number(BYTES).await, 0, "the object exists in the bucket but the ledger does not count it");
    assert_eq!(fixture.number(ROWS).await, 0);
    let confirmed = fixture.control("alice", "/objects/confirm", json!({"id":signed["id"]})).await;
    assert_eq!(confirmed["ok"], true, "{confirmed}");
    assert_eq!(confirmed["size"], 3 * MIB);
    assert_eq!(fixture.number(BYTES).await, 3 * MIB as i64, "explicit provider confirmation adopts the late object");
}

/// KNOWN LIMIT, pinned on purpose: only objects the ledger has seen are counted. An object that was already in the bucket before the ledger
/// existed (for example uploaded by an earlier release) has no row, so it never counts toward the owner quota and the owner can sign a full
/// quota on top of it. Forced here by removing the row of an uploaded object, which is exactly the state such an object has.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/test-minio-object-storage.sh with the TLS MinIO fixture"]
async fn live_known_limit_an_object_that_predates_the_ledger_is_not_counted() {
    let (store, client) = live_store(10 * MIB, 10);
    let fixture = Fixture::start(store).await;
    let signed = fixture.control("alice", "/objects/sign", put_request(3 * MIB)).await;
    assert_eq!(signed["ok"], true, "{signed}");
    assert!(put_to(&client, &signed, vec![5; (3 * MIB) as usize]).await.is_success());
    fixture.make_all_due().await;
    fixture.wait_until("the uploaded object is confirmed at its real size", BYTES, 3 * MIB as i64).await;

    fixture.db.execute(&format!("DELETE FROM {}.aip_object_ledger", fixture.infra), &[]).await.unwrap();
    assert_eq!(fixture.number(BYTES).await, 0, "the object is in the bucket but has no ledger row");
    let second = fixture.control("alice", "/objects/sign", put_request(10 * MIB)).await;
    assert_eq!(second["ok"], true, "a full quota can be signed on top of the uncounted object: {second}");
    assert_eq!(fixture.number(BYTES).await, 10 * MIB as i64);
    let id = signed["id"].as_str().unwrap();
    let other = fixture.control("bob", "/objects/confirm", json!({"id":id})).await;
    assert_eq!(other["code"], "WIRE.NOT_FOUND", "HEAD must use the authenticated owner's prefix: {other}");
    let confirmed = fixture.control("alice", "/objects/confirm", json!({"id":id})).await;
    assert_eq!(confirmed["ok"], true, "actual existing bytes are recorded even above quota: {confirmed}");
    assert_eq!(fixture.number(BYTES).await, 13 * MIB as i64);
    assert_eq!(fixture.control("alice", "/objects/confirm", json!({"id":id})).await["ok"], true);
    assert_eq!(fixture.number(BYTES).await, 13 * MIB as i64, "a repeated confirmation does not double count");
    let over = fixture.control("alice", "/objects/sign", put_request(1)).await;
    assert_eq!(over["code"], "WIRE.QUOTA_EXCEEDED");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirm_rejects_caller_sizes_and_invalid_keys_before_provider_io() {
    let fixture = Fixture::start(offline_store(10 * MIB, 3)).await;
    for body in [json!({}), json!({"id":"bad"}), json!({"id":"00000000-0000-4000-8000-000000000001","size":1})] {
        let response = fixture.control("alice", "/objects/confirm", body).await;
        assert_eq!(response["code"], "WIRE.BAD_REQUEST", "{response}");
        assert_eq!(fixture.number(ROWS).await, 0);
    }
}
