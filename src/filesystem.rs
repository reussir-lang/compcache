use crate::{Error, Key, Result, error::IoContext};
use fs4::fs_std::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

pub(crate) fn ensure_local(path: &Path) -> Result<()> {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        use std::os::fd::AsRawFd;
        let file = File::open(path).at("open filesystem probe", path)?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: fstatfs initializes stat on success; file's descriptor is live.
        if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error()).at("inspect filesystem", path);
        }
        // SAFETY: the successful call above initialized the structure.
        let kind = unsafe { stat.assume_init() }.f_type;
        if !is_local_kind(kind) {
            return Err(Error::FileSystem {
                path: path.into(),
                kind,
            });
        }
        Ok(())
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    {
        let _ = path;
        Err(Error::UnsupportedPlatform)
    }
}
fn is_local_kind(kind: i64) -> bool {
    matches!(
        kind,
        0xef53 | 0x58465342 | 0x9123683e | 0x2fc12fc1 | 0x01021994 | 0x858458f6 | 0xf2f52010
    )
}
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .at("open directory for sync", path)?
        .sync_all()
        .at("sync directory", path)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_network_and_unknown_filesystems() {
        for kind in [
            0x6969, 0xff534d42, 0xfe534d42, 0x65735546, 0x00c36400, 0x01021997, 0,
        ] {
            assert!(!is_local_kind(kind)); // NFS, SMB, CIFS, FUSE, Ceph, 9p, unknown.
        }
        assert!(is_local_kind(0x2fc12fc1));
    }
}
