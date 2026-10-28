use crate::{Key, MemoryConfig, Result};
use quick_cache::{Lifecycle, OptionsBuilder, Weighter, sync::Cache};
use rapidhash::fast::RandomState;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

/// A live payload, its admission weight and last persisted access second.
/// Serialized bytes need not be retained alongside the payload.
#[derive(Debug)]
pub struct MemoryEntry<P = Arc<[u8]>> {
    pub payload: P,
    weight: u64,
    pub(crate) persisted: bool,
    touched: AtomicI64,
}
impl MemoryEntry {
    #[must_use]
    pub fn new(bytes: Arc<[u8]>, touched: i64) -> Self {
        Self {
            persisted: true,
            weight: bytes.len() as u64 + 32,
            payload: bytes,
            touched: AtomicI64::new(touched),
        }
    }
}
impl<P> MemoryEntry<P> {
    /// Weight is an application estimate, independent of serialized size.
    #[must_use]
    pub fn with_weight(payload: P, touched: i64, weight: u64) -> Self {
        Self {
            payload,
            touched: AtomicI64::new(touched),
            weight,
            persisted: false,
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
    /// Usually an `Arc` owning a native object and its client context.
    type Payload: Clone + Send + Sync;
    /// Materialize disk bytes with this strategy's caller context. Called under
    /// the key lease, without a SQLite or memory-cache lock. Do not reenter the
    /// same key. Context identities belong to memory, not the persistent recipe.
    ///
    /// # Errors
    /// Returns an error for incompatible or invalid serialized bytes.
    fn materialize(&self, bytes: Arc<[u8]>) -> Result<Self::Payload>;
    fn weight(&self, payload: &Self::Payload) -> u64;

    /// Initialize the memory strategy from its configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the strategy cannot initialize from the supplied configuration.
    fn from_config(config: &MemoryConfig) -> Result<Self>
    where
        Self: Sized;
    fn get(&self, key: &Key) -> Option<Arc<MemoryEntry<Self::Payload>>>;
    /// Return all rejected/replaced/evicted entries without dropping their payloads
    /// under internal locks. Store releases them after the key lease is released.
    fn insert(
        &self,
        key: Key,
        value: Arc<MemoryEntry<Self::Payload>>,
    ) -> Vec<Arc<MemoryEntry<Self::Payload>>>;
    fn remove(&self, key: &Key) -> Option<Arc<MemoryEntry<Self::Payload>>>;
    fn clear(&self) -> Vec<Arc<MemoryEntry<Self::Payload>>>;
}
#[derive(Default, Debug)]
pub struct Bypass;
impl InMemoryCache for Bypass {
    type Payload = Arc<[u8]>;
    fn materialize(&self, bytes: Arc<[u8]>) -> Result<Self::Payload> {
        Ok(bytes)
    }
    fn weight(&self, payload: &Self::Payload) -> u64 {
        payload.len() as u64 + 32
    }
    fn from_config(_: &MemoryConfig) -> Result<Self> {
        Ok(Self)
    }
    fn get(&self, _: &Key) -> Option<Arc<MemoryEntry>> {
        None
    }
    fn insert(&self, _: Key, value: Arc<MemoryEntry>) -> Vec<Arc<MemoryEntry>> {
        vec![value]
    }
    fn remove(&self, _: &Key) -> Option<Arc<MemoryEntry>> {
        None
    }
    fn clear(&self) -> Vec<Arc<MemoryEntry>> {
        Vec::new()
    }
}
#[derive(Clone)]
struct EntryWeight;
impl<P> Weighter<Key, Arc<MemoryEntry<P>>> for EntryWeight {
    fn weight(&self, _: &Key, value: &Arc<MemoryEntry<P>>) -> u64 {
        value.weight
    }
}
#[derive(Clone)]
struct Retire;
impl<P> Lifecycle<Key, Arc<MemoryEntry<P>>> for Retire {
    type RequestState = Vec<Arc<MemoryEntry<P>>>;
    fn begin_request(&self) -> Self::RequestState {
        Vec::new()
    }
    fn on_evict(&self, state: &mut Self::RequestState, _: Key, value: Arc<MemoryEntry<P>>) {
        state.push(value);
    }
}
/// Weighted storage, reusable inside a context-owning `InMemoryCache` strategy.
/// Its default strategy materializes ordinary bytes.
pub struct QuickCache<P = Arc<[u8]>> {
    cache: Cache<Key, Arc<MemoryEntry<P>>, EntryWeight, RandomState, Retire>,
    max_entry_weight: u64,
}
impl<P: Send + Sync> QuickCache<P> {
    /// Construct a weighted, sharded memory cache.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid memory settings or cache construction options.
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
                EntryWeight,
                RandomState::new(),
                Retire,
            ),
            max_entry_weight: config.max_entry_weight,
        })
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.cache.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[must_use]
    pub fn weight(&self) -> u64 {
        self.cache.weight()
    }
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.cache.hits()
    }
    #[must_use]
    pub fn misses(&self) -> u64 {
        self.cache.misses()
    }
}
impl<P: Send + Sync> QuickCache<P> {
    #[must_use]
    pub fn get(&self, key: &Key) -> Option<Arc<MemoryEntry<P>>> {
        self.cache.get(key)
    }
    pub fn insert(&self, key: Key, value: Arc<MemoryEntry<P>>) -> Vec<Arc<MemoryEntry<P>>> {
        if value.weight <= self.max_entry_weight {
            self.cache.insert_with_lifecycle(key, value)
        } else {
            vec![value]
        }
    }
    #[must_use]
    pub fn remove(&self, key: &Key) -> Option<Arc<MemoryEntry<P>>> {
        self.cache.remove(key).map(|(_, value)| value)
    }
    /// Remove a snapshot of keys. Concurrent insertions may survive, as with a
    /// sharded clear. Never run a payload destructor under a shard lock.
    #[must_use]
    pub fn clear(&self) -> Vec<Arc<MemoryEntry<P>>> {
        let keys: Vec<_> = self.cache.iter().map(|(key, _)| key).collect();
        keys.iter().filter_map(|key| self.remove(key)).collect()
    }
}
impl InMemoryCache for QuickCache {
    type Payload = Arc<[u8]>;
    fn materialize(&self, bytes: Arc<[u8]>) -> Result<Self::Payload> {
        Ok(bytes)
    }
    fn weight(&self, payload: &Self::Payload) -> u64 {
        payload.len() as u64 + 32
    }
    fn from_config(config: &MemoryConfig) -> Result<Self> {
        Self::new(config)
    }
    fn get(&self, key: &Key) -> Option<Arc<MemoryEntry>> {
        Self::get(self, key)
    }
    fn insert(&self, key: Key, value: Arc<MemoryEntry>) -> Vec<Arc<MemoryEntry>> {
        Self::insert(self, key, value)
    }
    fn remove(&self, key: &Key) -> Option<Arc<MemoryEntry>> {
        Self::remove(self, key)
    }
    fn clear(&self) -> Vec<Arc<MemoryEntry>> {
        Self::clear(self)
    }
}
