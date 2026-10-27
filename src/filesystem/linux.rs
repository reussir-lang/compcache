use crate::{Error, Result, error::IoContext};
use std::{fs::File, os::fd::AsRawFd, path::Path};

pub(super) fn ensure_local(path: &Path) -> Result<()> {
    let file = File::open(path).at("open filesystem probe", path)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file's descriptor is live and stat points to writable storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).at("inspect filesystem", path);
    }
    // SAFETY: the successful fstatfs call initialized stat.
    let kind = unsafe { stat.assume_init() }.f_type;
    if !is_local_kind(kind) {
        return Err(Error::FileSystem {
            path: path.into(),
            kind,
        });
    }
    Ok(())
}

fn is_local_kind(kind: i64) -> bool {
    matches!(
        kind,
        0xef53 | 0x5846_5342 | 0x9123_683e | 0x2fc1_2fc1 | 0x0102_1994 | 0x8584_58f6 | 0xf2f5_2010
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_network_and_unknown_filesystems() {
        for kind in [
            0x6969,
            0xff53_4d42,
            0xfe53_4d42,
            0x6573_5546,
            0x00c3_6400,
            0x0102_1997,
            0,
        ] {
            assert!(!is_local_kind(kind));
        }
        assert!(is_local_kind(0x2fc1_2fc1));
    }
}
