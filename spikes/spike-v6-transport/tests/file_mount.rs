use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    hosting::ManagedFileMount,
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

static SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::Builder::new().name("aip-file-mount-cleanup".into()).spawn(move || {
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

struct Response {
    status: String,
    headers: String,
    body: Vec<u8>,
}
async fn request(addr: SocketAddr, method: &str, path: &str, token: &str, fingerprint: &str, body: &str) -> Response {
    request_with_origin(addr, method, path, token, fingerprint, body, None).await
}

async fn request_with_origin(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    fingerprint: &str,
    body: &str,
    origin: Option<&str>,
) -> Response {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let contract_header = if fingerprint.is_empty() { String::new() } else { format!("x-aip-contract: {fingerprint}\r\n") };
    let origin_header = origin.map(|origin| format!("origin: {origin}\r\n")).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {token}\r\n{origin_header}{contract_header}content-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut output)).await.unwrap().unwrap();
    let split = output.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(output[..split].to_vec()).unwrap();
    Response { status: head.lines().next().unwrap().to_owned(), headers: head.to_ascii_lowercase(), body: output[split + 4..].to_vec() }
}

#[test]
fn managed_file_mount_requires_a_safe_prefix_and_bucket_and_does_not_publish_host_paths() {
    assert!(ManagedFileMount::new("/reports".into(), "reports".into()).is_ok());
    for prefix in ["/", "/reports/", "/reports/%2e%2e"] {
        assert!(ManagedFileMount::new(prefix.into(), "reports".into()).is_err(), "{prefix}");
    }
    for bucket in ["", "../reports", "reports/private", "reports%2fprivate"] {
        assert!(ManagedFileMount::new("/reports".into(), bucket.into()).is_err(), "{bucket}");
    }
    let mut infrastructure = Infrastructure::default();
    infrastructure.file_mounts.push(ManagedFileMount::new("/reports".into(), "reports".into()).unwrap());
    let catalog = infrastructure.catalog();
    assert_eq!(catalog["fileMounts"][0]["prefix"], "/reports");
    assert_eq!(catalog["fileMounts"][0]["bucket"], "reports");
    assert!(!catalog.to_string().contains("directory"));
    assert!(infrastructure.validate().is_ok());
    let mut reserved = Infrastructure::default();
    reserved.file_mounts.push(ManagedFileMount::new("/files".into(), "reports".into()).unwrap());
    assert!(reserved.validate().is_err());
    let mut nested_reserved = Infrastructure::default();
    nested_reserved.file_mounts.push(ManagedFileMount::new("/files/private".into(), "reports".into()).unwrap());
    assert!(nested_reserved.validate().is_err());

    let mut overlapping = Infrastructure::default();
    overlapping.file_mounts.push(ManagedFileMount::new("/reports".into(), "reports".into()).unwrap());
    overlapping.file_mounts.push(ManagedFileMount::new("/reports/archive".into(), "archive".into()).unwrap());
    assert!(overlapping.validate().is_err());

    let mut static_overlap = Infrastructure::default();
    static_overlap.mounts.push(
        spike_v6_transport::hosting::StaticMount::new("/reports".into(), std::env::temp_dir(), spike_v6_transport::hosting::MountAccess::Public)
            .unwrap(),
    );
    static_overlap.file_mounts.push(ManagedFileMount::new("/reports/downloads".into(), "reports".into()).unwrap());
    assert!(static_overlap.validate().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn managed_file_mount_serves_owner_bucket_bytes_with_private_headers_and_rechecks_revocation() {
    let _schema_lock = SCHEMA_LOCK.lock().await;
    let schema = format!("aip_file_mount_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','owner',1),('test','other',1)")).await.unwrap();
    let iat: i64 = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let auth = Arc::new(Auth { schema: schema.clone(), iat });
    let digest = spike_v1_fixture::digest(&facts);
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    let infrastructure = Infrastructure {
        file_mounts: vec![
            ManagedFileMount::new("/reports".into(), "reports".into()).unwrap(),
            ManagedFileMount::new("/private".into(), "private".into()).unwrap(),
        ],
        ..Infrastructure::default()
    };
    let server = listen_with_infrastructure(
        facts,
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions { allowed_origins: vec!["https://app.example.com".into()] },
        false,
        auth,
        Some(DeploymentFence::new(schema.clone(), digest).unwrap()),
        None,
        true,
        infrastructure,
    )
    .await
    .unwrap();
    let uploaded = request(
        server.address,
        "POST",
        "/files/put",
        "owner",
        &fingerprint,
        r#"{"bucket":"reports","name":"report.txt","mediaType":"text/plain","base64":"aGVsbG8g8J+YgA=="}"#,
    )
    .await;
    let uploaded: Value = serde_json::from_slice(&uploaded.body).unwrap();
    assert_eq!(uploaded["ok"], true);
    let id = uploaded["file"]["id"].as_str().unwrap();

    let get = request(server.address, "GET", &format!("/reports/{id}"), "owner", &fingerprint, "").await;
    assert_eq!(get.status, "HTTP/1.1 200 OK");
    assert_eq!(get.body, "hello 😀".as_bytes());
    assert!(get.headers.contains("content-type: text/plain"));
    assert!(get.headers.contains(&format!("content-disposition: attachment; filename=\"{id}\"")));
    assert!(get.headers.contains("cache-control: private, no-store"));
    assert!(get.headers.contains("x-content-type-options: nosniff"));
    assert!(get.headers.contains("\r\nx-request-id: req_"), "a managed file reply is traceable: {}", get.headers);
    let cors_get =
        request_with_origin(server.address, "GET", &format!("/reports/{id}"), "owner", &fingerprint, "", Some("https://app.example.com")).await;
    assert!(cors_get.headers.contains("access-control-allow-origin: https://app.example.com"));
    assert!(cors_get.headers.contains("access-control-expose-headers: x-request-id"), "{}", cors_get.headers);
    let rejected_origin =
        request_with_origin(server.address, "GET", &format!("/reports/{id}"), "owner", &fingerprint, "", Some("https://evil.example")).await;
    assert_eq!(serde_json::from_slice::<Value>(&rejected_origin.body).unwrap()["code"], "ORIGIN_NOT_ALLOWED");
    assert!(rejected_origin.headers.contains("cache-control: private, no-store"));

    let head = request(server.address, "HEAD", &format!("/reports/{id}"), "owner", &fingerprint, "").await;
    assert_eq!(head.status, "HTTP/1.1 200 OK");
    assert!(head.body.is_empty());
    assert!(head.headers.contains(&format!("content-length: {}", "hello 😀".len())));

    let missing_contract = request(server.address, "GET", &format!("/reports/{id}"), "owner", "", "").await;
    assert_eq!(serde_json::from_slice::<Value>(&missing_contract.body).unwrap()["code"], "CONTRACT_MISMATCH");
    let missing_contract_head = request(server.address, "HEAD", &format!("/reports/{id}"), "owner", "", "").await;
    assert!(missing_contract_head.body.is_empty(), "HEAD errors must not include a response body");
    assert!(missing_contract_head.headers.contains("content-length: "));

    let invalid_id = request(server.address, "GET", "/reports/not-a-uuid", "owner", &fingerprint, "").await;
    assert_eq!(serde_json::from_slice::<Value>(&invalid_id.body).unwrap()["code"], "NOT_FOUND");

    let wrong_bucket = request(server.address, "GET", &format!("/private/{id}"), "owner", &fingerprint, "").await;
    assert_eq!(wrong_bucket.status, "HTTP/1.1 200 OK");
    assert_eq!(serde_json::from_slice::<Value>(&wrong_bucket.body).unwrap()["code"], "NOT_EXPOSED");
    let other_owner = request(server.address, "GET", &format!("/reports/{id}"), "other", &fingerprint, "").await;
    assert_eq!(serde_json::from_slice::<Value>(&other_owner.body).unwrap()["code"], "NOT_FOUND");

    db.execute(&format!("UPDATE {schema}.aip_principals SET enabled=false WHERE issuer='test' AND subject='owner'"), &[]).await.unwrap();
    let revoked = request(server.address, "GET", &format!("/reports/{id}"), "owner", &fingerprint, "").await;
    assert_eq!(serde_json::from_slice::<Value>(&revoked.body).unwrap()["code"], "UNAUTHENTICATED");
    db.execute(&format!("UPDATE {schema}.aip_principals SET enabled=true WHERE issuer='test' AND subject='owner'"), &[]).await.unwrap();
    db.execute(&format!("UPDATE {schema}.aip_migrate_meta SET facts_digest=repeat('0',64) WHERE singleton=true"), &[]).await.unwrap();
    let changed = request(server.address, "GET", &format!("/reports/{id}"), "owner", &fingerprint, "").await;
    assert_eq!(serde_json::from_slice::<Value>(&changed.body).unwrap()["code"], "DEPLOYMENT_CHANGED");
    server.shutdown().await.unwrap();
}
