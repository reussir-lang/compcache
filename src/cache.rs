use crate::{
    Config, EntryKind, EntryStatus, Error, ExtraOnDiskData, InMemoryCache, Key, ManagedDirectory,
    MemoryEntry, QuickCache, Result,
    error::IoContext,
    filesystem::{self, KeyLocks},
    metadata::{BLOB, Metadata},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{debug, info};

/// Cold result; serialization may fail independently of successful production.
///
/// ```no_run
/// use compcache::{Produced, Store, StoreBuilder, Key};
/// use std::sync::Arc;
/// let store: Store = StoreBuilder::new().path("/tmp/compiler-cache").open()?;
/// let key = Key::from_parts("bytecode-v1", b"complete-recipe");
/// let cached = store.get_or_insert_payload(key, || {
///     let payload: Arc<[u8]> = vec![1, 2, 3].into();
///     Ok(Produced { payload, serialized: Ok(None) }) // memory-only success
/// })?;
/// assert_eq!(&*cached.payload, &[1, 2, 3]);
/// # Ok::<(), compcache::Error>(())
/// ```
pub struct Produced<P> {
    /// Already usable; the store never serializes then materializes a cold value.
    pub payload: P,
    /// Optional persistence bytes, or a serialization failure independent of production.
    pub serialized: Result<Option<Vec<u8>>>,
}

/// Usable result, with an optional serialization/persistence/maintenance error.
#[derive(Debug)]
pub struct Cached<P> {
    pub payload: P,
    pub cache_error: Option<Error>,
}

/// Share a handle with `Arc`; independent processes may open the same root.
/// Every process must use this protocol and the same directory backend layout.
pub struct Store<I = QuickCache, D = ManagedDirectory> {
    pub(crate) metadata: Metadata,
    pub(crate) locks: KeyLocks,
    pub(crate) memory: I,
    pub(crate) directories: Option<D>,
    pub(crate) config: Config,
    pub(crate) root: PathBuf,
}
impl<I: InMemoryCache, D: ExtraOnDiskData> Store<I, D> {
    pub(crate) fn open_components(
        root: &Path,
        config: Config,
        memory: I,
        directories: Option<D>,
    ) -> Result<Self> {
        config.validate()?;
        filesystem::make_directory(root)?;
        let root = fs::canonicalize(root).at("resolve cache root", root)?;
        if let Some(store) = &directories {
            filesystem::ensure_local(store.root())?;
        }
        let db_path = root.join("cache.db");
        if db_path.try_exists().at("inspect database path", &db_path)? {
            filesystem::ensure_local(&db_path)?;
        }
        let locks = KeyLocks::new(root.join("locks"))?;
        // journal_mode changes do not reliably honor SQLite's busy handler.
        // Serialize first-open/schema setup across independent processes.
        let initialization = locks.exclusive(-1, &Key::from_bytes([0; 32]))?;
        let metadata = Metadata::open(&db_path, &config.disk)?;
        let directory_root = directories
            .as_ref()
            .map(|store| {
                fs::canonicalize(store.root()).at("resolve directory backend root", store.root())
            })
            .transpose()?;
        metadata.bind_directory_root(directory_root.as_deref())?;
        filesystem::sync_directory(&root)?;
        drop(initialization);
        let cache = Self {
            metadata,
            locks,
            memory,
            directories,
            config,
            root,
        };
        cache.recover()?;
        debug!(root = %cache.root.display(), "cache opened in SQLite WAL mode");
        Ok(cache)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Status is observable even while a builder/pruner holds its exclusive lease.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata cannot be read or contains an unknown state.
    pub fn status(&self, kind: EntryKind, key: &Key) -> Result<Option<EntryStatus>> {
        self.metadata.status(kind as i64, key)
    }
    /// Read and materialize a blob, or return this strategy's resident payload.
    /// Memory residency is independent of disk residency: a missing SQLite row
    /// does not invalidate a live snapshot.
    ///
    /// # Errors
    /// Returns locking, metadata, or materialization errors.
    pub fn get_blob(&self, key: &Key) -> Result<Option<I::Payload>> {
        if let Some(payload) = self.read_memory(key, now_secs()?)? {
            return Ok(Some(payload));
        }
        let mut retired = Vec::new();
        let lease = self.locks.exclusive(BLOB, key)?;
        let result = self.read_blob(key, now_secs()?, &mut retired);
        drop(lease);
        drop(retired);
        result
    }
    // A memory hit only clones ownership: it cannot re-admit after force eviction.
    // Keep the ordinary byte hot path free of filesystem locking.
    fn read_memory(&self, key: &Key, now: i64) -> Result<Option<I::Payload>> {
        if self.config.memory.enabled
            && let Some(entry) = self.memory.get(key)
        {
            if entry.persisted && entry.touched() < now {
                self.metadata.touch(BLOB, key, now)?;
                entry.set_touched(now);
            }
            return Ok(Some(entry.payload.clone()));
        }
        Ok(None)
    }
    fn read_blob(
        &self,
        key: &Key,
        now: i64,
        retired: &mut Vec<Arc<MemoryEntry<I::Payload>>>,
    ) -> Result<Option<I::Payload>> {
        if let Some(payload) = self.read_memory(key, now)? {
            return Ok(Some(payload));
        }
        let Some(bytes) = self.metadata.blob(key, now)? else {
            return Ok(None);
        };
        let payload = self
            .memory
            .materialize(bytes.into())
            .map_err(|error| Error::Materialize(Box::new(error)))?;
        self.admit(*key, &payload, now, true, retired);
        Ok(Some(payload))
    }
    fn admit(
        &self,
        key: Key,
        payload: &I::Payload,
        now: i64,
        persisted: bool,
        retired: &mut Vec<Arc<MemoryEntry<I::Payload>>>,
    ) {
        if self.config.memory.enabled {
            let weight = self.memory.weight(payload);
            let mut entry = MemoryEntry::with_weight(payload.clone(), now, weight);
            entry.persisted = persisted;
            retired.extend(self.memory.insert(key, Arc::new(entry)));
        }
    }
    /// Produce a live value once under the existing per-key filesystem lease.
    /// Cold production returns the original payload; only optional bytes enter
    /// SQLite. Serialization/persistence failure returns a usable payload and a
    /// cache error. Successful memory-only entries can be hit without a disk row.
    ///
    /// # Errors
    /// Returns lookup/locking/materialization errors before production, or
    /// `Error::Producer` for a true producer failure. No payload exists in those cases.
    pub fn get_or_insert_payload(
        &self,
        key: Key,
        produce: impl FnOnce() -> std::result::Result<Produced<I::Payload>, crate::ProducerError>,
    ) -> Result<Cached<I::Payload>> {
        self.get_or_insert_inner(key, || produce().map_err(Error::Producer))
    }
    fn get_or_insert_inner(
        &self,
        key: Key,
        produce: impl FnOnce() -> Result<Produced<I::Payload>>,
    ) -> Result<Cached<I::Payload>> {
        if let Some(payload) = self.read_memory(&key, now_secs()?)? {
            return Ok(Cached {
                payload,
                cache_error: None,
            });
        }
        let mut retired = Vec::new();
        let lease = self.locks.exclusive(BLOB, &key)?;
        let mut persisted = false;
        let result: Result<_> = (|| {
            let now = now_secs()?;
            if let Some(payload) = self.read_blob(&key, now, &mut retired)? {
                return Ok(Cached {
                    payload,
                    cache_error: None,
                });
            }
            debug!(%key, "producing blob payload");
            let Produced {
                payload,
                serialized,
            } = produce()?;
            let has_bytes = matches!(&serialized, Ok(Some(_)));
            let cache_error = match serialized {
                Ok(Some(bytes)) => self.metadata.insert_blob(&key, &bytes, now).err(),
                Ok(None) => None,
                Err(error) => Some(error),
            };
            persisted = cache_error.is_none() && has_bytes;
            self.admit(key, &payload, now, persisted, &mut retired);
            Ok(Cached {
                payload,
                cache_error,
            })
        })();
        drop(lease);
        drop(retired);
        let mut cached = result?;
        if persisted {
            cached.cache_error = self.after_write().err();
        }
        Ok(cached)
    }
    /// Byte-only convenience API retaining its original error behavior.
    /// Use `get_or_insert_payload` to preserve a result on persistence failure.
    ///
    /// # Errors
    /// Returns producer, lookup, persistence or maintenance errors.
    pub fn get_or_insert_blob(
        &self,
        key: Key,
        produce: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Arc<[u8]>>
    where
        I: InMemoryCache<Payload = Arc<[u8]>>,
    {
        let cached = self.get_or_insert_inner(key, || {
            let bytes = produce()?;
            Ok(Produced {
                payload: Arc::from(bytes.clone()),
                serialized: Ok(Some(bytes)),
            })
        })?;
        if let Some(error) = cached.cache_error {
            return Err(error);
        }
        Ok(cached.payload)
    }
    /// Remove the persisted blob and this Store's memory ownership. Outstanding
    /// payloads and other stores' snapshots remain valid. The key lease covers
    /// both publication and final admission, so older in-flight work cannot
    /// re-admit after this returns. A subsequent request can repopulate the key.
    /// Runtime directories and their reader leases are unaffected.
    ///
    /// # Errors
    /// Returns locking or SQLite errors. On error, memory ownership is unchanged.
    pub fn force_evict_blob(&self, key: &Key) -> Result<()> {
        let lease = self.locks.exclusive(BLOB, key)?;
        self.metadata.forget_blob(key)?;
        let retired = self.memory.remove(key);
        drop(lease);
        drop(retired);
        Ok(())
    }
    pub(crate) fn after_write(&self) -> Result<()> {
        if self.config.disk.prune_on_write {
            self.prune_if_needed()
                .map_err(|e| Error::Maintenance(Box::new(e)))?;
        }
        Ok(())
    }
    pub(crate) fn directory_store(&self) -> Result<&D> {
        self.directories.as_ref().ok_or(Error::DirectoriesDisabled)
    }
    /// Logical runtime payloads plus DB/WAL/SHM lengths, without walking payload trees.
    /// Blob bytes are included in SQLite files, never counted twice.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable metadata/files, invalid recorded sizes or overflow.
    pub fn accounted_size(&self) -> Result<u64> {
        let mut bytes = self.metadata.directory_bytes()?;
        for name in ["cache.db", "cache.db-wal", "cache.db-shm"] {
            let path = self.root.join(name);
            match fs::metadata(&path) {
                Ok(m) => bytes = bytes.checked_add(m.len()).ok_or(Error::SizeOverflow)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).at("measure SQLite file", path),
            }
        }
        Ok(bytes)
    }
    /// Flush WAL frames when readers allow it. SQLite can reuse freed DB pages;
    /// pruning is not a promise that the physical DB file shrinks immediately.
    ///
    /// # Errors
    ///
    /// Returns an error if SQLite cannot perform the checkpoint.
    pub fn checkpoint(&self) -> Result<bool> {
        let completed = self.metadata.checkpoint()?;
        info!(completed, "cache WAL checkpoint requested");
        Ok(completed)
    }
}
pub(crate) fn now_secs() -> Result<i64> {
    i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
        .map_err(|_| Error::TimestampOverflow)
}
