mod common;
use common::*;
use compcache::{
    ArtifactWriter, Bypass, EntryKind, EntryStatus, Error, Key, ManagedDirectory, StoreBuilder,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn deadline_is_strict_and_threshold_is_not_a_quota() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config();
    cfg.disk.prune_trigger_bytes = 0;
    let store = StoreBuilder::<Bypass, ManagedDirectory>::new()
        .path(root.path())
        .config(cfg)
        .open()
        .unwrap();
    let key = Key::from_parts("bytecode", b"fresh");
    store.get_or_insert_blob(key, || Ok(vec![1; 4096])).unwrap();
    let report = store.prune_if_needed().unwrap();
    assert!(report.triggered);
    assert_eq!(report.removed, 0);
    assert!(report.bytes_after > 0);
    let deadline = now() - 10;
    rusqlite::Connection::open(root.path().join("cache.db"))
        .unwrap()
        .execute("UPDATE entries SET accessed=?1", [deadline])
        .unwrap();
    assert_eq!(store.prune_before(deadline).unwrap().removed, 0);
    assert_eq!(store.prune_before(deadline + 1).unwrap().removed, 1);
    assert!(matches!(
        store.prune_before(now() + 10),
        Err(Error::FutureDeadline)
    ));
}

#[test]
fn size_uses_metadata_plus_sqlite_lengths_without_double_counting_blobs() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let blob = Key::from_parts("bytecode", b"size");
    let runtime = Key::from_parts("runtime", b"size");
    store
        .get_or_insert_blob(blob, || Ok(vec![0; 1024 * 1024]))
        .unwrap();
    drop(store.get_or_insert_runtime(runtime, &produce).unwrap());
    let sql_size: u64 = ["cache.db", "cache.db-wal", "cache.db-shm"]
        .iter()
        .map(|name| {
            std::fs::metadata(root.path().join(name))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum();
    assert_eq!(
        store.accounted_size().unwrap(),
        sql_size + b"complete runtime".len() as u64
    );
    // Trust reported bytes even if a noncooperating program changes the output.
    std::fs::write(
        root.path()
            .join("objects")
            .join(runtime.to_hex())
            .join("unexpected"),
        vec![0; 8192],
    )
    .unwrap();
    assert_eq!(
        store.accounted_size().unwrap(),
        sql_size + b"complete runtime".len() as u64
    );
}

#[test]
fn deletion_failure_keeps_pruning_visible_until_recovery() {
    let root = tempfile::tempdir().unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let backend = FailingDirectory {
        inner: ManagedDirectory::new(root.path().join("objects")).unwrap(),
        fail: fail.clone(),
    };
    let store = StoreBuilder::<Bypass, FailingDirectory>::new()
        .path(root.path())
        .config(config())
        .extra_on_disk(backend)
        .open()
        .unwrap();
    let key = Key::from_parts("runtime", b"remove-failure");
    drop(store.get_or_insert_runtime(key, &produce).unwrap());
    age(root.path(), 1, &key);
    fail.store(true, Ordering::SeqCst);
    assert!(store.prune_before(now() - 10).is_err());
    assert_eq!(
        store.status(EntryKind::Directory, &key).unwrap(),
        Some(EntryStatus::Pruning)
    );
    assert!(store.get_runtime(&key).unwrap().is_none());
    assert!(store.recover().is_err());
    fail.store(false, Ordering::SeqCst);
    assert_eq!(store.recover().unwrap().removed, 1);
    assert_eq!(store.status(EntryKind::Directory, &key).unwrap(), None);
}

#[test]
fn recover_missing_ready_directory_and_retry_build() {
    let root = tempfile::tempdir().unwrap();
    let key = Key::from_parts("runtime", b"missing");
    let path = {
        let store = open(root.path());
        let lease = store.get_or_insert_runtime(key, &produce).unwrap();
        lease.path().to_path_buf()
    };
    std::fs::remove_dir_all(&path).unwrap();
    let store = open(root.path());
    assert_eq!(store.status(EntryKind::Directory, &key).unwrap(), None);
    assert!(
        store
            .get_or_insert_runtime(key, &produce)
            .unwrap()
            .path()
            .exists()
    );
}

#[test]
fn irreparable_database_and_newer_schema_bail_out() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("cache.db"), b"not a sqlite database").unwrap();
    assert!(matches!(
        StoreBuilder::<Bypass, ManagedDirectory>::new()
            .path(root.path())
            .open(),
        Err(Error::Database(_))
    ));
    std::fs::remove_file(root.path().join("cache.db")).unwrap();
    let store = open(root.path());
    drop(store);
    rusqlite::Connection::open(root.path().join("cache.db"))
        .unwrap()
        .execute_batch("PRAGMA user_version=99")
        .unwrap();
    assert!(matches!(
        StoreBuilder::<Bypass, ManagedDirectory>::new()
            .path(root.path())
            .open(),
        Err(Error::Schema(99))
    ));
}

