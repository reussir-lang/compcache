use crate::{Key, Result, error::IoContext};
use fs4::fs_std::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[path = "filesystem/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "filesystem/macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "filesystem/windows.rs"]
mod platform;

pub(crate) fn ensure_local(path: &Path) -> Result<()> {
    #[cfg(any(
        all(target_os = "linux", target_pointer_width = "64"),
        target_os = "macos",
        windows
    ))]
    {
        platform::ensure_local(path)
    }
    #[cfg(not(any(
        all(target_os = "linux", target_pointer_width = "64"),
        target_os = "macos",
        windows
    )))]
    {
        let _ = path;
        Err(crate::Error::UnsupportedPlatform)
    }
}
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let directory = platform::open_directory(path, true);
    #[cfg(not(windows))]
    let directory = File::open(path);
    directory
        .at("open directory for sync", path)?
        .sync_all()
        .at("sync directory", path)
}

pub(crate) fn retain_shared(lock: &File, path: &Path) -> Result<()> {
    FileExt::lock_shared(lock).at("retain runtime reader lease", path)?;
    // LockFileEx stacks the shared lock on the exclusive lock. Its first unlock
    // releases the exclusive lock, retaining the shared lock without a gap.
    #[cfg(windows)]
    FileExt::unlock(lock).at("release runtime writer lease", path)?;
    Ok(())
}
pub(crate) fn make_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).at("create cache directory", path)?;
    ensure_local(path)
}

pub(crate) struct KeyLocks {
    root: PathBuf,
}
impl KeyLocks {
    pub(crate) fn new(root: PathBuf) -> Result<Self> {
        make_directory(&root)?;
        Ok(Self { root })
    }
    fn open(&self, kind: i64, key: &Key) -> Result<File> {
        let path = self.root.join(format!("{kind}-{key}.lock"));
        // Lock files must persist: unlinking one can create two independent lock domains.
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .at("open key lock", path)
    }
    pub(crate) fn shared(&self, kind: i64, key: &Key) -> Result<File> {
        let file = self.open(kind, key)?;
        FileExt::lock_shared(&file).at("acquire shared key lease", &self.root)?;
        Ok(file)
    }
    pub(crate) fn exclusive(&self, kind: i64, key: &Key) -> Result<File> {
        let file = self.open(kind, key)?;
        FileExt::lock_exclusive(&file).at("acquire exclusive key lease", &self.root)?;
        Ok(file)
    }
    pub(crate) fn try_exclusive(&self, kind: i64, key: &Key) -> Result<Option<File>> {
        let file = self.open(kind, key)?;
        if FileExt::try_lock_exclusive(&file).at("try exclusive key lease", &self.root)? {
            Ok(Some(file))
        } else {
            Ok(None)
        }
    }
}
