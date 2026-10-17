use aip_execution::events::Store;
use serde_json::json;
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
        let schema = format!("aip_events_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
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

    async fn second_client() -> Client {
        let (client, connection) = tokio_postgres::connect(DB, NoTls).await.expect("second local PostgreSQL connection");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
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
async fn publish_is_idempotent_but_same_key_with_different_input_conflicts() {
    let mut db = Db::new().await;
    let store = db.store();
    let payload = json!({"type":"invoice.paid","invoice":"i-1"});

    let first = store.publish(&mut db.client, "owner", "billing", "evt-1", payload.clone()).await.unwrap();
    let retry = store.publish(&mut db.client, "owner", "billing", "evt-1", payload).await.unwrap();
    assert_eq!(first, retry);
    assert!(store.publish(&mut db.client, "owner", "billing", "evt-1", json!({"invoice":"i-2"})).await.is_err());
}

#[tokio::test]
async fn scan_returns_bounded_replayable_cursors_scoped_to_owner_and_stream() {
    let mut db = Db::new().await;
    let store = db.store();
    let first = store.publish(&mut db.client, "owner-a", "billing", "k1", json!({"n":1})).await.unwrap();
    let second = store.publish(&mut db.client, "owner-a", "billing", "k2", json!({"n":2})).await.unwrap();
    store.publish(&mut db.client, "owner-a", "support", "k3", json!({"n":3})).await.unwrap();
    store.publish(&mut db.client, "owner-b", "billing", "k4", json!({"n":4})).await.unwrap();

    let page = store.scan(&db.client, "owner-a", "billing", "0", 1).await.unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].cursor, first);
    let replay = store.scan(&db.client, "owner-a", "billing", "0", 1).await.unwrap();
    assert_eq!(page, replay);
    let next = store.scan(&db.client, "owner-a", "billing", &page.next_cursor, 10).await.unwrap();
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0].cursor, second);
    assert!(store.scan(&db.client, "owner-a", "billing", "0", 0).await.is_err());
}

#[tokio::test]
async fn retention_marks_old_cursor_expired_and_preserves_new_replay() {
    let mut db = Db::new().await;
    let store = db.store();
    let first = store.publish(&mut db.client, "owner", "orders", "k1", json!({"n":1})).await.unwrap();
    let second = store.publish(&mut db.client, "owner", "orders", "k2", json!({"n":2})).await.unwrap();
    let third = store.publish(&mut db.client, "owner", "orders", "k3", json!({"n":3})).await.unwrap();

    store.prune(&mut db.client, "owner", "orders", &second).await.unwrap();
    assert!(store.scan(&db.client, "owner", "orders", &first, 10).await.is_err());
    let page = store.scan(&db.client, "owner", "orders", &second, 10).await.unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].cursor, third);
    assert_eq!(store.publish(&mut db.client, "owner", "orders", "k1", json!({"n":1})).await.unwrap(), first);
    assert!(store.publish(&mut db.client, "owner", "orders", "k1", json!({"n":99})).await.is_err());
}

#[tokio::test]
async fn scan_racing_retention_is_complete_or_explicitly_expired() {
    let mut db = Db::new().await;
    let store = db.store();
    for n in 0..3 {
        store.publish(&mut db.client, "owner", "orders", &format!("k{n}"), json!({"n":n})).await.unwrap();
    }
    let mut other = Db::second_client().await;

    let (scan, prune) = tokio::join!(store.scan(&db.client, "owner", "orders", "0", 10), store.prune(&mut other, "owner", "orders", "2"));
    prune.unwrap();
    match scan {
        Ok(page) => assert_eq!(page.events.len(), 3),
        Err(aip_execution::events::Error::CursorExpired) => {}
        Err(error) => panic!("unexpected scan result: {error:?}"),
    }
}

#[tokio::test]
async fn invalid_owner_stream_key_and_cursor_are_rejected() {
    let mut db = Db::new().await;
    let store = db.store();

    assert!(store.publish(&mut db.client, "", "orders", "k", json!({})).await.is_err());
    assert!(store.publish(&mut db.client, "owner", "../orders", "k", json!({})).await.is_err());
    assert!(store.publish(&mut db.client, "owner", "orders", "", json!({})).await.is_err());
    assert!(store.scan(&db.client, "owner", "orders", "-1", 10).await.is_err());
    assert!(store.scan(&db.client, "owner", "orders", "not-a-cursor", 10).await.is_err());
    assert!(store.scan(&db.client, "owner", "orders", "1", 10).await.is_err());
}