#[test]
fn counted_writer_rejects_duplicate_and_escaping_paths() {
    let root = tempfile::tempdir().unwrap();
    let mut writer = ArtifactWriter::new(root.path());
    assert!(writer.write_file("../escape", b"x").is_err());
    writer.write_file("a/b.bin", b"12345").unwrap();
    assert!(writer.write_file("a/b.bin", b"new").is_err());
    assert_eq!(writer.finish().unwrap(), 5);
}

#[test]
fn access_touches_metadata_at_second_resolution_and_never_moves_backwards() {
    let root = tempfile::tempdir().unwrap();
    let store = StoreBuilder::<Bypass, ManagedDirectory>::new()
        .path(root.path())
        .config(config())
        .open()
        .unwrap();
    let key = Key::from_parts("bytecode", b"touch");
    store.get_or_insert_blob(key, || Ok(vec![1])).unwrap();
    age(root.path(), 0, &key);
    assert!(store.get_blob(&key).unwrap().is_some());
    assert_eq!(store.prune_before(now() - 10).unwrap().removed, 0);
    let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
    let future = now() + 100;
    db.execute("UPDATE entries SET accessed=?1", [future])
        .unwrap();
    assert!(store.get_blob(&key).unwrap().is_some());
    assert_eq!(
        db.query_row("SELECT accessed FROM entries", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        future
    );
    assert!(store.checkpoint().unwrap());
}

#[test]
fn backend_root_cannot_silently_change_for_an_existing_store() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = open(root.path());
    drop(store);
    let backend = ManagedDirectory::new(other.path()).unwrap();
    assert!(matches!(
        StoreBuilder::<Bypass, ManagedDirectory>::new()
            .path(root.path())
            .extra_on_disk(backend)
            .open(),
        Err(Error::DirectoryRoot { .. })
    ));
}

#[test]
fn runtime_location_is_resolved_from_sqlite_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let key = Key::from_parts("runtime", b"location");
    let original = {
        let lease = store.get_or_insert_runtime(key, &produce).unwrap();
        lease.path().to_path_buf()
    };
    let object = Key::from_parts("object", b"relocated");
    let relocated = root.path().join("objects").join(object.to_hex());
    std::fs::rename(&original, &relocated).unwrap();
    rusqlite::Connection::open(root.path().join("cache.db"))
        .unwrap()
        .execute(
            "UPDATE entries SET object_key=?1 WHERE kind=1 AND key=?2",
            rusqlite::params![object.as_bytes().as_slice(), key.as_bytes().as_slice()],
        )
        .unwrap();
    let lease = store.get_runtime(&key).unwrap().unwrap();
    assert_eq!(lease.path(), std::fs::canonicalize(&relocated).unwrap());
    assert_eq!(
        std::fs::read(lease.path().join("lib/runtime.so")).unwrap(),
        b"complete runtime"
    );
    drop(lease);
    age(root.path(), 1, &key);
    assert_eq!(store.prune_before(now() - 10).unwrap().removed, 1);
    assert!(!relocated.exists());
}
