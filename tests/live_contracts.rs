//! Deterministic Reussir-shaped contracts, not LLVM, PJRT, or Reussir integration.
mod common;
use common::{age, config, now};
use compcache::{
    Cached, Config, EntryKind, Error, InMemoryCache, Key, MemoryConfig, MemoryEntry, NoExtraData,
    Produced, ProducerError, QuickCache, Result, Store, StoreBuilder,
};
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Default, Debug)]
struct Counts {
    compile: AtomicUsize,
    serialize: AtomicUsize,
    deserialize: AtomicUsize,
    destroy: AtomicUsize,
    client_destroy: AtomicUsize,
}
#[derive(Debug)]
struct Client {
    counts: Arc<Counts>,
    drop_check: Mutex<Option<(std::path::PathBuf, Key)>>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.counts.client_destroy.fetch_add(1, Ordering::SeqCst);
    }
}
#[derive(Debug)]
struct Executable {
    client: Arc<Client>,
    code: Vec<u8>,
}
impl Drop for Executable {
    fn drop(&mut self) {
        if let Some((root, key)) = &*self.client.drop_check.lock().unwrap() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.join("locks").join(format!("0-{key}.lock")))
                .unwrap();
            assert!(
                fs4::fs_std::FileExt::try_lock_exclusive(&file).unwrap(),
                "destructor ran under key lease"
            );
            let db = rusqlite::Connection::open(root.join("cache.db")).unwrap();
            db.busy_timeout(std::time::Duration::ZERO).unwrap();
            db.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
        }
        assert_eq!(self.client.counts.client_destroy.load(Ordering::SeqCst), 0);
        self.client.counts.destroy.fetch_add(1, Ordering::SeqCst);
    }
}
impl Client {
    fn compile(self: &Arc<Self>) -> Arc<Executable> {
        self.counts.compile.fetch_add(1, Ordering::SeqCst);
        Arc::new(Executable {
            client: self.clone(),
            code: b"executable-v1".to_vec(),
        })
    }
    fn produce(self: &Arc<Self>, fail_serialization: bool) -> Produced<Arc<Executable>> {
        let payload = self.compile();
        self.counts.serialize.fetch_add(1, Ordering::SeqCst);
        let serialized = if fail_serialization {
            Err(Error::producer(std::io::Error::other("serialize failed")))
        } else {
            Ok(Some(payload.code.clone()))
        };
        Produced {
            payload,
            serialized,
        }
    }
}
struct Gate {
    entered: Barrier,
    release: Barrier,
}
impl Gate {
    fn new() -> Self {
        Self {
            entered: Barrier::new(2),
            release: Barrier::new(2),
        }
    }
}
struct ClientMemory {
    client: Arc<Client>,
    entries: QuickCache<Arc<Executable>>,
    bypass: bool,
    admission_gate: Mutex<Option<Arc<Gate>>>,
}
impl InMemoryCache for ClientMemory {
    type Payload = Arc<Executable>;
    fn from_config(_: &MemoryConfig) -> Result<Self> {
        Err(Error::Config {
            field: "client",
            reason: "provide client context with in_memory",
        })
    }
    fn materialize(&self, bytes: Arc<[u8]>) -> Result<Self::Payload> {
        self.client
            .counts
            .deserialize
            .fetch_add(1, Ordering::SeqCst);
        if &*bytes != b"executable-v1" {
            return Err(Error::producer(std::io::Error::other("invalid executable")));
        }
        Ok(Arc::new(Executable {
            client: self.client.clone(),
            code: bytes.to_vec(),
        }))
    }
    fn weight(&self, _: &Self::Payload) -> u64 {
        128
    }
    fn get(&self, key: &Key) -> Option<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.get(key)
    }
    fn insert(
        &self,
        key: Key,
        value: Arc<MemoryEntry<Self::Payload>>,
    ) -> Vec<Arc<MemoryEntry<Self::Payload>>> {
        let gate = self.admission_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.entered.wait();
            gate.release.wait();
        }
        if self.bypass {
            vec![value]
        } else {
            self.entries.insert(key, value)
        }
    }
    fn remove(&self, key: &Key) -> Option<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.remove(key)
    }
    fn clear(&self) -> Vec<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.clear()
    }
}
type ClientStore = Store<ClientMemory, NoExtraData>;
fn client() -> Arc<Client> {
    Arc::new(Client {
        counts: Arc::default(),
        drop_check: Mutex::new(None),
    })
}
fn open(
    root: &Path,
    client: &Arc<Client>,
    cfg: Config,
    bypass: bool,
    gate: Option<Arc<Gate>>,
) -> ClientStore {
    let memory = ClientMemory {
        client: client.clone(),
        entries: QuickCache::new(&cfg.memory).unwrap(),
        bypass,
        admission_gate: Mutex::new(gate),
    };
    StoreBuilder::new()
        .path(root)
        .config(cfg)
        .in_memory(memory)
        .open()
        .unwrap()
}
fn count(counter: &AtomicUsize) -> usize {
    counter.load(Ordering::SeqCst)
}
fn key() -> Key {
    Key::from_parts("pjrt", b"recipe")
}
fn cached(store: &ClientStore, client: &Arc<Client>) -> Cached<Arc<Executable>> {
    store
        .get_or_insert_payload(key(), || Ok(client.produce(false)))
        .unwrap()
}

