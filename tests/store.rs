mod common;
use common::*;
use compcache::{
    Bypass, EntryKind, EntryStatus, Error, Key, MemoryEntry, NoExtraData, QuickCache, StoreBuilder,
};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn sqlite_blobs_persist_without_external_layer() {
    let root = tempfile::tempdir().unwrap();
    let key = Key::from_parts("bytecode", b"compiler-v1/recipe");
    {
        let store = StoreBuilder::<Bypass, NoExtraData>::new()
            .path(root.path())
            .config(config())
            .open()
            .unwrap();
        assert_eq!(
            &*store
                .get_or_insert_blob(key, || Ok(b"bytecode".to_vec()))
                .unwrap(),
            b"bytecode"
        );
        assert!(matches!(
            store.get_runtime(&key),
            Err(Error::DirectoriesDisabled)
        ));
    }
    let store = StoreBuilder::<Bypass, NoExtraData>::new()
        .path(root.path())
        .open()
        .unwrap();
    assert_eq!(&*store.get_blob(&key).unwrap().unwrap(), b"bytecode");
    assert!(!root.path().join("objects").exists());
    let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
}

#[test]
fn parallel_handles_produce_a_blob_once() {
    let root = tempfile::tempdir().unwrap();
    let key = Key::from_parts("code", b"one");
    let calls = AtomicUsize::new(0);
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let root = root.path();
            let calls = &calls;
            let barrier = &barrier;
            scope.spawn(move || {
                let store = open(root);
                barrier.wait();
                let bytes = store
                    .get_or_insert_blob(key, || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![42; 4096])
                    })
                    .unwrap();
                assert_eq!(bytes.len(), 4096);
            });
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn ready_runtime_keeps_a_reader_lease_and_separate_namespace() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let key = Key::from_parts("recipe", b"same");
    store.get_or_insert_blob(key, || Ok(vec![1])).unwrap();
    let runtime = store.get_or_insert_runtime(key, &produce).unwrap();
    assert_eq!(
        std::fs::read(runtime.path().join("lib/runtime.so")).unwrap(),
        b"complete runtime"
    );
    assert_eq!(
        store.status(EntryKind::Directory, &key).unwrap(),
        Some(EntryStatus::Ready)
    );
    age(root.path(), 1, &key);
    let report = store.prune_before(now() - 10).unwrap();
    assert_eq!(report.skipped_busy, 1);
    assert!(runtime.path().exists());
    drop(runtime);
    assert_eq!(store.prune_before(now() - 10).unwrap().removed, 1);
    assert!(store.get_runtime(&key).unwrap().is_none());
    assert!(store.get_blob(&key).unwrap().is_some());
}

#[test]
fn parallel_runtime_production_is_deduplicated() {
    let root = tempfile::tempdir().unwrap();
    let calls = AtomicUsize::new(0);
    let barrier = Barrier::new(4);
    let key = Key::from_parts("runtime", b"same");
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let root = root.path();
            let calls = &calls;
            let barrier = &barrier;
            scope.spawn(move || {
                let store = open(root);
                barrier.wait();
                let lease = store
                    .get_or_insert_runtime(key, &|path: &std::path::Path| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        produce(path)
                    })
                    .unwrap();
                assert!(lease.path().join("lib/runtime.so").exists());
            });
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn quickcache_enforces_weight_and_is_shared_safely() {
    let config = compcache::MemoryConfig {
        max_entry_weight: 64,
        weight_capacity: 256,
        shards: Some(1),
        ..Default::default()
    };
    let memory = QuickCache::new(&config).unwrap();
    let key = Key::from_parts("memory", b"x");
    memory.insert(key, Arc::new(MemoryEntry::new(vec![0; 100].into(), now())));
    assert!(memory.get(&key).is_none());
    memory.insert(key, Arc::new(MemoryEntry::new(vec![0; 32].into(), now())));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let memory = &memory;
            scope.spawn(move || {
                for _ in 0..1000 {
                    assert_eq!(memory.get(&key).unwrap().payload.len(), 32);
                }
            });
        }
    });
    assert!(memory.hits() >= 8000);
    assert!(memory.weight() <= 256);
}

#[test]
fn producer_errors_preserve_their_type_and_allow_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let key = Key::from_parts("runtime", b"fail");
    let error = store
        .get_or_insert_runtime(key, &|_: &std::path::Path| {
            Err(Error::producer(std::io::Error::other("compile failed")))
        })
        .unwrap_err();
    match error {
        Error::Producer(e) => assert!(e.downcast_ref::<std::io::Error>().is_some()),
        _ => panic!("wrong error"),
    }
    assert_eq!(store.status(EntryKind::Directory, &key).unwrap(), None);
    assert!(store.get_or_insert_runtime(key, &produce).is_ok());
}

#[test]
fn invalid_config_is_a_typed_error_instead_of_a_panic() {
    let mut cfg = config();
    cfg.memory.hot_allocation = f64::NAN;
    assert!(matches!(
        cfg.validate(),
        Err(Error::Config {
            field: "memory.hot_allocation",
            ..
        })
    ));
    assert!(serde_json::from_str::<compcache::Config>(r#"{"typo":42}"#).is_err());
    assert_ne!(Key::from_parts("a", b"bc"), Key::from_parts("ab", b"c"));
    assert_ne!(
        Key::from_parts("bytecode", b"x"),
        Key::from_parts("runtime", b"x")
    );
}
