//! Mock Polyffi and Rene contracts. No Reussir, LLVM, or linker dependency.
mod common;
use common::{age, config, now};
use compcache::{ArtifactWriter, Bypass, Error, Key, NoExtraData, Produced, Store, StoreBuilder};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Serialize)]
struct PolyffiRecipe {
    source: &'static str,
    options: Vec<&'static str>,
    toolchain: &'static str,
    libraries: Vec<&'static str>,
}
impl PolyffiRecipe {
    fn key(&self) -> Key {
        Key::from_parts("polyffi-v1", &serde_json::to_vec(self).unwrap())
    }
}
fn compile(calls: &AtomicUsize) -> Vec<u8> {
    calls.fetch_add(1, Ordering::SeqCst);
    b"valid-native-object".to_vec()
}
fn load_polyffi(store: &Store, key: Key, calls: &AtomicUsize) -> compcache::Result<Arc<[u8]>> {
    // Only one repair retry: persistently incompatible output cannot loop forever.
    for attempt in 0..2 {
        let bytes = store.get_or_insert_blob(key, || Ok(compile(calls)))?;
        if &*bytes == b"valid-native-object" {
            return Ok(bytes);
        }
        store.force_evict_blob(&key)?;
        if attempt == 1 {
            break;
        }
    }
    Err(Error::producer(std::io::Error::other(
        "invalid cached native object after retry",
    )))
}
#[test]
fn polyffi_counting_compiler_recipe_changes_and_bounded_repair() {
    let root = tempfile::tempdir().unwrap();
    let store = common::open(root.path());
    let calls = AtomicUsize::new(0);
    let recipe = PolyffiRecipe {
        source: "int f() { return 1; }",
        options: vec!["-O2"],
        toolchain: "clang-content-v1",
        libraries: vec!["libc-content-v1"],
    };
    load_polyffi(&store, recipe.key(), &calls).unwrap();
    load_polyffi(&store, recipe.key(), &calls).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut variants = vec![recipe; 4];
    variants[0].source = "int f() { return 2; }";
    variants[1].options.push("-fPIC");
    variants[2].toolchain = "clang-content-v2";
    variants[3].libraries = vec!["libc-content-v2"];
    for changed in variants {
        load_polyffi(&store, changed.key(), &calls).unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    let bad = Key::from_parts("polyffi-v1", b"invalid-cache");
    store
        .get_or_insert_blob(bad, || Ok(b"corrupt".to_vec()))
        .unwrap();
    assert_eq!(
        &*load_polyffi(&store, bad, &calls).unwrap(),
        b"valid-native-object"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert_eq!(
        &*load_polyffi(&store, bad, &calls).unwrap(),
        b"valid-native-object"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6);
}

#[test]
fn polyffi_cache_failure_fallback_is_distinct_from_real_compile_failure() {
    let root = tempfile::tempdir().unwrap();
    let store = common::open(root.path());
    let calls = AtomicUsize::new(0);
    let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_blob BEFORE INSERT ON entries BEGIN SELECT RAISE(FAIL, 'cache failure'); END;").unwrap();
    let result = store
        .get_or_insert_payload(Key::from_parts("polyffi", b"ok"), || {
            let bytes = compile(&calls);
            Ok(Produced {
                payload: Arc::from(bytes.clone()),
                serialized: Ok(Some(bytes)),
            })
        })
        .unwrap();
    // Fallback uses the already compiled object rather than invoking the compiler twice.
    assert!(matches!(result.cache_error, Some(Error::Database(_))));
    assert_eq!(&*result.payload, b"valid-native-object");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let error = store
        .get_or_insert_payload(Key::from_parts("polyffi", b"fail"), || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(Box::new(std::io::Error::other("invalid C source")) as compcache::ProducerError)
        })
        .unwrap_err();
    assert!(matches!(error, Error::Producer(_)));
    // A real compiler error is propagated, never treated as a cache-only fallback.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // A cache lookup failure occurs before production; callers may compile once outside it.
    db.execute_batch("DROP TABLE entries;").unwrap();
    let result = store.get_or_insert_payload(Key::from_parts("polyffi", b"lookup-failure"), || {
        panic!("lookup must fail before compile")
    });
    assert!(matches!(result, Err(Error::Database(_))));
    assert_eq!(compile(&calls), b"valid-native-object");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[derive(Clone, Serialize)]
struct BakeRecipe {
    bundle: &'static str,
    toolchain: &'static str,
    target: &'static str,
}
impl BakeRecipe {
    fn key(&self) -> Key {
        Key::from_parts("rene-bake-v1", &serde_json::to_vec(self).unwrap())
    }
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    runtime: String,
    dependencies: Vec<String>,
}
fn bake(path: &Path, calls: &AtomicUsize) -> compcache::Result<u64> {
    calls.fetch_add(1, Ordering::SeqCst);
    let mut writer = ArtifactWriter::new(path);
    writer.write_file("lib/runtime.a", b"dummy runtime archive")?;
    writer.write_file("lib/dependency.a", b"dummy dependency archive")?;
    let manifest = Manifest {
        runtime: "lib/runtime.a".into(),
        dependencies: vec!["lib/dependency.a".into()],
    };
    writer.write_file("manifest.json", &serde_json::to_vec(&manifest).unwrap())?;
    writer.finish()
}
#[test]
fn rene_relative_manifest_reopen_build_directory_reuse_and_linker_lease() {
    let root = tempfile::tempdir().unwrap();
    let build_a = tempfile::tempdir().unwrap();
    let build_b = tempfile::tempdir().unwrap();
    let recipe = BakeRecipe {
        bundle: "runtime-bundle-content-v1",
        toolchain: "clang-v1",
        target: "x86_64-linux",
    };
    let calls = AtomicUsize::new(0);
    let first_path = {
        let store = common::open(root.path());
        let lease = store
            .get_or_insert_runtime(recipe.key(), &|path: &Path| bake(path, &calls))
            .unwrap();
        std::fs::write(
            build_a.path().join("linked-output"),
            std::fs::read(lease.path().join("lib/runtime.a")).unwrap(),
        )
        .unwrap();
        lease.path().to_path_buf()
    };
    let store = common::open(root.path());
    let lease = store
        .get_or_insert_runtime(recipe.key(), &|path: &Path| bake(path, &calls))
        .unwrap();
    assert_eq!(lease.path(), first_path);
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(lease.path().join("manifest.json")).unwrap())
            .unwrap();
    for relative in [&manifest.runtime, &manifest.dependencies[0]] {
        assert!(Path::new(relative).is_relative());
        assert!(!relative.contains(".."));
        assert!(lease.path().join(relative).is_file());
    }
    age(root.path(), 1, &recipe.key());
    assert_eq!(store.prune_before(now() - 10).unwrap().skipped_busy, 1);
    // Simulated linker reads while holding its runtime lease, after a prune attempt.
    let linked = [
        std::fs::read(lease.path().join(&manifest.runtime)).unwrap(),
        std::fs::read(lease.path().join(&manifest.dependencies[0])).unwrap(),
    ]
    .concat();
    std::fs::write(build_b.path().join("linked-output"), linked).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Blob force eviction never deletes leased runtime directories, even at the same key.
    store.force_evict_blob(&recipe.key()).unwrap();
    assert!(lease.path().is_dir());
    drop(lease);
    assert_eq!(store.prune_before(now() - 10).unwrap().removed, 1);
    let mut variants = vec![recipe; 3];
    variants[0].bundle = "runtime-bundle-content-v2";
    variants[1].toolchain = "clang-v2";
    variants[2].target = "aarch64-linux";
    for changed in variants {
        store
            .get_or_insert_runtime(changed.key(), &|path: &Path| bake(path, &calls))
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[test]
fn byte_defaults_work_with_disabled_admission() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config();
    cfg.memory.enabled = false;
    let store = StoreBuilder::<Bypass, NoExtraData>::new()
        .path(root.path())
        .config(cfg)
        .open()
        .unwrap();
    let key = Key::from_parts("bytes", b"default");
    assert_eq!(
        &*store.get_or_insert_blob(key, || Ok(vec![42])).unwrap(),
        &[42]
    );
    assert_eq!(&*store.get_blob(&key).unwrap().unwrap(), &[42]);
}
