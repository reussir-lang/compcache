use compcache::{
    ArtifactWriter, Bypass, Config, Key, ManagedDirectory, MemoryEntry, NoExtraData, QuickCache,
    SchemaKey, Store, StoreBuilder,
};
use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn config() -> Config {
    let mut config = Config::default();
    config.disk.prune_on_write = false;
    config
}
#[allow(
    clippy::significant_drop_tightening,
    reason = "Criterion's finish consumes and drops its benchmark group"
)]
fn blobs(c: &mut Criterion) {
    let mut group = c.benchmark_group("warm_blob");
    for size in [64usize, 4096, 262_144] {
        let root = tempfile::tempdir().unwrap();
        let store: Store = StoreBuilder::new()
            .path(root.path())
            .config(config())
            .open()
            .unwrap();
        let key = Key::from_parts("bench", &size.to_le_bytes());
        store.get_or_insert_blob(key, || Ok(vec![7; size])).unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| black_box(store.get_blob(black_box(&key)).unwrap()));
        });
    }
    group.finish();
    let root = tempfile::tempdir().unwrap();
    let store = StoreBuilder::<Bypass, NoExtraData>::new()
        .path(root.path())
        .config(config())
        .open()
        .unwrap();
    let key = Key::from_parts("bench", b"sqlite");
    store.get_or_insert_blob(key, || Ok(vec![7; 4096])).unwrap();
    c.bench_function("sqlite_hit_4k", |b| {
        b.iter(|| black_box(store.get_blob(&key).unwrap()));
    });
    let mut sequence = 0u64;
    c.bench_function("sqlite_insert_4k", |b| {
        b.iter(|| {
            sequence += 1;
            let key = Key::from_parts("bench", &sequence.to_le_bytes());
            black_box(store.get_or_insert_blob(key, || Ok(vec![7; 4096])).unwrap());
        });
    });
}
struct BenchKey<'a> {
    values: [&'a str; 4],
}
impl SchemaKey<4> for BenchKey<'_> {
    const KEY_FIELDS: [&'static str; 4] = ["compiler", "target", "source", "flags"];
    fn key_value(&self) -> [&str; 4] {
        self.values
    }
}
#[derive(serde::Serialize)]
struct SerdeBenchKey<'a> {
    compiler: &'a str,
    target: &'a str,
    inputs: &'a [[u8; 32]],
    optimization: u8,
}
#[allow(
    clippy::significant_drop_tightening,
    reason = "Criterion's finish consumes and drops its benchmark group"
)]
fn memory(c: &mut Criterion) {
    let recipe = BenchKey {
        values: ["v1", "x86_64", "source-digest", "-O2"],
    };
    c.bench_function("schema_key_4_fields", |b| {
        b.iter(|| black_box(Key::new(black_box(&recipe))));
    });

    let inputs = [[42u8; 32]; 16];
    let mut serde_keys = c.benchmark_group("serde_key");
    for count in [0, 1, 16] {
        let recipe = SerdeBenchKey {
            compiler: "v1",
            target: "x86_64",
            inputs: &inputs[..count],
            optimization: 2,
        };
        serde_keys.bench_with_input(BenchmarkId::from_parameter(count), &recipe, |b, recipe| {
            b.iter(|| black_box(Key::from_serde("bytecode-v1", black_box(recipe)).unwrap()));
        });
    }
    serde_keys.finish();
    let cache = QuickCache::new(&config().memory).unwrap();
    let key = Key::from_parts("bench", b"memory");
    cache.insert(key, Arc::new(MemoryEntry::new(vec![7; 4096].into(), 0)));
    c.bench_function("quickcache_hit_4k", |b| {
        b.iter(|| black_box(cache.get(black_box(&key))));
    });
    c.bench_function("quickcache_miss", |b| {
        b.iter(|| black_box(cache.get(black_box(&Key::from_bytes([255; 32])))));
    });
    let root = tempfile::tempdir().unwrap();
    let store: Store = StoreBuilder::new()
        .path(root.path())
        .config(config())
        .open()
        .unwrap();
    let keys: Vec<_> = (0u64..1024)
        .map(|i| Key::from_parts("bench", &i.to_le_bytes()))
        .collect();
    for key in &keys {
        store
            .get_or_insert_blob(*key, || Ok(vec![1; 4096]))
            .unwrap();
    }
    let mut group = c.benchmark_group("shared_store");
    // Thread setup is outside the measured interval; joins and all reads are inside.
    // Different workers have disjoint key ranges, avoiding a single-key Arc hotspot.
    for workers in [1usize, 4, 16] {
        group.throughput(Throughput::Elements(workers as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(workers),
            &workers,
            |b, &workers| {
                b.iter_custom(|iterations| {
                    std::thread::scope(|scope| {
                        let barrier = Arc::new(std::sync::Barrier::new(workers + 1));
                        let mut handles = Vec::new();
                        for worker in 0..workers {
                            let barrier = barrier.clone();
                            let store = &store;
                            let keys = &keys;
                            handles.push(scope.spawn(move || {
                                barrier.wait();
                                for i in 0..iterations {
                                    black_box(
                                        store
                                            .get_blob(
                                                &keys[(worker * 64
                                                    + usize::try_from(i % 64).unwrap())
                                                    % keys.len()],
                                            )
                                            .unwrap(),
                                    );
                                }
                            }));
                        }
                        let started = Instant::now();
                        barrier.wait();
                        for handle in handles {
                            handle.join().unwrap();
                        }
                        started.elapsed()
                    })
                });
            },
        );
    }
    group.finish();
}
fn directories(c: &mut Criterion) {
    let root = tempfile::tempdir().unwrap();
    let store = StoreBuilder::<Bypass, ManagedDirectory>::new()
        .path(root.path())
        .config(config())
        .open()
        .unwrap();
    let key = Key::from_parts("bench", b"runtime");
    let producer = |path: &std::path::Path| {
        let mut writer = ArtifactWriter::new(path);
        writer.write_file("runtime.so", &[1; 1024])?;
        writer.finish()
    };
    drop(store.get_or_insert_runtime(key, &producer).unwrap());
    c.bench_function("runtime_reader_lease", |b| {
        b.iter(|| black_box(store.get_runtime(&key).unwrap()));
    });
    c.bench_function("prune_100_directories", |b| {
        b.iter_batched_ref(
            || {
                let root = tempfile::tempdir().unwrap();
                let store = StoreBuilder::<Bypass, ManagedDirectory>::new()
                    .path(root.path())
                    .config(config())
                    .open()
                    .unwrap();
                for i in 0u64..100 {
                    drop(
                        store
                            .get_or_insert_runtime(
                                Key::from_parts("bench", &i.to_le_bytes()),
                                &producer,
                            )
                            .unwrap(),
                    );
                }
                rusqlite::Connection::open(root.path().join("cache.db"))
                    .unwrap()
                    .execute("UPDATE entries SET accessed=0", [])
                    .unwrap();
                (root, store)
            },
            |(_root, store)| {
                black_box(store.prune_before(1).unwrap());
            },
            BatchSize::PerIteration,
        );
    });
}
fn configured() -> Criterion {
    Criterion::default()
        .sample_size(30)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .without_plots()
}
criterion_group! { name = benches; config = configured(); targets = blobs, memory, directories }
criterion_main!(benches);
