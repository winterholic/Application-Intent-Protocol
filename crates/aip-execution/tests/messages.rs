use aip_execution::messages::{Error, MessageStore, NewMessage, ddl};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
const OWNER: &str = "https://issuer.example|subject-1";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Db {
    client: Client,
    schema: String,
}

impl Db {
    async fn new() -> Self {
        let schema = format!("aip_messages_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let (client, connection) = tokio_postgres::connect(DB, NoTls).await.expect("connect to local test PostgreSQL");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        client.batch_execute(&ddl(&schema).unwrap()).await.unwrap();
        Self { client, schema }
    }

    fn store(&self) -> MessageStore {
        MessageStore::new(&self.schema).unwrap()
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

fn message(queue: &str, key: &str, payload: serde_json::Value) -> NewMessage {
    NewMessage { owner: OWNER.into(), queue: queue.into(), key: key.into(), payload, max_deliveries: 3, ttl_ms: 60_000, delay_ms: 0 }
}

#[tokio::test]
async fn enqueue_is_owner_and_queue_scoped_and_idempotent_by_payload() {
    let db = Db::new().await;
    let store = db.store();
    let first = store.enqueue(&db.client, message("billing", "k1", json!({"invoice":"i-1"}))).await.unwrap();
    let replay = store.enqueue(&db.client, message("billing", "k1", json!({"invoice":"i-1"}))).await.unwrap();
    assert_eq!(first.id, replay.id);
    assert_eq!(store.enqueue(&db.client, message("billing", "k1", json!({"invoice":"i-2"}))).await.unwrap_err(), Error::IdempotencyConflict);

    let other_queue = store.enqueue(&db.client, message("support", "k1", json!({"invoice":"i-1"}))).await.unwrap();
    let mut other_owner = message("billing", "k1", json!({"invoice":"i-1"}));
    other_owner.owner = "https://issuer.example|subject-2".into();
    let other_tenant = store.enqueue(&db.client, other_owner).await.unwrap();
    assert_ne!(first.id, other_queue.id);
    assert_ne!(first.id, other_tenant.id);
}

#[tokio::test]
async fn claims_are_exclusive_and_expired_lease_rejects_stale_ack() {
    let db = Db::new().await;
    let store = db.store();
    let msg = store.enqueue(&db.client, message("jobs", "k1", json!({"n":1}))).await.unwrap();
    let (left, right) =
        tokio::join!(store.claim(&db.client, OWNER, "jobs", "worker-a", 250), store.claim(&db.client, OWNER, "jobs", "worker-b", 250));
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.is_some() as usize + right.is_some() as usize, 1);
    let lease = left.or(right).unwrap();
    let encoded = serde_json::to_string(&lease).unwrap();
    let decoded: aip_execution::messages::MessageLease = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, lease);

    db.client
        .execute(
            &format!("UPDATE {}.aip_messages SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&lease.id).unwrap()],
        )
        .await
        .unwrap();
    let replacement = store.claim(&db.client, OWNER, "jobs", "worker-c", 250).await.unwrap().unwrap();
    assert_eq!(replacement.id, msg.id);
    assert!(replacement.epoch > lease.epoch);
    assert!(!store.ack(&db.client, &lease).await.unwrap());
    assert!(store.ack(&db.client, &replacement).await.unwrap());
}

#[tokio::test]
async fn skip_locked_claim_skips_a_row_locked_by_another_transaction() {
    let mut db = Db::new().await;
    let store = db.store();
    let first = store.enqueue(&db.client, message("jobs", "k1", json!({"n":1}))).await.unwrap();
    let second = store.enqueue(&db.client, message("jobs", "k2", json!({"n":2}))).await.unwrap();
    let tx = db.client.transaction().await.unwrap();
    tx.query_one(&format!("SELECT id FROM {}.aip_messages WHERE id=$1 FOR UPDATE", db.schema), &[&uuid::Uuid::parse_str(&first.id).unwrap()])
        .await
        .unwrap();

    let other = Db::second_client().await;
    let lease = store.claim(&other, OWNER, "jobs", "worker-b", 250).await.unwrap().unwrap();
    assert_eq!(lease.id, second.id);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn delayed_message_is_not_claimed_early() {
    let db = Db::new().await;
    let store = db.store();
    let mut delayed = message("jobs", "later", json!({"n":1}));
    delayed.delay_ms = 150;
    store.enqueue(&db.client, delayed).await.unwrap();
    assert!(store.claim(&db.client, OWNER, "jobs", "worker-a", 250).await.unwrap().is_none());
    tokio::time::sleep(std::time::Duration::from_millis(180)).await;
    assert!(store.claim(&db.client, OWNER, "jobs", "worker-a", 250).await.unwrap().is_some());
}

#[tokio::test]
async fn transport_outage_preserves_delivery_budget_and_rejects_stale_release() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = message("relay", "outage", json!({"valid":true}));
    input.max_deliveries = 1;
    let msg = store.enqueue(&db.client, input).await.unwrap();
    for _ in 0..5 {
        let lease =
            store.claim(&db.client, OWNER, "relay", "broker", 5_000).await.unwrap().expect("transport outages must not exhaust job deliveries");
        assert_eq!(lease.id, msg.id);
        assert_eq!(lease.deliveries, 1);
        assert!(store.retry_transport_after(&db.client, &lease, 0).await.unwrap());
        assert!(!store.retry_transport_after(&db.client, &lease, 0).await.unwrap());
    }
    assert!(store.dead_letters(&db.client, OWNER, "relay", 10).await.unwrap().is_empty());
    let lease = store.claim(&db.client, OWNER, "relay", "broker", 5_000).await.unwrap().unwrap();
    assert!(store.ack(&db.client, &lease).await.unwrap());
}

#[tokio::test]
async fn nack_uses_bounded_backoff_then_moves_message_to_dlq() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = message("jobs", "poison", json!({"broken":true}));
    input.max_deliveries = 2;
    let msg = store.enqueue(&db.client, input).await.unwrap();
    let first = store.claim(&db.client, OWNER, "jobs", "worker-a", 250).await.unwrap().unwrap();
    assert!(store.nack_after(&db.client, &first, "TEMPORARY", 0).await.unwrap());
    let second = store.claim(&db.client, OWNER, "jobs", "worker-b", 250).await.unwrap().unwrap();
    assert_eq!(second.deliveries, 2);
    assert!(store.nack_after(&db.client, &second, "POISON", 0).await.unwrap());

    let dead = store.dead_letters(&db.client, OWNER, "jobs", 10).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].id, msg.id);
    assert_eq!(dead[0].deliveries, 2);
    assert_eq!(dead[0].last_error.as_deref(), Some("POISON"));
    assert!(store.claim(&db.client, OWNER, "jobs", "worker-c", 250).await.unwrap().is_none());
}

