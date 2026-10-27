use crate::{Error, Result, error::IoContext};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
};
use windows_sys::Win32::{
    Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, GetDriveTypeW, GetVolumePathNameW},
    System::WindowsProgramming::{DRIVE_NO_ROOT_DIR, DRIVE_REMOTE, DRIVE_UNKNOWN},
};

pub(super) fn open_directory(path: &Path, writable: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

pub(super) fn ensure_local(path: &Path) -> Result<()> {
    let canonical = fs::canonicalize(path).at("resolve filesystem probe", path)?;
    let wide: Vec<u16> = canonical.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut volume = vec![0u16; 32768];
    // SAFETY: wide is NUL-terminated; volume has the advertised writable length.
    if unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), 32768) } == 0 {
        return Err(io::Error::last_os_error()).at("inspect volume path", path);
    }
    // SAFETY: GetVolumePathNameW successfully NUL-terminated volume.
    let drive = unsafe { GetDriveTypeW(volume.as_ptr()) };
    if is_rejected_drive(drive) {
        return Err(Error::FileSystemName {
            path: path.into(),
            name: format!("drive type {drive}"),
        });
    }
    Ok(())
}

fn is_rejected_drive(drive: u32) -> bool {
    matches!(drive, DRIVE_REMOTE | DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE,
    };

    #[test]
    fn rejects_remote_and_unknown_drives_without_checking_filesystem_names() {
        for drive in [DRIVE_FIXED, DRIVE_REMOVABLE, DRIVE_RAMDISK, DRIVE_CDROM] {
            assert!(!is_rejected_drive(drive));
        }
        assert!(is_rejected_drive(DRIVE_REMOTE));
        assert!(is_rejected_drive(DRIVE_UNKNOWN));
        assert!(is_rejected_drive(DRIVE_NO_ROOT_DIR));
    }
}