#[test]
fn pjrt_cold_original_concurrent_hits_and_client_scoped_warm_materialization() {
    let root = tempfile::tempdir().unwrap();
    let a = client();
    let store = open(root.path(), &a, config(), false, None);
    let original = a.compile();
    let cold = store
        .get_or_insert_payload(key(), || {
            a.counts.serialize.fetch_add(1, Ordering::SeqCst);
            Ok(Produced {
                payload: original.clone(),
                serialized: Ok(Some(original.code.clone())),
            })
        })
        .unwrap()
        .payload;
    assert!(Arc::ptr_eq(&original, &cold));
    assert_eq!(count(&a.counts.deserialize), 0);
    let start = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                start.wait();
                assert!(Arc::ptr_eq(&cold, &cached(&store, &a).payload));
            });
        }
    });
    assert_eq!(count(&a.counts.compile), 1);
    assert_eq!(count(&a.counts.serialize), 1);
    drop(store);
    let warm = open(root.path(), &a, config(), false, None);
    let loaded = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                start.wait();
                let value = cached(&warm, &a).payload;
                loaded.lock().unwrap().push(value);
            });
        }
    });
    let loaded = loaded.into_inner().unwrap();
    assert!(loaded.iter().all(|v| Arc::ptr_eq(v, &loaded[0])));
    assert!(!Arc::ptr_eq(&cold, &loaded[0]));
    assert_eq!(count(&a.counts.deserialize), 1);
    let b = client();
    let other = open(root.path(), &b, config(), false, None);
    let other_value = cached(&other, &b).payload;
    assert!(!Arc::ptr_eq(&other_value, &loaded[0]));
    assert!(Arc::ptr_eq(&other_value.client, &b));
    assert_eq!(count(&b.counts.compile), 0);
    assert_eq!(count(&b.counts.deserialize), 1);
    assert_eq!(count(&a.counts.compile), 1);
}

#[test]
fn pjrt_concurrent_cold_requests_compile_once() {
    let root = tempfile::tempdir().unwrap();
    let client = client();
    let store = open(root.path(), &client, config(), false, None);
    let start = Barrier::new(8);
    let results = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                start.wait();
                let value = cached(&store, &client).payload;
                results.lock().unwrap().push(value);
            });
        }
    });
    let results = results.into_inner().unwrap();
    assert!(results.iter().all(|v| Arc::ptr_eq(v, &results[0])));
    assert_eq!(count(&client.counts.compile), 1);
    assert_eq!(count(&client.counts.deserialize), 0);
}

#[test]
fn pjrt_serialization_and_persistence_failures_preserve_memory_only_success() {
    for fail_serialization in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let client = client();
        let store = open(root.path(), &client, config(), false, None);
        let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
        if !fail_serialization {
            db.execute_batch("CREATE TRIGGER reject_blob BEFORE INSERT ON entries BEGIN SELECT RAISE(FAIL, 'injected persistence failure'); END;").unwrap();
        }
        let result = store
            .get_or_insert_payload(key(), || Ok(client.produce(fail_serialization)))
            .unwrap();
        assert!(result.cache_error.is_some());
        assert_eq!(result.payload.code, b"executable-v1");
        assert_eq!(store.status(EntryKind::Blob, &key()).unwrap(), None);
        let hit = store
            .get_or_insert_payload(key(), || panic!("memory-only hit must not compile"))
            .unwrap();
        assert!(Arc::ptr_eq(&result.payload, &hit.payload));
        assert_eq!(count(&client.counts.compile), 1);
        store.force_evict_blob(&key()).unwrap();
        assert!(store.get_blob(&key()).unwrap().is_none());
        assert_eq!(count(&client.counts.destroy), 0);
    }
}

