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

/// Optional managed payload backend. All methods are called under the key lease.
/// Paths and layout must be stable across processes and restarts. `prepare` must
/// not destroy a Ready entry. `remove` must be idempotent and sync the parent.
/// All data must be on the validated local filesystem rooted at `root`.
pub trait ExtraOnDiskData: Send + Sync {
    /// `None` selects a backend without external payloads.
    fn from_config(root: PathBuf, config: &crate::DiskConfig) -> Result<Option<Self>>
    where
        Self: Sized;
    fn root(&self) -> &Path;
    fn path(&self, key: &Key) -> PathBuf;
    fn prepare(&self, key: &Key) -> Result<()>;
    fn remove(&self, key: &Key) -> Result<()>;
    fn exists(&self, key: &Key) -> Result<bool>;
}
/// A producer must complete all writes, sync files and directories, and report
/// the total logical bytes it produced. Never modify output after returning.
pub trait DirectoryProducer {
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
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        make_directory(&root)?;
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
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Count and sync produced files without walking the tree. This helper supports
/// streamed compiler output; external tools may instead use the producer contract.
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
    pub fn write_file(&mut self, relative: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
        self.copy_file(relative, &mut &*bytes)
    }
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
        let parent = path.parent().expect("joined path has a parent");
        self.backoff.run("create artifact parents", parent, || {
            fs::create_dir_all(parent)
        })?;
        let mut dir = parent;
        while dir.starts_with(&self.root) {
            self.directories.insert(dir.to_path_buf());
            if dir == self.root {
                break;
            }
            dir = dir.parent().expect("nested directory has parent");
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
    pub fn finish(self) -> Result<u64> {
        for directory in self.directories.iter().rev() {
            sync_directory(directory)?;
        }
        Ok(self.bytes)
    }
}

/// A type-level choice to omit external payloads completely.
pub enum NoExtraData {}
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
