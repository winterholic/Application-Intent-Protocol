//! Bounded LRU DEK cache with a cancellable Tokio expiry task. Callers may retain borrowed keys past eviction;
//! eviction/expiry removes the cache's reference and zeroizes material once its last borrower releases it.
use crate::cell::{DataKey, Error, valid_id};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    schema: String,
    scope_kind: String,
    scope_id: String,
    dek_id: String,
    version: u32,
    kek_id: String,
}
impl CacheKey {
    pub fn new(schema: &str, scope_kind: &str, scope_id: &str, dek_id: &str, version: u32, kek_id: &str) -> Result<Self, Error> {
        if [schema, scope_kind, scope_id].iter().any(|part| part.is_empty() || part.len() > 256)
            || !valid_id(dek_id)
            || !valid_id(kek_id)
            || version == 0
        {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            schema: schema.into(),
            scope_kind: scope_kind.into(),
            scope_id: scope_id.into(),
            dek_id: dek_id.into(),
            version,
            kek_id: kek_id.into(),
        })
    }
}
struct Entry {
    expires: Instant,
    material: Arc<DataKey>,
}

pub struct DekCache {
    entries: Arc<Mutex<LruCache<CacheKey, Entry>>>,
    ttl: Duration,
    maintenance: tokio::task::JoinHandle<()>,
}
impl DekCache {
    /// Needs a live Tokio runtime. Capacity <=4096, TTL 1..300 seconds. Expiry is checked on every get;
    /// an idle cache is purged by the owned one-second task, which is aborted when this cache is dropped.
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, Error> {
        if !(1..=4096).contains(&capacity) || !(Duration::from_secs(1)..=Duration::from_secs(300)).contains(&ttl) {
            return Err(Error::InvalidInput);
        }
        let handle = tokio::runtime::Handle::try_current().map_err(|_| Error::Unavailable)?;
        let entries = Arc::new(Mutex::new(LruCache::<CacheKey, Entry>::new(NonZeroUsize::new(capacity).ok_or(Error::InvalidInput)?)));
        let weak = Arc::downgrade(&entries);
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let maintenance = handle.spawn(async move {
            loop {
                timer.tick().await;
                let Some(entries) = weak.upgrade() else { break };
                let Ok(mut cache) = entries.lock() else { break };
                let now = Instant::now();
                let expired: Vec<_> = cache.iter().filter(|(_, entry)| entry.expires <= now).map(|(key, _)| key.clone()).collect();
                for key in expired {
                    cache.pop(&key);
                }
            }
        });
        Ok(Self { entries, ttl, maintenance })
    }
    pub fn insert(&self, key: CacheKey, value: Arc<DataKey>) -> Result<(), Error> {
        if key.dek_id != value.id() || key.version != value.version() {
            return Err(Error::InvalidInput);
        }
        let mut cache = self.entries.lock().map_err(|_| Error::Unavailable)?;
        cache.put(key, Entry { expires: Instant::now() + self.ttl, material: value });
        Ok(())
    }
    pub fn get(&self, key: &CacheKey) -> Result<Option<Arc<DataKey>>, Error> {
        let mut cache = self.entries.lock().map_err(|_| Error::Unavailable)?;
        if cache.peek(key).is_some_and(|entry| entry.expires <= Instant::now()) {
            cache.pop(key);
            return Ok(None);
        }
        Ok(cache.get(key).map(|entry| entry.material.clone()))
    }
}
impl Drop for DekCache {
    fn drop(&mut self) {
        self.maintenance.abort();
    }
}
