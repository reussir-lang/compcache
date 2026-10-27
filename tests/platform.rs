mod common;
use common::*;
use compcache::{Key, Store, StoreBuilder};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

#[test]
fn published_writer_lease_allows_concurrent_readers() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(open(root.path()));
    let key = Key::from_parts("platform", b"shared-lease");
    let first = store.get_or_insert_runtime(key, &produce).unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        send.send(store.get_runtime(&key).unwrap()).unwrap();
    });
    let second = receive.recv_timeout(Duration::from_secs(5));
    // Release before joining even on timeout, so a broken downgrade cannot hang the test.
    drop(first);
    reader.join().unwrap();
    assert!(
        second
            .expect("second reader blocked behind the published writer")
            .is_some()
    );
}

#[test]
fn unicode_root_and_artifacts_survive_reopening() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("编译缓存-🦀");
    let key = Key::from_parts("platform", b"unicode");
    {
        let store: Store = StoreBuilder::new().path(&path).open().unwrap();
        store
            .get_or_insert_blob(key, || Ok(b"complete".to_vec()))
            .unwrap();
        drop(store.get_or_insert_runtime(key, &produce).unwrap());
    }
    let store = open(&path);
    assert_eq!(&*store.get_blob(&key).unwrap().unwrap(), b"complete");
    let lease = store.get_runtime(&key).unwrap().unwrap();
    assert_eq!(
        std::fs::read(lease.path().join("lib/runtime.so")).unwrap(),
        b"complete runtime"
    );
}

#[cfg(windows)]
mod windows {
    use super::*;
    use compcache::{EntryKind, EntryStatus};
    use std::{
        fs::{File, OpenOptions},
        os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
        process::Command,
    };
    use windows_sys::Win32::{
        Foundation::{FreeLibrary, HMODULE},
        Storage::FileSystem::FILE_SHARE_READ,
        System::LibraryLoader::LoadLibraryW,
    };

    fn short_retry_store(path: &std::path::Path) -> Store {
        let mut cfg = config();
        cfg.disk.io_backoff.max_elapsed_ms = 25;
        StoreBuilder::new().path(path).config(cfg).open().unwrap()
    }

    #[test]
    fn held_file_leaves_pruning_recoverable() {
        let root = tempfile::tempdir().unwrap();
        let store = short_retry_store(root.path());
        let key = Key::from_parts("platform", b"scanner");
        let lease = store.get_or_insert_runtime(key, &produce).unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(lease.path().join("lib/runtime.so"))
            .unwrap();
        drop(lease);
        age(root.path(), 1, &key);
        assert!(store.prune_before(now() - 10).is_err());
        assert_eq!(
            store.status(EntryKind::Directory, &key).unwrap(),
            Some(EntryStatus::Pruning)
        );
        assert!(store.get_runtime(&key).unwrap().is_none());
        drop(held);
        assert_eq!(store.recover().unwrap().removed, 1);
    }

    struct Library(HMODULE);
    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: this guard owns exactly the reference returned by LoadLibraryW.
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }

    #[test]
    fn loaded_dll_can_be_pruned_or_recovered_after_unloading() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("fixture.rs");
        let dll = fixture.path().join("fixture.dll");
        std::fs::write(
            &source,
            "#[unsafe(no_mangle)] pub extern \"C\" fn fixture() -> u32 { 7 }",
        )
        .unwrap();
        let output = Command::new("rustc")
            .args(["--crate-type", "cdylib"])
            .arg(&source)
            .arg("-o")
            .arg(&dll)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let root = tempfile::tempdir().unwrap();
        let store = short_retry_store(root.path());
        let key = Key::from_parts("platform", b"loaded-dll");
        let lease = store
            .get_or_insert_runtime(key, &|path: &std::path::Path| {
                let mut writer = compcache::ArtifactWriter::new(path);
                writer.copy_file(
                    "fixture.dll",
                    &mut File::open(&dll).map_err(compcache::Error::producer)?,
                )?;
                writer.finish()
            })
            .unwrap();
        let wide: Vec<u16> = lease
            .path()
            .join("fixture.dll")
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: wide is a live NUL-terminated absolute path to the test DLL.
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        assert!(!module.is_null(), "{}", std::io::Error::last_os_error());
        let library = Library(module);
        drop(lease);
        age(root.path(), 1, &key);
        if store.prune_before(now() - 10).is_err() {
            assert_eq!(
                store.status(EntryKind::Directory, &key).unwrap(),
                Some(EntryStatus::Pruning)
            );
        }
        drop(library);
        store.recover().unwrap();
        assert!(store.get_runtime(&key).unwrap().is_none());
    }
}
