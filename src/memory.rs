use crate::{Key, MemoryConfig, Result};
use quick_cache::{
    OptionsBuilder, Weighter,
    sync::{Cache, DefaultLifecycle},
};
use rapidhash::fast::RandomState;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

/// Shared immutable bytes and the last successfully persisted access second.
#[derive(Debug)]
pub struct MemoryEntry {
    pub bytes: Arc<[u8]>,
    touched: AtomicI64,
}
impl MemoryEntry {
    pub fn new(bytes: Arc<[u8]>, touched: i64) -> Self {
        Self {
            bytes,
            touched: AtomicI64::new(touched),
        }
    }
    pub(crate) fn touched(&self) -> i64 {
        self.touched.load(Ordering::Relaxed)
    }
    pub(crate) fn set_touched(&self, now: i64) {
        self.touched.fetch_max(now, Ordering::Relaxed);
    }
}
/// Implementations must support concurrent calls. Blob values are immutable snapshots.
/// A strategy must not share entries between distinct cache roots.
pub trait InMemoryCache: Send + Sync {
    fn from_config(config: &MemoryConfig) -> Result<Self>
    where
        Self: Sized;
    fn get(&self, key: &Key) -> Option<Arc<MemoryEntry>>;
    fn insert(&self, key: Key, value: Arc<MemoryEntry>);
    fn remove(&self, key: &Key);
    fn clear(&self);
}
#[derive(Default, Debug)]
pub struct Bypass;
impl InMemoryCache for Bypass {
    fn from_config(_: &MemoryConfig) -> Result<Self> {
        Ok(Self)
    }
    fn get(&self, _: &Key) -> Option<Arc<MemoryEntry>> {
        None
    }
    fn insert(&self, _: Key, _: Arc<MemoryEntry>) {}
    fn remove(&self, _: &Key) {}
    fn clear(&self) {}
}
#[derive(Clone)]
struct ByteWeight;
impl Weighter<Key, Arc<MemoryEntry>> for ByteWeight {
    fn weight(&self, _: &Key, value: &Arc<MemoryEntry>) -> u64 {
        value.bytes.len() as u64 + 32
    }
}
pub struct QuickCache {
    cache: Cache<Key, Arc<MemoryEntry>, ByteWeight, RandomState>,
    max_entry_weight: u64,
}
impl QuickCache {
    pub fn new(config: &MemoryConfig) -> Result<Self> {
        crate::Config {
            memory: config.clone(),
            ..Default::default()
        }
        .validate()?;
        let mut options = OptionsBuilder::new();
        options
            .estimated_items_capacity(config.estimated_items_capacity)
            .weight_capacity(config.weight_capacity)
            .hot_allocation(config.hot_allocation)
            .ghost_allocation(config.ghost_allocation);
        if let Some(shards) = config.shards {
            options.shards(shards);
        }
        Ok(Self {
            cache: Cache::with_options(
                options
                    .build()
                    .map_err(|e| crate::Error::MemoryOptions(Box::new(e)))?,
                ByteWeight,
                RandomState::new(),
                DefaultLifecycle::default(),
            ),
            max_entry_weight: config.max_entry_weight,
        })
    }
    pub fn len(&self) -> usize {
        self.cache.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn weight(&self) -> u64 {
        self.cache.weight()
    }
    pub fn hits(&self) -> u64 {
        self.cache.hits()
    }
    pub fn misses(&self) -> u64 {
        self.cache.misses()
    }
}
impl InMemoryCache for QuickCache {
    fn from_config(config: &MemoryConfig) -> Result<Self> {
        Self::new(config)
    }
    fn get(&self, key: &Key) -> Option<Arc<MemoryEntry>> {
        self.cache.get(key)
    }
    fn insert(&self, key: Key, value: Arc<MemoryEntry>) {
        if value.bytes.len() as u64 + 32 <= self.max_entry_weight {
            self.cache.insert(key, value);
        }
    }
    fn remove(&self, key: &Key) {
        self.cache.remove(key);
    }
    fn clear(&self) {
        self.cache.clear();
    }
}