#[tokio::test]
async fn publish_in_uses_the_callers_transaction() {
    let mut db = Db::new().await;
    let store = db.store();
    let tx = db.client.transaction().await.unwrap();
    let cursor = store.publish_in(&tx, "owner", "orders", "tx-event", json!({"n":1})).await.unwrap();
    tx.rollback().await.unwrap();

    let page = store.scan(&db.client, "owner", "orders", "0", 10).await.unwrap();
    assert!(page.events.is_empty());
    assert_eq!(cursor, "1");
}

#[tokio::test]
async fn payload_is_limited_to_64_kibibytes() {
    let mut db = Db::new().await;
    let store = db.store();
    let too_large = json!("x".repeat(64 * 1024));

    assert!(store.publish(&mut db.client, "owner", "notices", "large", too_large).await.is_err());
}

#[tokio::test]
async fn scan_pages_are_bounded_by_total_payload_bytes() {
    let mut db = Db::new().await;
    let store = db.store();
    let payload = json!("x".repeat(60 * 1024));
    for index in 0..18 {
        store.publish(&mut db.client, "owner", "large-notices", &format!("k{index}"), payload.clone()).await.unwrap();
    }

    let page = store.scan(&db.client, "owner", "large-notices", "0", 1000).await.unwrap();
    let returned_bytes: usize = page.events.iter().map(|event| serde_json::to_vec(&event.payload).unwrap().len()).sum();
    assert!(returned_bytes <= 1024 * 1024);
    assert!(page.events.len() < 18);
    let next = store.scan(&db.client, "owner", "large-notices", &page.next_cursor, 1000).await.unwrap();
    assert_eq!(page.events.len() + next.events.len(), 18);
}

#[tokio::test]
async fn automatic_retention_keeps_last_thousand_and_preserves_idempotency_ledger() {
    let mut db = Db::new().await;
    let store = db.store();
    for index in 0..1001 {
        store.publish(&mut db.client, "owner", "busy", &format!("k{index}"), json!({"n":index})).await.unwrap();
    }

    assert!(matches!(store.scan(&db.client, "owner", "busy", "0", 10).await, Err(aip_execution::events::Error::CursorExpired)));
    let page = store.scan(&db.client, "owner", "busy", "1", 10).await.unwrap();
    assert_eq!(page.events[0].cursor, "2");
    assert_eq!(store.publish(&mut db.client, "owner", "busy", "k0", json!({"n":0})).await.unwrap(), "1");
    assert!(store.publish(&mut db.client, "owner", "busy", "k0", json!({"n":999})).await.is_err());
}

/// O-20: the idempotency ledger used to grow forever, one row per event ever published, because only the replay
/// payloads were trimmed. Keys now stay for a bounded window of cursors beyond the replay window.
#[tokio::test]
async fn idempotency_ledger_is_bounded_beyond_the_replay_window() {
    let mut db = Db::new().await;
    let store = db.store().with_ledger_window(100);
    for index in 0..1300 {
        store.publish(&mut db.client, "owner", "busy", &format!("k{index}"), json!({"n":index})).await.unwrap();
    }
    let keys: i64 = db.client.query_one(&format!("SELECT count(*) FROM {}.aip_event_keys", db.schema), &[]).await.unwrap().get(0);
    // 1000 replayable events + 100 extra cursors + at most one prune interval of lag.
    assert!(keys <= 1000 + 100 + 100, "ledger kept {keys} keys");
    // Recent keys, including ones that already left the replay window, still deduplicate.
    assert_eq!(store.publish(&mut db.client, "owner", "busy", "k299", json!({"n":299})).await.unwrap(), "300");
    assert!(store.publish(&mut db.client, "owner", "busy", "k299", json!({"n":1})).await.is_err());
    // Another stream's ledger is untouched.
    store.publish(&mut db.client, "owner", "quiet", "only", json!({})).await.unwrap();
    assert_eq!(store.publish(&mut db.client, "owner", "quiet", "only", json!({})).await.unwrap(), "1");
}
