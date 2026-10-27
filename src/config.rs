use crate::{Error, Result, error::IoContext};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub memory: MemoryConfig,
    pub disk: DiskConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub estimated_items_capacity: usize,
    /// Payload bytes plus the 32-byte key; excludes allocator bookkeeping.
    pub weight_capacity: u64,
    pub shards: Option<usize>,
    pub hot_allocation: f64,
    pub ghost_allocation: f64,
    pub max_entry_weight: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiskConfig {
    pub io_backoff: crate::Backoff,
    pub prune_trigger_bytes: u64,
    /// At pruning time, deadline = current Unix second - retention seconds.
    pub prune_older_than_secs: u64,
    pub prune_on_write: bool,
    pub directories: bool,
    pub busy_timeout_ms: u64,
    pub sqlite_cache_kib: u32,
    pub wal_autocheckpoint_pages: u32,
}
impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            estimated_items_capacity: 4096,
            weight_capacity: 64 << 20,
            shards: None,
            hot_allocation: 0.97,
            ghost_allocation: 0.5,
            max_entry_weight: 8 << 20,
        }
    }
}
impl Default for DiskConfig {
    fn default() -> Self {
        Self {
            io_backoff: crate::Backoff::default(),
            prune_trigger_bytes: 4 << 30,
            prune_older_than_secs: 7 * 24 * 3600,
            prune_on_write: true,
            directories: true,
            busy_timeout_ms: 5000,
            sqlite_cache_kib: 8192,
            wal_autocheckpoint_pages: 1000,
        }
    }
}
impl Config {
    /// Evaluate Nickel once during startup; the hot path never starts a process.
    /// Requires the `nickel` executable from the development shell or deployment.
    pub fn from_nickel(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let output = Command::new("nickel")
            .args(["export", "--format", "json"])
            .arg(path)
            .output()
            .at("evaluate Nickel configuration", path)?;
        if !output.status.success() {
            return Err(Error::Nickel {
                status: output.status,
                diagnostic: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let config: Self = serde_json::from_slice(&output.stdout)?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        let m = &self.memory;
        let retry = &self.disk.io_backoff;
        if retry.initial_delay_ms == 0
            || retry.max_delay_ms < retry.initial_delay_ms
            || retry.max_delay_ms > 60_000
        {
            return Err(Error::Config {
                field: "disk.io_backoff",
                reason: "delays must satisfy 0 < initial <= max <= 60000 ms",
            });
        }

        for (field, valid, reason) in [
            (
                "memory.estimated_items_capacity",
                m.estimated_items_capacity > 0,
                "must be positive",
            ),
            (
                "memory.weight_capacity",
                m.weight_capacity > 0,
                "must be positive",
            ),
            (
                "memory.shards",
                m.shards.is_none_or(|s| s > 0 && s <= 65536),
                "must be in 1..=65536",
            ),
            (
                "memory.hot_allocation",
                (0.0..=1.0).contains(&m.hot_allocation),
                "must be in 0..=1",
            ),
            (
                "memory.ghost_allocation",
                (0.0..=1.0).contains(&m.ghost_allocation),
                "must be in 0..=1",
            ),
            (
                "disk.prune_trigger_bytes",
                self.disk.prune_trigger_bytes <= i64::MAX as u64,
                "exceeds SQLite integer range",
            ),
            (
                "disk.prune_older_than_secs",
                self.disk.prune_older_than_secs <= i64::MAX as u64,
                "exceeds timestamp range",
            ),
            (
                "disk.sqlite_cache_kib",
                self.disk.sqlite_cache_kib > 0 && self.disk.sqlite_cache_kib <= i32::MAX as u32,
                "must fit a positive i32",
            ),
        ] {
            if !valid {
                return Err(Error::Config { field, reason });
            }
        }
        Ok(())
    }
}