#[test]
fn pjrt_disabled_bypass_and_oversize_keep_hooks_and_caller_value() {
    for mode in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let client = client();
        let mut cfg = config();
        if mode == 0 {
            cfg.memory.enabled = false;
        }
        if mode == 2 {
            cfg.memory.max_entry_weight = 64;
        }
        let store = open(root.path(), &client, cfg, mode == 1, None);
        let original = cached(&store, &client).payload;
        assert_eq!(count(&client.counts.deserialize), 0);
        let hit = cached(&store, &client).payload;
        assert_eq!(hit.code, original.code);
        assert!(!Arc::ptr_eq(&hit, &original));
        assert_eq!(count(&client.counts.deserialize), 1);
        store.force_evict_blob(&key()).unwrap();
        assert_eq!(original.code, b"executable-v1");
        assert_eq!(count(&client.counts.destroy), 0);
    }
}

#[test]
fn pjrt_external_arc_survives_eviction_prune_and_store_drop() {
    for mode in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let client = client();
        let counts = client.counts.clone();
        let store = open(root.path(), &client, config(), false, None);
        let value = cached(&store, &client).payload;
        if mode == 0 {
            store.force_evict_blob(&key()).unwrap();
        }
        if mode == 1 {
            age(root.path(), 0, &key());
            assert_eq!(store.prune_before(now() - 10).unwrap().removed, 1);
        }
        drop(store);
        drop(client);
        assert_eq!(value.code, b"executable-v1");
        assert_eq!(count(&counts.destroy), 0);
        assert_eq!(count(&counts.client_destroy), 0);
        drop(value);
        assert_eq!(count(&counts.destroy), 1);
        assert_eq!(count(&counts.client_destroy), 1);
    }
}

#[test]
fn force_evict_waits_for_publication_and_final_admission() {
    for gate_admission in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let client = client();
        let gate = Arc::new(Gate::new());
        let store = open(
            root.path(),
            &client,
            config(),
            false,
            gate_admission.then(|| gate.clone()),
        );
        let eviction_started = Barrier::new(2);
        std::thread::scope(|scope| {
            let producer = scope.spawn(|| {
                store
                    .get_or_insert_payload(key(), || {
                        if !gate_admission {
                            gate.entered.wait();
                            gate.release.wait();
                        }
                        Ok(client.produce(false))
                    })
                    .unwrap()
                    .payload
            });
            gate.entered.wait();
            // The producer/admission owns the existing filesystem lease.
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.path().join("locks").join(format!("0-{}.lock", key())))
                .unwrap();
            assert!(!fs4::fs_std::FileExt::try_lock_exclusive(&lock).unwrap());
            let evict = scope.spawn(|| {
                eviction_started.wait();
                store.force_evict_blob(&key()).unwrap();
            });
            eviction_started.wait();
            gate.release.wait();
            let value = producer.join().unwrap();
            evict.join().unwrap();
            assert_eq!(value.code, b"executable-v1");
            assert!(store.get_blob(&key()).unwrap().is_none());
            assert_eq!(store.status(EntryKind::Blob, &key()).unwrap(), None);
        });
        // A genuinely new request may repopulate.
        assert_eq!(cached(&store, &client).payload.code, b"executable-v1");
        assert_eq!(count(&client.counts.compile), 2);
    }
}

#[derive(Clone, Serialize)]
struct PjrtRecipe {
    program: &'static str,
    shape: Vec<usize>,
    donate: bool,
    target: &'static str,
}
#[test]
fn pjrt_recipe_shape_and_donation_are_persistent_but_client_is_not() {
    let root = tempfile::tempdir().unwrap();
    let client = client();
    let store = open(root.path(), &client, config(), false, None);
    let recipe = PjrtRecipe {
        program: "add",
        shape: vec![2, 4],
        donate: false,
        target: "mock-cpu-v1",
    };
    let mut shape = recipe.clone();
    shape.shape = vec![8];
    let mut donate = recipe.clone();
    donate.donate = true;
    for recipe in [&recipe, &recipe, &shape, &donate] {
        let key = Key::from_parts("pjrt-v1", &serde_json::to_vec(recipe).unwrap());
        store
            .get_or_insert_payload(key, || Ok(client.produce(false)))
            .unwrap();
    }
    assert_eq!(count(&client.counts.compile), 3);
}

#[test]
fn true_producer_failure_has_no_success_payload_and_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let client = client();
    let store = open(root.path(), &client, config(), false, None);
    let error = store
        .get_or_insert_payload(key(), || {
            Err(Box::new(std::io::Error::other("compile failed")) as ProducerError)
        })
        .unwrap_err();
    assert!(matches!(error, Error::Producer(_)));
    assert!(store.get_blob(&key()).unwrap().is_none());
    assert_eq!(cached(&store, &client).payload.code, b"executable-v1");
}

