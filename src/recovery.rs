use crate::{EntryStatus, Key, Result, Store, metadata::DIRECTORY};
use crate::{ExtraOnDiskData, InMemoryCache};
use tracing::{debug, info};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub removed: usize,
    pub skipped_busy: usize,
}
impl<I: InMemoryCache, D: ExtraOnDiskData> Store<I, D> {
    /// Recovery never waits on live producers/readers. Call again after a crash
    /// if this handle outlives another process; opening a new handle also recovers.
    pub fn recover(&self) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        for row in self.metadata.directories()? {
            if self.recover_key(&row.key)? {
                report.removed += 1;
            } else if row.status != EntryStatus::Ready {
                report.skipped_busy += 1;
            }
        }
        if report.removed > 0 {
            info!(removed = report.removed, "cache recovery completed");
        }
        Ok(report)
    }
    pub(crate) fn recover_key(&self, key: &Key) -> Result<bool> {
        let store = self.directory_store()?;
        let Some(_lock) = self.locks.try_exclusive(DIRECTORY, key)? else {
            debug!(%key, "recovery skipped live key lease");
            return Ok(false);
        };
        let Some(row) = self.metadata.directory(key)? else {
            return Ok(false);
        };
        if row.status == EntryStatus::Ready && store.exists(&row.object)? {
            return Ok(false);
        }
        info!(%key, status = ?row.status, "recovering incomplete or missing runtime");
        self.remove_directory(key)?;
        Ok(true)
    }
}
