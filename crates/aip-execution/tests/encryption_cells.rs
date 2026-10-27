//! DB-backed cell boundary tests; no listener readiness or policy authorization is implied.
use aip_crypto::provider::LocalProvider;
use aip_execution::encryption::{CellLocation, DekStore, Error, MAX_DEK_SEALS, Scope};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
const SENTINEL: &[u8] = b"explicit-sensitive-cell-fixture-582e8c36";
static NEXT: AtomicU64 = AtomicU64::new(0);

async fn connect() -> Client {
    let (db, driver) = tokio_postgres::connect(DB, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = driver.await;
    });
    db
}
fn provider(id: &str, byte: u8) -> LocalProvider {
    LocalProvider::from_spec(&format!("{id}:{}", STANDARD.encode([byte; 32])), id).unwrap()
}
fn location(id: &str) -> CellLocation {
    CellLocation::new("Secret", "value", id).unwrap()
}
async fn fixture() -> (Client, DekStore) {
    let mut db = connect().await;
    let store = DekStore::new(&format!("aip_cells_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))).unwrap();
    db.batch_execute(&store.ddl()).await.unwrap();
    let tx = db.transaction().await.unwrap();
    store.ensure_active_in(&tx, &Scope::service(), &provider("old", 7)).await.unwrap();
    tx.commit().await.unwrap();
    (db, store)
}
async fn cleanup(db: &Client, store: &DekStore) {
    db.batch_execute(&format!("SET lock_timeout='2s'; SET statement_timeout='5s'; DROP SCHEMA {} CASCADE", store.infrastructure_schema()))
        .await
        .unwrap();
}
async fn count(db: &Client, store: &DekStore) -> i64 {
    db.query_one(&format!("SELECT sum(seal_count)::bigint FROM {}.aip_encryption_deks", store.infrastructure_schema()), &[]).await.unwrap().get(0)
}

#[test]
fn cell_locations_require_canonical_identity_and_bounded_names() {
    for id in ["", "01", "-1", "9223372036854775808", "550E8400-E29B-41D4-A716-446655440000"] {
        assert!(CellLocation::new("Secret", "value", id).is_err(), "{id}");
    }
    assert!(CellLocation::new("Secret", "value", "550e8400-e29b-41d4-a716-446655440000").is_ok());
    assert!(CellLocation::new("Secret;DROP", "value", "1").is_err());
    assert!(CellLocation::new("Secret", "", "1").is_err());
}

#[tokio::test]
async fn reservations_survive_business_rollback_and_only_store_authenticated_frames() {
    let (db, store) = fixture().await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let scope = Scope::service();
        let kek = provider("old", 7);
        let mut budget = connect().await;
        let mut request = connect().await;
        request
            .batch_execute(&format!("CREATE TABLE {}.cells(id bigint PRIMARY KEY, value text NOT NULL)", owned.infrastructure_schema()))
            .await
            .unwrap();
        let mut permit = owned.reserve_for_write(&mut budget, &scope, 2).await.unwrap();
        assert_eq!(count(&budget, &owned).await, 2);
        let tx = request.transaction().await.unwrap();
        let frame = owned.seal_in(&tx, &mut permit, &location("1"), SENTINEL, &kek).await.unwrap();
        tx.execute(&format!("INSERT INTO {}.cells VALUES(1,$1)", owned.infrastructure_schema()), &[&frame]).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(count(&budget, &owned).await, 2, "a rolled-back write still consumed a nonce reservation");
        let tx = request.transaction().await.unwrap();
        let frame = owned.seal_in(&tx, &mut permit, &location("2"), SENTINEL, &kek).await.unwrap();
        tx.execute(&format!("INSERT INTO {}.cells VALUES(2,$1)", owned.infrastructure_schema()), &[&frame]).await.unwrap();
        assert_eq!(owned.seal_in(&tx, &mut permit, &location("3"), SENTINEL, &kek).await.unwrap_err(), Error::UsageExhausted);
        tx.commit().await.unwrap();
        let stored: String =
            request.query_one(&format!("SELECT value FROM {}.cells WHERE id=2", owned.infrastructure_schema()), &[]).await.unwrap().get(0);
        assert!(!stored.contains(std::str::from_utf8(SENTINEL).unwrap()));
        assert_eq!(owned.open_cell(&request, &scope, &location("2"), &stored, &kek).await.unwrap().as_slice(), SENTINEL);
        let metadata: String = request
            .query_one(&format!("SELECT row_to_json(d)::text FROM {}.aip_encryption_deks d", owned.infrastructure_schema()), &[])
            .await
            .unwrap()
            .get(0);
        assert!(!metadata.contains(std::str::from_utf8(SENTINEL).unwrap()));
        assert!(!format!("{permit:?}").contains("wrapped"));
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}

#[tokio::test]
async fn every_cell_identity_and_unknown_keys_fail_closed_without_plaintext_fallback() {
    let (db, store) = fixture().await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let scope = Scope::service();
        let kek = provider("old", 7);
        let mut budget = connect().await;
        let mut request = connect().await;
        let mut permit = owned.reserve_for_write(&mut budget, &scope, 1).await.unwrap();
        let tx = request.transaction().await.unwrap();
        let frame = owned.seal_in(&tx, &mut permit, &location("1"), SENTINEL, &kek).await.unwrap();
        tx.commit().await.unwrap();
        for changed in [location("2"), CellLocation::new("Other", "value", "1").unwrap(), CellLocation::new("Secret", "other", "1").unwrap()] {
            assert_eq!(owned.open_cell(&request, &scope, &changed, &frame, &kek).await.unwrap_err(), Error::Corrupted);
        }
        assert!(owned.open_cell(&request, &Scope::reference("Project", "1").unwrap(), &location("1"), &frame, &kek).await.is_err());
        assert!(DekStore::new("aip_other_deployment").unwrap().open_cell(&request, &scope, &location("1"), &frame, &kek).await.is_err());
        for malformed in [std::str::from_utf8(SENTINEL).unwrap().to_string(), frame.replace(":1:", ":2:"), format!("{frame}A")] {
            assert!(owned.open_cell(&request, &scope, &location("1"), &malformed, &kek).await.is_err());
        }
        assert_eq!(owned.open_cell(&request, &scope, &location("1"), &frame, &provider("missing", 9)).await.unwrap_err(), Error::Crypto);
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_budget_reservations_cannot_overrun_the_key_limit() {
    let (db, store) = fixture().await;
    db.execute(&format!("UPDATE {}.aip_encryption_deks SET seal_count=$1", store.infrastructure_schema()), &[&(MAX_DEK_SEALS as i64 - 3)])
        .await
        .unwrap();
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let gate = Arc::new(tokio::sync::Barrier::new(12));
        let mut tasks = vec![];
        for _ in 0..12 {
            let (store, gate) = (owned.clone(), gate.clone());
            tasks.push(tokio::spawn(async move {
                let mut db = connect().await;
                gate.wait().await;
                store.reserve_for_write(&mut db, &Scope::service(), 1).await
            }));
        }
        let mut successes = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(_) => successes += 1,
                Err(e) => assert_eq!(e, Error::UsageExhausted),
            }
        }
        assert_eq!(successes, 3);
        let mut db = connect().await;
        assert_eq!(count(&db, &owned).await, MAX_DEK_SEALS as i64);
        for invalid in [0, 501, u32::MAX] {
            assert_eq!(owned.reserve_for_write(&mut db, &Scope::service(), invalid).await.unwrap_err(), Error::InvalidInput);
        }
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}

#[tokio::test]
async fn promotion_rejects_cached_old_writers_and_retired_reads_but_keeps_readable_data() {
    let (db, store) = fixture().await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let scope = Scope::service();
        let kek = provider("old", 7);
        let mut budget = connect().await;
        let mut request = connect().await;
        let mut permit = owned.reserve_for_write(&mut budget, &scope, 2).await.unwrap();
        let tx = request.transaction().await.unwrap();
        let frame = owned.seal_in(&tx, &mut permit, &location("1"), SENTINEL, &kek).await.unwrap();
        let before = owned.active_for_write_in(&tx, &scope).await.unwrap();
        let old_id = before.dek().id().to_string();
        tx.commit().await.unwrap();
        let tx = budget.transaction().await.unwrap();
        let after = owned.promote_in(&tx, &scope, before.epoch(), &kek).await.unwrap();
        assert_eq!(after.epoch(), before.epoch() + 1);
        assert_eq!(after.dek().version(), 2);
        assert_ne!(after.dek().id(), old_id);
        tx.commit().await.unwrap();
        let tx = request.transaction().await.unwrap();
        assert_eq!(owned.seal_in(&tx, &mut permit, &location("2"), SENTINEL, &kek).await.unwrap_err(), Error::StaleWriter);
        assert_eq!(owned.promote_in(&tx, &scope, before.epoch(), &kek).await.unwrap_err(), Error::StaleWriter);
        tx.rollback().await.unwrap();
        assert_eq!(owned.open_cell(&request, &scope, &location("1"), &frame, &kek).await.unwrap().as_slice(), SENTINEL);
        request
            .execute(
                &format!("UPDATE {}.aip_encryption_deks SET state='RETIRED' WHERE dek_id=$1::text::uuid", owned.infrastructure_schema()),
                &[&old_id],
            )
            .await
            .unwrap();
        assert_eq!(owned.open_cell(&request, &scope, &location("1"), &frame, &kek).await.unwrap_err(), Error::NotFound);
        let mut permit = owned.reserve_for_write(&mut budget, &scope, 1).await.unwrap();
        let tx = request.transaction().await.unwrap();
        let fresh = owned.seal_in(&tx, &mut permit, &location("2"), SENTINEL, &kek).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(owned.open_cell(&request, &scope, &location("2"), &fresh, &kek).await.unwrap().as_slice(), SENTINEL);
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}

#[tokio::test]
async fn rewrap_uses_current_metadata_and_cross_deployment_permits_are_rejected() {
    let (db, store) = fixture().await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let scope = Scope::service();
        let (old, new) = (provider("old", 7), provider("new", 9));
        let mut budget = connect().await;
        let mut request = connect().await;
        let mut permit = owned.reserve_for_write(&mut budget, &scope, 2).await.unwrap();
        let tx = request.transaction().await.unwrap();
        let frame = owned.seal_in(&tx, &mut permit, &location("1"), SENTINEL, &old).await.unwrap();
        let active = owned.active_for_write_in(&tx, &scope).await.unwrap();
        tx.commit().await.unwrap();
        let tx = budget.transaction().await.unwrap();
        owned.rewrap_in(&tx, &scope, active.dek().id(), active.dek().version(), &old, &new).await.unwrap();
        tx.commit().await.unwrap();
        let tx = request.transaction().await.unwrap();
        assert_eq!(
            DekStore::new("aip_other_deployment").unwrap().seal_in(&tx, &mut permit, &location("2"), SENTINEL, &new).await.unwrap_err(),
            Error::InvalidInput
        );
        let fresh = owned.seal_in(&tx, &mut permit, &location("2"), SENTINEL, &new).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(owned.open_cell(&request, &scope, &location("1"), &frame, &new).await.unwrap().as_slice(), SENTINEL);
        assert_eq!(owned.open_cell(&request, &scope, &location("2"), &fresh, &new).await.unwrap().as_slice(), SENTINEL);
        assert_eq!(owned.open_cell(&request, &scope, &location("1"), &frame, &old).await.unwrap_err(), Error::Crypto);
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}

#[tokio::test]
async fn real_cell_writer_blocks_promotion_until_the_business_commit() {
    let (db, store) = fixture().await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let scope = Scope::service();
        let kek = provider("old", 7);
        let mut budget = connect().await;
        let mut writer = connect().await;
        let mut permit = owned.reserve_for_write(&mut budget, &scope, 1).await.unwrap();
        let tx = writer.transaction().await.unwrap();
        owned.seal_in(&tx, &mut permit, &location("1"), SENTINEL, &kek).await.unwrap();
        let promotion = budget.transaction().await.unwrap();
        promotion.batch_execute("SET LOCAL lock_timeout='100ms'; SET LOCAL statement_timeout='2s'").await.unwrap();
        assert_eq!(owned.promote_in(&promotion, &scope, 1, &kek).await.unwrap_err(), Error::Database);
        promotion.rollback().await.unwrap();
        tx.commit().await.unwrap();
        let promotion = budget.transaction().await.unwrap();
        assert_eq!(owned.promote_in(&promotion, &scope, 1, &kek).await.unwrap().epoch(), 2);
        promotion.commit().await.unwrap();
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}

#[tokio::test]
async fn another_cell_can_reserve_budget_while_the_same_business_transaction_is_live() {
    let (db, store) = fixture().await;
    let owned = store.clone();
    let result = tokio::spawn(async move {
        let scope = Scope::service();
        let kek = provider("old", 7);
        let mut budget = connect().await;
        let mut writer = connect().await;
        let mut first = owned.reserve_for_write(&mut budget, &scope, 1).await.unwrap();
        let tx = writer.transaction().await.unwrap();
        owned.seal_in(&tx, &mut first, &location("1"), SENTINEL, &kek).await.unwrap();
        let mut second = tokio::time::timeout(std::time::Duration::from_millis(500), owned.reserve_for_write(&mut budget, &scope, 1))
            .await
            .expect("another field's budget must not wait behind its own transaction")
            .unwrap();
        owned.seal_in(&tx, &mut second, &location("2"), SENTINEL, &kek).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(count(&budget, &owned).await, 2);
    })
    .await;
    cleanup(&db, &store).await;
    result.unwrap();
}
