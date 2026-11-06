//! Independent WBS 8.20 boundary probe. Runs only in the TLS MinIO fixture.
use aip_execution::{
    object_ledger::{ObjectLedger, Outcome, ScanPageReport, ScanPolicy, State},
    objects::{ObjectKey, S3Config, S3Store},
};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

async fn put(client: &reqwest::Client, store: &S3Store, key: &ObjectKey, bytes: &[u8]) {
    let url = store.presign_put(key, "application/octet-stream", 600).unwrap();
    let response = client.put(url).header("content-type", "application/octet-stream").body(bytes.to_vec()).send().await.unwrap();
    assert!(response.status().is_success(), "MinIO PUT status: {}", response.status());
}

async fn db() -> (Client, String, ObjectLedger) {
    let schema = format!("scan_verify_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let (client, connection) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
    client.batch_execute(&ObjectLedger::ddl(&schema).unwrap()).await.unwrap();
    let ledger = ObjectLedger::new(&schema, 1024 * 1024, 100).unwrap();
    (client, schema, ledger)
}

async fn due_owner(client: &Client, schema: &str) {
    client
        .batch_execute(&format!("UPDATE {schema}.aip_object_owners SET next_scan_at=clock_timestamp()-interval '1 second',lease_until=NULL"))
        .await
        .unwrap();
}

async fn row(client: &Client, schema: &str, owner: &str, key: &ObjectKey) -> Option<(String, i64, i64)> {
    client
        .query_opt(&format!("SELECT state,bytes,version FROM {schema}.aip_object_ledger WHERE owner=$1 AND object_id=$2"), &[&owner, &key.as_str()])
        .await
        .unwrap()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run through scripts/test-minio-object-storage.sh with PostgreSQL and TLS MinIO"]
async fn pg_minio_owner_mapping_paging_fencing_and_multipart_boundary() {
    let endpoint = std::env::var("AIP_MINIO_ENDPOINT").unwrap();
    let bucket = std::env::var("AIP_MINIO_BUCKET").unwrap();
    let ca = std::fs::read(std::env::var("AIP_MINIO_CA_PEM").unwrap()).unwrap();
    let store = S3Store::new(
        S3Config::new(endpoint, bucket, "us-east-1", std::env::var("AIP_MINIO_ACCESS_KEY").unwrap(), std::env::var("AIP_MINIO_SECRET_KEY").unwrap())
            .with_root_certificate_pem(&ca),
    )
    .unwrap();
    let http =
        reqwest::Client::builder().https_only(true).no_proxy().add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap()).build().unwrap();
    let (mut client, schema, ledger) = db().await;

    // Real S3-compatible listing pages feed real PostgreSQL transactions. One request per tick must
    // resume its persisted continuation token until all three objects have been adopted.
    let owner = format!("scan-verify-{}", uuid::Uuid::new_v4());
    let scoped = store.for_owner(&owner).unwrap();
    let keys = [ObjectKey::generate(), ObjectKey::generate(), ObjectKey::generate()];
    for (index, key) in keys.iter().enumerate() {
        put(&http, &scoped, key, &vec![index as u8 + 1; index + 1]).await;
    }
    let prefix = ledger.register_owner(&mut client, &owner).await.unwrap();
    assert_eq!(ledger.owner_for_prefix(&client, &prefix).await.unwrap().as_deref(), Some(owner.as_str()));
    let policy = ScanPolicy { page_keys: 1, max_pages: 1, lease_secs: 60, rescan_secs: 3600, resume_secs: 1 };
    let mut calls = 0;
    for expected in 1..=3 {
        if expected > 1 {
            due_owner(&client, &schema).await;
        }
        let listing = scoped.clone();
        let tick = ledger
            .scan_tick(&mut client, &policy, move |target, token, max_keys| {
                let listing = listing.clone();
                async move { listing.list_objects_v2(max_keys, token.as_deref()).await }
            })
            .await
            .unwrap();
        calls += tick.pages;
        assert_eq!(tick.pages, 1, "tick {expected} is capped at one billed page");
        if expected < 3 {
            assert!(ledger.owner_for_prefix(&client, &prefix).await.unwrap().is_some());
            let token: Option<String> = client
                .query_one(&format!("SELECT continuation_token FROM {schema}.aip_object_owners WHERE owner=$1"), &[&owner])
                .await
                .unwrap()
                .get(0);
            assert!(token.is_some(), "page {expected} stores a resumable cursor");
        }
    }
    assert_eq!(calls, 3, "three pages required for three keys with max-keys=1");
    assert_eq!(ledger.usage(&client, &owner).await.unwrap().objects, 3);

    // A matching observation leaves version intact, preserving any concurrent claim.
    let snapshot = ledger.snapshot_owner_versions(&client, &owner).await.unwrap();
    let page = scoped.list_objects_v2(100, None).await.unwrap();
    let items: Vec<_> = page.items.iter().map(|(key, size)| (key.as_str(), *size)).collect();
    let before = row(&client, &schema, &owner, &page.items[0].0).await.unwrap().2;
    let applied = ledger.apply_scan_page(&mut client, &owner, &snapshot, &items, None, 3600, 1).await.unwrap();
    assert_eq!(applied.unchanged, 3);
    assert_eq!(row(&client, &schema, &owner, &page.items[0].0).await.unwrap().2, before);

    // Capture a real MinIO listing while DELETING, remove the object, resolve the PG row, then
    // apply that stale provider response: the deleted key must not be resurrected.
    let gone = page.items[0].0.clone();
    ledger.delete_signed(&mut client, &owner, &gone.as_str(), 600).await.unwrap();
    let stale_versions = ledger.snapshot_owner_versions(&client, &owner).await.unwrap();
    assert_eq!(stale_versions[&gone.as_str()].0, State::Deleting);
    let stale_page = scoped.list_objects_v2(100, None).await.unwrap();
    let delete_url = scoped.presign_delete(&gone, 600).unwrap();
    assert!(http.delete(delete_url).send().await.unwrap().status().is_success());
    client
        .batch_execute(&format!(
            "UPDATE {schema}.aip_object_ledger SET expires_at=clock_timestamp()-interval '1 second' WHERE owner='{}' AND object_id='{}'",
            owner,
            gone.as_str()
        ))
        .await
        .unwrap();
    let due = ledger.claim_due(&mut client, 10, 60).await.unwrap();
    let deletion = due.into_iter().find(|item| item.object_id == gone.as_str()).unwrap();
    assert!(ledger.resolve(&mut client, &deletion, Outcome::Absent).await.unwrap());
    let stale_items: Vec<_> = stale_page.items.iter().filter(|(key, _)| key == &gone).map(|(key, size)| (key.as_str(), *size)).collect();
    let report = ledger.apply_scan_page(&mut client, &owner, &stale_versions, &stale_items, None, 3600, 1).await.unwrap();
    assert_eq!(report.skipped_fenced, 1);
    assert!(row(&client, &schema, &owner, &gone).await.is_none(), "stale listing did not revive deleted row");
    assert_eq!(
        ledger.owner_for_prefix(&client, &prefix).await.unwrap().as_deref(),
        Some(owner.as_str()),
        "owner mapping survives deletion of a ledger row"
    );

    // A pre-existing object with the same key as an active multipart upload is skipped while a
    // separate late object in the same MinIO page is adopted.
    let multipart_owner = format!("scan-multipart-{}", uuid::Uuid::new_v4());
    let multipart_store = store.for_owner(&multipart_owner).unwrap();
    let upload = multipart_store.create_multipart("application/octet-stream").await.unwrap();
    put(&http, &multipart_store, &upload.key, b"old-object").await;
    ledger.start_multipart(&mut client, &multipart_owner, &upload.key.as_str(), &upload.upload_id, 600).await.unwrap();
    ledger.reserve_part(&mut client, &multipart_owner, &upload.key.as_str(), &upload.upload_id, 1, 99, 600).await.unwrap();
    let late = ObjectKey::generate();
    put(&http, &multipart_store, &late, b"late").await;
    let multipart_before = ledger.snapshot_owner_versions(&client, &multipart_owner).await.unwrap();
    let multipart_page = multipart_store.list_objects_v2(100, None).await.unwrap();
    let multipart_items: Vec<_> = multipart_page.items.iter().map(|(key, size)| (key.as_str(), *size)).collect();
    let report = ledger.apply_scan_page(&mut client, &multipart_owner, &multipart_before, &multipart_items, None, 3600, 1).await.unwrap();
    assert_eq!(report, ScanPageReport { adopted: 1, skipped_multipart: 1, ..Default::default() });
    assert_eq!(row(&client, &schema, &multipart_owner, &upload.key).await.unwrap().1, 99);
    assert_eq!(row(&client, &schema, &multipart_owner, &late).await.unwrap().1, 4);
    ledger.require_open_multipart(&mut client, &multipart_owner, &upload.key.as_str(), &upload.upload_id).await.unwrap();
    multipart_store.abort_multipart(&upload).await.unwrap();

    client.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
