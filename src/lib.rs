//! Synchronous compiler cache with SQLite WAL metadata, immutable blobs and
//! optional leased runtime directories. No worker threads or async runtime.
//!
//! Keys identify deterministic recipes; include compiler version and target.
//! Install a `tracing` subscriber in your application for lifecycle diagnostics.
mod backoff;
mod builder;
mod cache;
mod config;
mod directory;
mod error;
mod filesystem;
mod key;
mod memory;
mod metadata;
mod prune;
mod recovery;
mod runtime;
mod schema;

pub use backoff::Backoff;
pub use builder::StoreBuilder;
pub use cache::Store;
pub use config::{Config, DiskConfig, MemoryConfig};
pub use directory::{
    ArtifactWriter, DirectoryProducer, ExtraOnDiskData, ManagedDirectory, NoExtraData, RuntimeLease,
};
pub use error::{Error, ProducerError, Result};
pub use key::{Key, SchemaKey};
pub use memory::{Bypass, InMemoryCache, MemoryEntry, QuickCache};
pub use metadata::{EntryKind, EntryStatus};
pub use prune::PruneReport;
pub use recovery::RecoveryReport;
