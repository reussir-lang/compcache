use std::{io, path::PathBuf, process::ExitStatus};

pub type Result<T> = std::result::Result<T, Error>;
pub type ProducerError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{action} at {path}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("SQLite cache operation failed")]
    Database(#[from] rusqlite::Error),
    #[error("invalid configuration field {field}: {reason}")]
    Config {
        field: &'static str,
        reason: &'static str,
    },
    #[error("invalid quick_cache options")]
    MemoryOptions(#[source] ProducerError),
    #[error("Nickel evaluation failed ({status}): {diagnostic}")]
    Nickel {
        status: ExitStatus,
        diagnostic: String,
    },
    #[error("invalid exported Nickel configuration")]
    ConfigDecode(#[from] serde_json::Error),
    #[error("unsupported or remote filesystem {kind:#x} at {path}")]
    FileSystem { path: PathBuf, kind: i64 },
    #[error("unsupported or remote filesystem {name} at {path}")]
    FileSystemName { path: PathBuf, name: String },
    #[error("this platform does not support the cache filesystem protocol")]
    UnsupportedPlatform,
    #[error("unsupported cache schema version {0}")]
    Schema(i64),
    #[error("SQLite integrity check failed: {0}")]
    Integrity(String),
    #[error("SQLite refused WAL mode")]
    WalUnavailable,
    #[error("unknown cache entry status {0}")]
    UnknownStatus(i64),
    #[error("could not encode serde cache key")]
    KeyEncode(#[from] crate::KeyEncodeError),
    #[error("cache key must be 32 bytes")]
    InvalidKey,
    #[error("cache size exceeds the supported range")]
    SizeOverflow,
    #[error("cache metadata has a negative payload size {0}")]
    NegativeSize(i64),
    #[error("artifact path must contain only normal relative components: {0}")]
    ArtifactPath(PathBuf),
    #[error("runtime path is not a directory: {0}")]
    RuntimePath(PathBuf),
    #[error("the runtime Building row was lost before publication")]
    LostBuild,
    #[error("system timestamp exceeds the SQLite integer range")]
    TimestampOverflow,
    #[error("payload was stored but subsequent pruning failed")]
    Maintenance(#[source] Box<Self>),
    #[error("directory backend root differs from the recorded root {recorded}")]
    DirectoryRoot {
        recorded: PathBuf,
        requested: Option<PathBuf>,
    },
    #[error("directory storage is disabled")]
    DirectoriesDisabled,
    #[error("cache producer failed")]
    Producer(#[source] ProducerError),
    #[error("producer failed and its partial output could not be cleaned up")]
    Cleanup {
        #[source]
        production: Box<Self>,
        cleanup: Box<Self>,
    },
    #[error("prune deadline must not be in the future")]
    FutureDeadline,
    #[error("system clock is before the Unix epoch")]
    Clock(#[from] std::time::SystemTimeError),
}

impl Error {
    pub fn producer(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Producer(Box::new(error))
    }
}

pub(crate) trait IoContext<T> {
    fn at(self, action: &'static str, path: impl Into<PathBuf>) -> Result<T>;
}
impl<T> IoContext<T> for io::Result<T> {
    fn at(self, action: &'static str, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            action,
            path: path.into(),
            source,
        })
    }
}
