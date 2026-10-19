#![allow(clippy::unwrap_used)]
use aip_crypto::{
    cache::{CacheKey, DekCache},
    cell::DataKey,
};
use std::{sync::Arc, time::Duration};
fn key(id: &str, kek: &str) -> CacheKey {
    CacheKey::new("aip_fixture", "ref:Project", "7", id, 1, kek).unwrap()
}

#[tokio::test(start_paused = true)]
async fn cache_binds_full_key_identity_evicts_lru_and_expires_without_a_read() {
    let cache = DekCache::new(2, Duration::from_secs(2)).unwrap();
    let first = Arc::new(DataKey::new("dek1", 1, [1; 32]).unwrap());
    let weak_first = Arc::downgrade(&first);
    cache.insert(key("dek1", "kek1"), first).unwrap();
    let second = Arc::new(DataKey::new("dek2", 1, [2; 32]).unwrap());
    let weak_second = Arc::downgrade(&second);
    cache.insert(key("dek2", "kek1"), second).unwrap();
    assert!(cache.get(&key("dek1", "kek1")).unwrap().is_some());
    assert!(cache.get(&key("dek1", "kek2")).unwrap().is_none(), "rewrapping must not reuse an entry for another KEK");
    assert!(cache.get(&CacheKey::new("aip_fixture", "ref:Project", "8", "dek1", 1, "kek1").unwrap()).unwrap().is_none());
    cache.insert(key("dek3", "kek1"), Arc::new(DataKey::new("dek3", 1, [3; 32]).unwrap())).unwrap();
    assert!(weak_second.upgrade().is_none(), "eviction releases the key when no borrower holds it");
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert!(weak_first.upgrade().is_none(), "the maintenance task removes expired material even without another get");
    assert!(cache.get(&key("dek1", "kek1")).unwrap().is_none());
}

#[tokio::test]
async fn cache_rejects_unbounded_configuration_and_mismatched_material() {
    for capacity in [0, 4097] {
        assert!(DekCache::new(capacity, Duration::from_secs(2)).is_err());
    }
    for ttl in [Duration::ZERO, Duration::from_secs(301)] {
        assert!(DekCache::new(1, ttl).is_err());
    }
    let cache = DekCache::new(1, Duration::from_secs(2)).unwrap();
    assert!(cache.insert(key("dek1", "kek1"), Arc::new(DataKey::new("wrong", 1, [1; 32]).unwrap())).is_err());
}

#[test]
fn cache_without_a_runtime_fails_closed() {
    assert!(matches!(DekCache::new(1, Duration::from_secs(2)), Err(aip_crypto::cell::Error::Unavailable)));
}

#[tokio::test(start_paused = true)]
async fn cache_drop_releases_entries_but_preserves_live_borrowers() {
    let cache = DekCache::new(1, Duration::from_secs(1)).unwrap();
    let material = Arc::new(DataKey::new("dek1", 1, [1; 32]).unwrap());
    let weak = Arc::downgrade(&material);
    cache.insert(key("dek1", "kek1"), material).unwrap();
    let borrower = cache.get(&key("dek1", "kek1")).unwrap().unwrap();
    for changed in [
        CacheKey::new("other", "ref:Project", "7", "dek1", 1, "kek1").unwrap(),
        CacheKey::new("aip_fixture", "ref:Other", "7", "dek1", 1, "kek1").unwrap(),
        CacheKey::new("aip_fixture", "ref:Project", "7", "dek1", 2, "kek1").unwrap(),
    ] {
        assert!(cache.get(&changed).unwrap().is_none());
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(cache.get(&key("dek1", "kek1")).unwrap().is_none(), "expiry is enforced even before maintenance runs");
    assert!(weak.upgrade().is_some());
    cache.insert(key("dek1", "kek1"), borrower.clone()).unwrap();
    drop(cache);
    assert_eq!(Arc::strong_count(&borrower), 1, "drop releases the cache reference immediately");
    drop(borrower);
    assert!(weak.upgrade().is_none());
    tokio::task::yield_now().await;
}
