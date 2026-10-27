use crate::{
    Error, Key, Result,
    error::IoContext,
    filesystem::{make_directory, sync_directory},
};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

/// Optional managed payload backend.
///
/// All methods are called under the key lease.
///
/// Paths and layout must be stable across processes and restarts. `prepare` must
/// not destroy a Ready entry. `remove` must be idempotent and sync the parent.
/// All data must be on the validated local filesystem rooted at `root`.
pub trait ExtraOnDiskData: Send + Sync {
    /// `None` selects a backend without external payloads.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend cannot initialize its configured storage.
    fn from_config(root: PathBuf, config: &crate::DiskConfig) -> Result<Option<Self>>
    where
        Self: Sized;
    fn root(&self) -> &Path;
    fn path(&self, key: &Key) -> PathBuf;
    /// Prepare an empty directory while holding the key exclusively.
    ///
    /// # Errors
    ///
    /// Returns an error if an empty build directory cannot be prepared and synced.
    fn prepare(&self, key: &Key) -> Result<()>;
    /// Remove output idempotently and sync its parent.
    ///
    /// # Errors
    ///
    /// Returns an error if deletion or parent syncing fails; a later call must be able to resume.
    fn remove(&self, key: &Key) -> Result<()>;
    /// Inspect whether published output is present.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be inspected or the path has an invalid type.
    fn exists(&self, key: &Key) -> Result<bool>;
}
/// A producer must complete all writes, sync files and directories, and report
/// the total logical bytes it produced. Never modify output after returning.
pub trait DirectoryProducer {
    /// Produce complete, synced output and return its logical byte count.
    ///
    /// # Errors
    ///
    /// Returns a production or durability error. The cache will attempt to remove partial output.
    fn produce(&self, directory: &Path) -> Result<u64>;
}
impl<F: Fn(&Path) -> Result<u64>> DirectoryProducer for F {
    fn produce(&self, directory: &Path) -> Result<u64> {
        self(directory)
    }
}

pub struct ManagedDirectory {
    root: PathBuf,
    backoff: crate::Backoff,
}
impl ManagedDirectory {
    /// Create and canonicalize the managed storage root.
    ///
    /// # Errors
    ///
    /// Returns an error if the root cannot be created or is on unsupported storage.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        make_directory(&root)?;
        let root = fs::canonicalize(&root).at("resolve managed directory root", &root)?;
        Ok(Self {
            root,
            backoff: crate::Backoff::default(),
        })
    }
}
impl ExtraOnDiskData for ManagedDirectory {
    fn from_config(root: PathBuf, config: &crate::DiskConfig) -> Result<Option<Self>> {
        let mut store = Self::new(root)?;
        store.backoff = config.io_backoff.clone();
        Ok(Some(store))
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, key: &Key) -> PathBuf {
        self.root.join(key.to_hex())
    }
    fn prepare(&self, key: &Key) -> Result<()> {
        let path = self.path(key);
        self.backoff
            .run("prepare runtime directory", &path, || fs::create_dir(&path))?;
        sync_directory(&self.root)
    }
    fn remove(&self, key: &Key) -> Result<()> {
        let path = self.path(key);
        self.backoff.run(
            "remove runtime directory",
            &path,
            || match fs::remove_dir_all(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
        )?;
        sync_directory(&self.root)
    }
    fn exists(&self, key: &Key) -> Result<bool> {
        let path = self.path(key);
        match fs::metadata(&path) {
            Ok(m) if m.is_dir() => Ok(true),
            Ok(_) => Err(Error::RuntimePath(path)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).at("inspect runtime directory", path),
        }
    }
}

/// A leased path stays complete until this value is dropped. Do not retain a
/// borrowed path for later use or mutate cached files.
#[derive(Debug)]
pub struct RuntimeLease {
    pub(crate) path: PathBuf,
    pub(crate) _lock: File,
}
impl RuntimeLease {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Count and sync produced files without walking the tree. This helper supports
/// streamed compiler output; external tools may instead use the producer contract.
#[must_use = "finish the writer to sync its directories and obtain the byte count"]
pub struct ArtifactWriter {
    root: PathBuf,
    directories: BTreeSet<PathBuf>,
    bytes: u64,
    backoff: crate::Backoff,
}
impl ArtifactWriter {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            directories: BTreeSet::from([root.clone()]),
            root,
            bytes: 0,
            backoff: crate::Backoff::default(),
        }
    }
    pub fn with_backoff(mut self, backoff: crate::Backoff) -> Self {
        self.backoff = backoff;
        self
    }
    /// Create a new artifact from bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or existing path, failed writes/syncs, or size overflow.
    pub fn write_file(&mut self, relative: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
        self.copy_file(relative, &mut &*bytes)
    }
    /// Stream an input into a new artifact and sync it.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or existing path, failed reads/writes/syncs, or size overflow.
    pub fn copy_file(&mut self, relative: impl AsRef<Path>, input: &mut impl Read) -> Result<()> {
        let relative = relative.as_ref();
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(Error::ArtifactPath(relative.to_path_buf()));
        }
        let path = self.root.join(relative);
        let parent = path
            .parent()
            .ok_or_else(|| Error::ArtifactPath(relative.to_path_buf()))?;
        self.backoff.run("create artifact parents", parent, || {
            fs::create_dir_all(parent)
        })?;
        let mut dir = parent;
        while dir.starts_with(&self.root) {
            self.directories.insert(dir.to_path_buf());
            if dir == self.root {
                break;
            }
            let Some(parent) = dir.parent() else { break };
            dir = parent;
        }
        let mut file = self.backoff.run("create artifact file", &path, || {
            OpenOptions::new().write(true).create_new(true).open(&path)
        })?;
        let bytes = std::io::copy(input, &mut file).at("write artifact", &path)?;
        file.flush().at("flush artifact", &path)?;
        self.backoff
            .run("sync artifact", &path, || file.sync_all())?;
        self.bytes = self.bytes.checked_add(bytes).ok_or(Error::SizeOverflow)?;
        Ok(())
    }
    /// Sync produced directories and return the total bytes written.
    ///
    /// # Errors
    ///
    /// Returns an error if a produced directory cannot be synced.
    pub fn finish(self) -> Result<u64> {
        for directory in self.directories.iter().rev() {
            sync_directory(directory)?;
        }
        Ok(self.bytes)
    }
}

/// A type-level choice to omit external payloads completely.
pub enum NoExtraData {}
#[allow(
    clippy::uninhabited_references,
    reason = "NoExtraData has no instances; these trait methods are unreachable"
)]
impl ExtraOnDiskData for NoExtraData {
    fn from_config(_: PathBuf, _config: &crate::DiskConfig) -> Result<Option<Self>> {
        Ok(None)
    }
    fn root(&self) -> &Path {
        match *self {}
    }
    fn path(&self, _: &Key) -> PathBuf {
        match *self {}
    }
    fn prepare(&self, _: &Key) -> Result<()> {
        match *self {}
    }
    fn remove(&self, _: &Key) -> Result<()> {
        match *self {}
    }
    fn exists(&self, _: &Key) -> Result<bool> {
        match *self {}
    }
}
