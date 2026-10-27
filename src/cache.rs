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

/// Share a handle with `Arc`; independent processes may open the same root.
/// Every process must use this protocol and the same directory backend layout.
pub struct Store<I = QuickCache, D = ManagedDirectory> {
    pub(crate) metadata: Metadata,
    pub(crate) locks: KeyLocks,
    pub(crate) memory: Option<I>,
    pub(crate) directories: Option<D>,
    pub(crate) config: Config,
    pub(crate) root: PathBuf,
}
impl<I: InMemoryCache, D: ExtraOnDiskData> Store<I, D> {
    pub(crate) fn open_components(
        root: &Path,
        config: Config,
        memory: Option<I>,
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
    pub fn status(&self, kind: EntryKind, key: &Key) -> Result<Option<EntryStatus>> {
        self.metadata.status(kind as i64, key)
    }
    pub fn get_blob(&self, key: &Key) -> Result<Option<Arc<[u8]>>> {
        let now = now_secs()?;
        if let Some(entry) = self.memory.as_ref().and_then(|m| m.get(key)) {
            if entry.touched() >= now || self.metadata.touch(BLOB, key, now)? {
                entry.set_touched(now);
                return Ok(Some(entry.bytes.clone()));
            }
            if let Some(memory) = &self.memory {
                memory.remove(key);
            }
        }
        let _lease = self.locks.shared(BLOB, key)?;
        self.read_blob(key, now)
    }
    fn read_blob(&self, key: &Key, now: i64) -> Result<Option<Arc<[u8]>>> {
        let Some(bytes) = self.metadata.blob(key, now)? else {
            return Ok(None);
        };
        let bytes: Arc<[u8]> = bytes.into();
        if let Some(memory) = &self.memory {
            memory.insert(*key, Arc::new(MemoryEntry::new(bytes.clone(), now)));
        }
        Ok(Some(bytes))
    }
    /// At most one producer runs for a key at a time, including across processes.
    /// A failed or crashed producer can be retried; recipes must be deterministic.
    pub fn get_or_insert_blob(
        &self,
        key: Key,
        produce: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Arc<[u8]>> {
        if let Some(bytes) = self.get_blob(&key)? {
            return Ok(bytes);
        }
        let lease = self.locks.exclusive(BLOB, &key)?;
        if let Some(bytes) = self.read_blob(&key, now_secs()?)? {
            return Ok(bytes);
        }
        debug!(%key, "producing SQLite blob");
        let bytes: Arc<[u8]> = produce()?.into();
        let now = now_secs()?;
        self.metadata.insert_blob(&key, &bytes, now)?;
        if let Some(memory) = &self.memory {
            memory.insert(key, Arc::new(MemoryEntry::new(bytes.clone(), now)));
        }
        drop(lease);
        self.after_write()?;
        Ok(bytes)
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