#[tokio::test]
async fn lease_and_dead_letter_timestamps_use_rfc3339_utc() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = message("timestamps", "timestamp", json!({"n":1}));
    input.max_deliveries = 1;
    store.enqueue(&db.client, input).await.unwrap();
    let lease = store.claim(&db.client, OWNER, "timestamps", "worker-a", 250).await.unwrap().unwrap();
    assert_rfc3339_utc(&lease.lease_until);
    assert!(store.nack_after(&db.client, &lease, "POISON", 0).await.unwrap());
    let dead = store.dead_letters(&db.client, OWNER, "timestamps", 1).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_rfc3339_utc(&dead[0].created_at);
}

fn assert_rfc3339_utc(value: &str) {
    let bytes = value.as_bytes();
    assert_eq!(bytes.len(), 27, "expected RFC3339 UTC timestamp with microseconds: {value}");
    assert_eq!(bytes[4], b'-');
    assert_eq!(bytes[7], b'-');
    assert_eq!(bytes[10], b'T');
    assert_eq!(bytes[13], b':');
    assert_eq!(bytes[16], b':');
    assert_eq!(bytes[19], b'.');
    assert_eq!(bytes[26], b'Z');
    assert!(bytes.iter().enumerate().all(|(index, byte)| [4, 7, 10, 13, 16, 19, 26].contains(&index) || byte.is_ascii_digit()));
}

#[tokio::test]
async fn dead_letter_pages_bound_payload_bytes_and_continue_with_cursor() {
    let db = Db::new().await;
    let store = db.store();
    for index in 0..12 {
        let mut input = message("large", &format!("dead-{index}"), json!({"data":"x".repeat(100_000)}));
        input.max_deliveries = 1;
        store.enqueue(&db.client, input).await.unwrap();
        let lease = store.claim(&db.client, OWNER, "large", "worker-a", 250).await.unwrap().unwrap();
        assert!(store.nack_after(&db.client, &lease, "POISON", 0).await.unwrap());
    }

    let first = store.dead_letters_after(&db.client, OWNER, "large", None, 100).await.unwrap();
    let first_bytes = serde_json::to_vec(&first).unwrap().len();
    assert!(first_bytes <= 1 << 20);
    let cursor = first.next_cursor.expect("more dead letters should remain");
    let second = store.dead_letters_after(&db.client, OWNER, "large", Some(&cursor), 100).await.unwrap();
    assert_eq!(first.items.len() + second.items.len(), 12);
    assert!(second.next_cursor.is_none());
}

