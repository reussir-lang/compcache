#![allow(dead_code)]
use compcache::{
    ArtifactWriter, Config, ExtraOnDiskData, Key, ManagedDirectory, Result, Store, StoreBuilder,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub fn config() -> Config {
    let mut config = Config::default();
    config.disk.prune_on_write = false;
    config
}
pub fn open(root: &Path) -> Store {
    StoreBuilder::new()
        .path(root)
        .config(config())
        .open()
        .unwrap()
}
pub fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}
pub fn age(root: &Path, kind: i64, key: &Key) {
    rusqlite::Connection::open(root.join("cache.db"))
        .unwrap()
        .execute(
            "UPDATE entries SET accessed=?1 WHERE kind=?2 AND key=?3",
            rusqlite::params![now() - 100, kind, key.as_bytes().as_slice()],
        )
        .unwrap();
}
pub fn produce(path: &Path) -> Result<u64> {
    let mut writer = ArtifactWriter::new(path);
    writer.write_file("lib/runtime.so", b"complete runtime")?;
    writer.finish()
}

pub struct FailingDirectory {
    pub inner: ManagedDirectory,
    pub fail: Arc<AtomicBool>,
}
impl ExtraOnDiskData for FailingDirectory {
    fn from_config(root: PathBuf, _config: &compcache::DiskConfig) -> Result<Option<Self>> {
        Ok(Some(Self {
            inner: ManagedDirectory::new(root)?,
            fail: Arc::new(AtomicBool::new(false)),
        }))
    }
    fn root(&self) -> &Path {
        self.inner.root()
    }
    fn path(&self, key: &Key) -> PathBuf {
        self.inner.path(key)
    }
    fn prepare(&self, key: &Key) -> Result<()> {
        self.inner.prepare(key)
    }
    fn exists(&self, key: &Key) -> Result<bool> {
        self.inner.exists(key)
    }
    fn remove(&self, key: &Key) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            Err(compcache::Error::producer(std::io::Error::other(
                "injected removal failure",
            )))
        } else {
            self.inner.remove(key)
        }
    }
}
