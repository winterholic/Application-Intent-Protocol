use aip_execution::files::Store;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Db {
    client: Client,
    schema: String,
}

impl Db {
    async fn new() -> Self {
        let schema = format!("aip_files_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let (client, connection) = tokio_postgres::connect(DB, NoTls).await.expect("connect to local test PostgreSQL");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        let store = Store::new(&schema).unwrap();
        client.batch_execute(&store.ddl()).await.unwrap();
        Self { client, schema }
    }

    fn store(&self) -> Store {
        Store::new(&self.schema).unwrap()
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let schema = self.schema.clone();
        std::thread::spawn(move || {
            let _ =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
        })
        .join()
        .unwrap();
    }
}

#[tokio::test]
async fn managed_file_round_trip_uses_opaque_handle_and_owner_scope() {
    let db = Db::new().await;
    let store = db.store();
    let owner = "issuer-a|subject-1";
    let file = store.put(&db.client, owner, "reports", "monthly.csv", "text/csv", b"a,b\n1,2\n", None).await.unwrap();
    let retry = store.put(&db.client, owner, "reports", "monthly.csv", "text/csv", b"a,b\n1,2\n", None).await.unwrap();

    assert_eq!(retry.handle, file.handle);
    assert_eq!(store.get(&db.client, owner, &file.handle).await.unwrap(), Some(file.clone()));
    assert!(store.put(&db.client, owner, "reports", "monthly.csv", "text/csv", b"different", None).await.is_err());
    assert_eq!(store.get(&db.client, "issuer-a|subject-2", &file.handle).await.unwrap(), None);
    assert!(!store.delete(&db.client, "issuer-a|subject-2", &file.handle).await.unwrap());
    assert_eq!(store.get(&db.client, owner, &file.handle).await.unwrap(), Some(file.clone()));
    assert!(store.delete(&db.client, owner, &file.handle).await.unwrap());
    assert_eq!(store.get(&db.client, owner, &file.handle).await.unwrap(), None);
}

#[tokio::test]
async fn managed_file_rejects_oversize_and_invalid_metadata() {
    let db = Db::new().await;
    let store = db.store();

    assert!(store.put(&db.client, "owner", "reports", "large.bin", "application/octet-stream", &vec![0; 1_048_577], None).await.is_err());
    assert!(store.put(&db.client, "owner", "../outside", "x.csv", "text/csv", b"x", None).await.is_err());
    assert!(store.put(&db.client, "owner", "reports", "x.csv", "text/csv\r\nX-Evil: yes", b"x", None).await.is_err());
}

#[tokio::test]
async fn expired_managed_file_is_not_returned_or_downloadable() {
    let db = Db::new().await;
    let store = db.store();
    let expires_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 + 1;
    let file = store.put(&db.client, "owner", "reports", "expired.csv", "text/csv", b"secret", Some(expires_at)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    assert_eq!(store.get(&db.client, "owner", &file.handle).await.unwrap(), None);
    assert!(store.delete(&db.client, "owner", &file.handle).await.unwrap());
}

#[tokio::test]
async fn managed_file_writes_can_share_the_caller_transaction() {
    let mut db = Db::new().await;
    let store = db.store();
    let tx = db.client.transaction().await.unwrap();
    let file = store.put(&tx, "owner", "reports", "atomic.csv", "text/csv", b"x", None).await.unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(store.get(&db.client, "owner", &file.handle).await.unwrap(), None);
}

#[tokio::test]
async fn separate_runs_keep_the_same_display_name_without_conflict() {
    let db = Db::new().await;
    let store = db.store();
    let first = store.put_scoped(&db.client, "owner", "reports", "report.txt", "text/plain", b"first", None, "run-a").await.unwrap();
    let second = store.put_scoped(&db.client, "owner", "reports", "report.txt", "text/plain", b"second", None, "run-b").await.unwrap();
    assert_ne!(first.handle, second.handle);
    assert_eq!(first.name, second.name);
    let replay = store.put_scoped(&db.client, "owner", "reports", "report.txt", "text/plain", b"first", None, "run-a").await.unwrap();
    assert_eq!(replay.handle, first.handle);
    assert!(store.put_scoped(&db.client, "owner", "reports", "report.txt", "text/plain", b"changed", None, "run-a").await.is_err());
}