#[tokio::test]
async fn expired_claims_exhaust_delivery_limit_into_dead_letter() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = message("jobs", "crash", json!({"n":1}));
    input.max_deliveries = 1;
    store.enqueue(&db.client, input).await.unwrap();
    let lease = store.claim(&db.client, OWNER, "jobs", "worker-a", 250).await.unwrap().unwrap();
    db.client
        .execute(
            &format!("UPDATE {}.aip_messages SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&lease.id).unwrap()],
        )
        .await
        .unwrap();

    assert!(store.claim(&db.client, OWNER, "jobs", "worker-b", 250).await.unwrap().is_none());
    let dead = store.dead_letters(&db.client, OWNER, "jobs", 1).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].last_error.as_deref(), Some("LEASE_EXPIRED"));
    assert!(store.dead_letters(&db.client, "https://issuer.example|other", "jobs", 1).await.unwrap().is_empty());
}

#[tokio::test]
async fn dead_letter_can_be_explicitly_redriven() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = message("jobs", "repair", json!({"n":1}));
    input.max_deliveries = 1;
    let msg = store.enqueue(&db.client, input).await.unwrap();
    let lease = store.claim(&db.client, OWNER, "jobs", "worker-a", 250).await.unwrap().unwrap();
    assert!(store.nack_after(&db.client, &lease, "BAD_INPUT", 0).await.unwrap());
    assert!(store.redrive(&db.client, OWNER, "jobs", &msg.id, 3).await.unwrap());
    let lease = store.claim(&db.client, OWNER, "jobs", "worker-b", 250).await.unwrap().unwrap();
    assert_eq!(lease.deliveries, 1);
    assert!(store.ack(&db.client, &lease).await.unwrap());
}

#[tokio::test]
async fn owner_checks_scope_claim_ack_and_dlq_and_expiry_cleanup_is_bounded() {
    let db = Db::new().await;
    let store = db.store();
    let mut input = message("private", "k1", json!({"secret":"payload"}));
    input.ttl_ms = 60_000;
    let msg = store.enqueue(&db.client, input).await.unwrap();
    assert!(store.claim(&db.client, "other-owner", "private", "worker-a", 250).await.unwrap().is_none());
    let lease = store.claim(&db.client, OWNER, "private", "worker-a", 250).await.unwrap().unwrap();
    let mut forged = lease.clone();
    forged.owner = "other-owner".into();
    assert!(!store.ack(&db.client, &forged).await.unwrap());
    assert!(store.ack(&db.client, &lease).await.unwrap());
    // Expiry must not race the owner/lease assertions on a loaded CI runner.
    db.client
        .execute(
            &format!("UPDATE {}.aip_messages SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&msg.id).unwrap()],
        )
        .await
        .unwrap();
    let deleted = store.cleanup_expired(&db.client, 10).await.unwrap();
    assert_eq!(deleted, vec![uuid::Uuid::parse_str(&msg.id).unwrap()]);
    assert!(store.dead_letters(&db.client, OWNER, "private", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn enqueue_uses_callers_transaction_for_outbox_atomicity() {
    let mut db = Db::new().await;
    let store = db.store();
    let tx = db.client.transaction().await.unwrap();
    let msg = store.enqueue(&tx, message("outbox", "event-1", json!({"event":"paid"}))).await.unwrap();
    tx.rollback().await.unwrap();
    assert!(store.claim(&db.client, OWNER, "outbox", "worker-a", 250).await.unwrap().is_none());
    assert!(!msg.id.is_empty());
}

#[tokio::test]
async fn poll_wait_is_bounded_and_respects_owner_queue() {
    let db = Db::new().await;
    let store = db.store();
    let started = tokio::time::Instant::now();
    let none = store.poll_claim(&db.client, OWNER, "empty", "worker-a", 250, 120, 25).await.unwrap();
    assert!(none.is_none());
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
}

#[tokio::test]
async fn payload_and_poll_limits_are_enforced() {
    let db = Db::new().await;
    let store = db.store();
    let large = message("jobs", "large", json!({"data":"x".repeat((1 << 20) + 1)}));
    assert_eq!(store.enqueue(&db.client, large).await.unwrap_err(), Error::PayloadTooLarge);
    assert!(store.claim(&db.client, OWNER, "jobs", "worker-a", 0).await.is_err());
    assert!(store.poll_claim(&db.client, OWNER, "jobs", "worker-a", 250, 31_000, 20).await.is_err());
}

#[tokio::test]
async fn default_nack_applies_bounded_exponential_backoff() {
    let db = Db::new().await;
    let store = db.store();
    store.enqueue(&db.client, message("jobs", "retry", json!({"n":1}))).await.unwrap();
    let lease = store.claim(&db.client, OWNER, "jobs", "worker-a", 250).await.unwrap().unwrap();
    assert!(store.nack(&db.client, &lease, "TEMPORARY").await.unwrap());
    let is_backed_off: bool = db
        .client
        .query_one(
            &format!("SELECT available_at>clock_timestamp()+interval '900 milliseconds' FROM {}.aip_messages WHERE id=$1", db.schema),
            &[&uuid::Uuid::parse_str(&lease.id).unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert!(is_backed_off);
    assert!(store.claim(&db.client, OWNER, "jobs", "worker-b", 250).await.unwrap().is_none());
}
