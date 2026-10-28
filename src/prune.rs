use crate::{Error, Key, Result, Store, cache::now_secs, metadata::BLOB};
use crate::{ExtraOnDiskData, InMemoryCache};
use tracing::{debug, info};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub triggered: bool,
    pub removed: usize,
    pub skipped_busy: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}
impl<I: InMemoryCache, D: ExtraOnDiskData> Store<I, D> {
    /// The threshold triggers pruning; it is not a hard quota. Recent and leased
    /// entries survive even if accounted size remains larger than the threshold.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid time/size data or failed metadata, locking or filesystem operations.
    pub fn prune_if_needed(&self) -> Result<PruneReport> {
        let bytes = self.accounted_size()?;
        if bytes <= self.config.disk.prune_trigger_bytes {
            return Ok(PruneReport {
                bytes_before: bytes,
                bytes_after: bytes,
                ..Default::default()
            });
        }
        let retention =
            i64::try_from(self.config.disk.prune_older_than_secs).map_err(|_| Error::Config {
                field: "disk.prune_older_than_secs",
                reason: "exceeds timestamp range",
            })?;
        let deadline = now_secs()?.saturating_sub(retention);
        self.prune_before(deadline)
    }
    /// Prune all entries last accessed strictly before a Unix-second deadline,
    /// independent of the configured size trigger. Future deadlines are rejected.
    ///
    /// # Errors
    ///
    /// Returns an error for a future deadline or failed metadata, locking or filesystem operations. Failed deletion remains recoverable.
    pub fn prune_before(&self, deadline: i64) -> Result<PruneReport> {
        if deadline > now_secs()? {
            return Err(Error::FutureDeadline);
        }
        let mut report = PruneReport {
            triggered: true,
            bytes_before: self.accounted_size()?,
            ..Default::default()
        };
        info!(
            deadline,
            bytes = report.bytes_before,
            "cache pruning started"
        );
        let mut retired = Vec::new();
        for row in self.metadata.candidates(deadline)? {
            let Some(_lock) = self.locks.try_exclusive(row.kind, &row.key)? else {
                report.skipped_busy += 1;
                continue;
            };
            if self.prune_key(row.kind, &row.key, deadline, &mut retired)? {
                report.removed += 1;
            }
        }
        drop(retired);
        report.bytes_after = self.accounted_size()?;
        info!(
            removed = report.removed,
            skipped_busy = report.skipped_busy,
            bytes = report.bytes_after,
            "cache pruning completed"
        );
        Ok(report)
    }
    fn prune_key(
        &self,
        kind: i64,
        key: &Key,
        deadline: i64,
        retired: &mut Vec<std::sync::Arc<crate::MemoryEntry<I::Payload>>>,
    ) -> Result<bool> {
        if kind == BLOB {
            if self.metadata.prune_blob(key, deadline)? {
                retired.extend(self.memory.remove(key));
                return Ok(true);
            }
        } else if self.metadata.mark_pruning(key, Some(deadline))? {
            debug!(%key, "runtime Pruning");
            self.remove_directory(key)?;
            return Ok(true);
        }
        Ok(false)
    }
}
