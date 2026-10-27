use crate::{Error, Result, error::IoContext};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::{
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, GetDriveTypeW, GetVolumeInformationByHandleW,
        GetVolumePathNameW,
    },
    System::WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE},
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
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .at("open filesystem probe", path)?;
    let mut name = [0u16; 32];
    // SAFETY: the live file handle and writable filesystem-name buffer are valid;
    // unused outputs are explicitly documented as optional.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error()).at("inspect filesystem", path);
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let name = String::from_utf16_lossy(&name[..end]);
    if !is_local_volume(drive, &name) {
        return Err(Error::FileSystemName {
            path: path.into(),
            name: format!("{name} (drive type {drive})"),
        });
    }
    Ok(())
}

fn is_local_volume(drive: u32, name: &str) -> bool {
    matches!(drive, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK) && name == "NTFS"
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

    #[test]
    fn rejects_remote_and_unvalidated_filesystems() {
        assert!(is_local_volume(DRIVE_FIXED, "NTFS"));
        assert!(!is_local_volume(DRIVE_REMOTE, "NTFS"));
        assert!(!is_local_volume(DRIVE_FIXED, "FAT32"));
        assert!(!is_local_volume(DRIVE_FIXED, "ReFS"));
        assert!(!is_local_volume(0, "NTFS"));
    }
}
