use crate::{
    DirectoryProducer, EntryStatus, Error, ExtraOnDiskData, InMemoryCache, Key, Result,
    RuntimeLease, Store, cache::now_secs, filesystem::retain_shared, metadata::DIRECTORY,
};
use tracing::{debug, info, warn};

impl<I: InMemoryCache, D: ExtraOnDiskData> Store<I, D> {
    /// Acquire a reader lease for complete published output.
    ///
    /// # Errors
    ///
    /// Returns an error if directory storage is disabled or lease, metadata or filesystem operations fail.
    pub fn get_runtime(&self, key: &Key) -> Result<Option<RuntimeLease>> {
        let store = self.directory_store()?;
        let lock = self.locks.shared(DIRECTORY, key)?;
        let Some(row) = self.metadata.directory(key)? else {
            return Ok(None);
        };
        if row.status != EntryStatus::Ready {
            return Ok(None);
        }
        if !store.exists(&row.object)? {
            drop(lock);
            self.recover_key(key)?;
            return Ok(None);
        }
        self.metadata.touch(DIRECTORY, key, now_secs()?)?;
        Ok(Some(RuntimeLease {
            path: store.path(&row.object),
            _lock: lock,
        }))
    }
    /// Acquire a runtime lease, producing output once under an exclusive lease on a miss.
    ///
    /// # Errors
    ///
    /// Returns a backend, producer, durability, locking or SQLite error. Cleanup errors preserve both the production and removal failures.
    pub fn get_or_insert_runtime(
        &self,
        key: Key,
        producer: &impl DirectoryProducer,
    ) -> Result<RuntimeLease> {
        loop {
            if let Some(runtime) = self.get_runtime(&key)? {
                return Ok(runtime);
            }
            let store = self.directory_store()?;
            let lock = self.locks.exclusive(DIRECTORY, &key)?;
            let row = self.metadata.directory(&key)?;
            let object = if let Some(row) = row.filter(|r| r.status == EntryStatus::Ready) {
                if store.exists(&row.object)? {
                    self.metadata.touch(DIRECTORY, &key, now_secs()?)?;
                    row.object
                } else {
                    self.produce_runtime(&key, producer)?
                }
            } else {
                self.produce_runtime(&key, producer)?
            };
            retain_shared(&lock, &store.path(&object))?;
            // Conversion may release the old lock: verify both state and reference.
            let valid = self
                .metadata
                .directory(&key)?
                .is_some_and(|r| r.status == EntryStatus::Ready && r.object == object);
            if !valid || !store.exists(&object)? {
                drop(lock);
                continue;
            }
            let runtime = RuntimeLease {
                path: store.path(&object),
                _lock: lock,
            };
            self.after_write()?;
            return Ok(runtime);
        }
    }
    /// Caller holds the logical key exclusively throughout production.
    fn produce_runtime(&self, key: &Key, producer: &impl DirectoryProducer) -> Result<Key> {
        if self.metadata.directory(key)?.is_some() {
            self.remove_directory(key)?;
        }
        // The directory reference is metadata, independent of the user field schema.
        let object = *key;
        self.metadata.begin_build(key, &object, now_secs()?)?;
        info!(%key, %object, "runtime Building");
        let store = self.directory_store()?;
        let result = (|| {
            store.prepare(&object)?;
            let bytes = producer.produce(&store.path(&object))?;
            self.metadata.publish_directory(key, bytes, now_secs()?)
        })();
        if let Err(production) = result {
            warn!(%key, error = %production, "runtime production failed; cleaning partial output");
            return match self.remove_directory(key) {
                Ok(()) => Err(production),
                Err(cleanup) => Err(Error::Cleanup {
                    production: Box::new(production),
                    cleanup: Box::new(cleanup),
                }),
            };
        }
        info!(%key, "runtime Ready");
        Ok(object)
    }
    /// Caller holds the logical key exclusively. Failed deletion retains Pruning.
    pub(crate) fn remove_directory(&self, key: &Key) -> Result<()> {
        let Some(row) = self.metadata.directory(key)? else {
            return Ok(());
        };
        if row.status != EntryStatus::Pruning {
            self.metadata.mark_pruning(key, None)?;
        }
        debug!(%key, object = %row.object, "runtime Pruning committed before filesystem deletion");
        self.directory_store()?.remove(&row.object)?;
        self.metadata.forget_directory(key)
    }
}