#[test]
fn memory_only_without_serialization_and_other_store_snapshot_survive_disk_eviction() {
    let root = tempfile::tempdir().unwrap();
    let client = client();
    let first = open(root.path(), &client, config(), false, None);
    let second = open(root.path(), &client, config(), false, None);
    let value = cached(&first, &client).payload;
    let snapshot = cached(&second, &client).payload;
    first.force_evict_blob(&key()).unwrap();
    assert!(first.get_blob(&key()).unwrap().is_none());
    assert!(Arc::ptr_eq(
        &snapshot,
        &second.get_blob(&key()).unwrap().unwrap()
    ));
    let memory_only = first
        .get_or_insert_payload(key(), || {
            Ok(Produced {
                payload: value.clone(),
                serialized: Ok(None),
            })
        })
        .unwrap();
    assert!(memory_only.cache_error.is_none());
    assert_eq!(first.status(EntryKind::Blob, &key()).unwrap(), None);
    assert!(Arc::ptr_eq(
        &value,
        &first.get_blob(&key()).unwrap().unwrap()
    ));
    assert_eq!(count(&client.counts.serialize), 1);
}

#[test]
fn invalid_disk_materialization_is_a_cache_error_and_explicit_repair_is_bounded() {
    let root = tempfile::tempdir().unwrap();
    let bytes = StoreBuilder::<compcache::Bypass, NoExtraData>::new()
        .path(root.path())
        .config(config())
        .open()
        .unwrap();
    bytes
        .get_or_insert_blob(key(), || Ok(b"invalid".to_vec()))
        .unwrap();
    let client = client();
    let store = open(root.path(), &client, config(), false, None);
    let error = store
        .get_or_insert_payload(key(), || panic!("disk materialization precedes production"))
        .unwrap_err();
    assert!(matches!(error, Error::Materialize(_)));
    store.force_evict_blob(&key()).unwrap();
    let value = cached(&store, &client).payload;
    assert_eq!(value.code, b"executable-v1");
    assert_eq!(count(&client.counts.compile), 1);
    assert_eq!(count(&client.counts.deserialize), 1);
}

#[test]
fn native_cleanup_runs_after_key_and_sqlite_locks_are_released() {
    for prune in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let client = client();
        let store = open(root.path(), &client, config(), false, None);
        drop(cached(&store, &client));
        *client.drop_check.lock().unwrap() = Some((root.path().to_path_buf(), key()));
        if prune {
            age(root.path(), 0, &key());
            assert_eq!(store.prune_before(now() - 10).unwrap().removed, 1);
        } else {
            store.force_evict_blob(&key()).unwrap();
        }
        assert_eq!(count(&client.counts.destroy), 1);
    }
}

#[test]
fn quick_cache_defers_all_capacity_evictions_replacements_and_clear_drops() {
    struct Probe(Arc<AtomicUsize>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let cfg = MemoryConfig {
        weight_capacity: 128,
        max_entry_weight: 128,
        shards: Some(1),
        estimated_items_capacity: 128,
        ..Default::default()
    };
    let memory = QuickCache::new(&cfg).unwrap();
    let mut retired = Vec::new();
    for index in 0..64_u8 {
        retired.extend(memory.insert(
            Key::from_parts("probe", &[index]),
            Arc::new(MemoryEntry::with_weight(Probe(drops.clone()), 0, 1)),
        ));
    }
    let large = Key::from_parts("probe", b"large");
    retired.extend(memory.insert(
        large,
        Arc::new(MemoryEntry::with_weight(Probe(drops.clone()), 0, 100)),
    ));
    assert!(
        retired.len() > 2,
        "exercise more than DefaultLifecycle's two slots"
    );
    retired.extend(memory.insert(
        large,
        Arc::new(MemoryEntry::with_weight(Probe(drops.clone()), 0, 100)),
    ));
    retired.extend(memory.clear());
    assert!(memory.is_empty());
    assert_eq!(count(&drops), 0);
    drop(retired);
    assert_eq!(count(&drops), 66);
}

#[test]
fn capacity_eviction_defers_native_cleanup_until_after_new_keys_admission() {
    let root = tempfile::tempdir().unwrap();
    let client = client();
    let mut cfg = config();
    cfg.memory.weight_capacity = 192;
    cfg.memory.max_entry_weight = 128;
    cfg.memory.shards = Some(1);
    let store = open(root.path(), &client, cfg, false, None);
    drop(cached(&store, &client));
    let second = Key::from_parts("pjrt", b"second-recipe");
    *client.drop_check.lock().unwrap() = Some((root.path().to_path_buf(), second));
    let value = store
        .get_or_insert_payload(second, || Ok(client.produce(false)))
        .unwrap();
    assert_eq!(count(&client.counts.destroy), 1);
    assert_eq!(value.payload.code, b"executable-v1");
}
