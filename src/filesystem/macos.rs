use crate::{Error, Result, error::IoContext};
use std::{ffi::CStr, fs::File, os::fd::AsRawFd, path::Path};

pub(super) fn ensure_local(path: &Path) -> Result<()> {
    let file = File::open(path).at("open filesystem probe", path)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file's descriptor is live and stat points to writable storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).at("inspect filesystem", path);
    }
    // SAFETY: the successful fstatfs call initialized stat, including the
    // NUL-terminated f_fstypename array supplied by the kernel.
    let stat = unsafe { stat.assume_init() };
    let name = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) };
    if !is_local_filesystem(stat.f_flags, name.to_bytes()) {
        return Err(Error::FileSystemName {
            path: path.into(),
            name: name.to_string_lossy().into_owned(),
        });
    }
    Ok(())
}

fn is_local_filesystem(flags: u32, name: &[u8]) -> bool {
    i64::from(flags) & i64::from(libc::MNT_LOCAL) != 0 && matches!(name, b"apfs" | b"hfs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_and_unvalidated_filesystems() {
        let local = u32::try_from(libc::MNT_LOCAL).unwrap();
        assert!(is_local_filesystem(local, b"apfs"));
        assert!(is_local_filesystem(local, b"hfs"));
        assert!(!is_local_filesystem(0, b"apfs"));
        assert!(!is_local_filesystem(local, b"smbfs"));
        assert!(!is_local_filesystem(local, b"unknown"));
    }
}
