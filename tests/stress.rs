mod common;
use common::*;
use compcache::{
    ArtifactWriter, Bypass, ExtraOnDiskData, InMemoryCache, Key, ManagedDirectory, QuickCache,
    Store, StoreBuilder,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Report {
    operations: usize,
    runtime_reads: usize,
    blob_reads: usize,
    pruned: usize,
    skipped_busy: usize,
}
fn blob_key(index: u8) -> Key {
    Key::from_parts("stress-bytecode-v1", &[index])
}
fn runtime_key(index: u8) -> Key {
    Key::from_parts("stress-runtime-v1", &[index])
}
fn next_random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn exercise<I: InMemoryCache, D: ExtraOnDiskData>(
    store: Store<I, D>,
    id: u64,
    seconds: u64,
) -> Report {
    let started = Instant::now();
    let mut seed = id + 1729;
    let mut report = Report::default();
    while started.elapsed() < Duration::from_secs(seconds) {
        let random = next_random(&mut seed);
        if id >= 4 {
            let prune = store.prune_before(now()).unwrap();
            report.pruned += prune.removed;
            report.skipped_busy += prune.skipped_busy;
            store.recover().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        } else if random.is_multiple_of(3) {
            let index = (random % 48) as u8;
            let bytes = store
                .get_or_insert_blob(blob_key(index), || Ok(vec![index; 4096]))
                .unwrap();
            assert_eq!(bytes.as_ref(), vec![index; 4096]);
            report.blob_reads += 1;
        } else {
            let index = (random % 16) as u8;
            let lease = store
                .get_or_insert_runtime(runtime_key(index), &|path: &Path| {
                    let mut output = ArtifactWriter::new(path);
                    output.write_file("code/image", &vec![index; 8192])?;
                    // Leave a genuine partial-build interval for concurrent open/recovery.
                    std::thread::sleep(Duration::from_millis(2));
                    output.write_file("manifest", &[index, 1])?;
                    output.finish()
                })
                .unwrap();
            assert_eq!(
                std::fs::read(lease.path().join("code/image")).unwrap(),
                vec![index; 8192]
            );
            // Leases sometimes cross access-second/pruning boundaries.
            if random.is_multiple_of(8) {
                std::thread::sleep(Duration::from_millis(25));
            }
            assert_eq!(
                std::fs::read(lease.path().join("manifest")).unwrap(),
                [index, 1]
            );
            report.runtime_reads += 1;
        }
        report.operations += 1;
    }
    report
}

#[test]
#[ignore]
fn stress_worker() {
    let root = std::path::PathBuf::from(
        std::env::var_os("COMPCACHE_STRESS_ROOT").expect("parent supplies root"),
    );
    let id: u64 = std::env::var("COMPCACHE_STRESS_ID")
        .unwrap()
        .parse()
        .unwrap();
    let seconds = std::env::var("COMPCACHE_STRESS_SECONDS")
        .unwrap()
        .parse()
        .unwrap();
    let mut cfg = config();
    cfg.disk.prune_older_than_secs = 0;
    let report = if id.is_multiple_of(2) {
        let store = StoreBuilder::<QuickCache, ManagedDirectory>::new()
            .path(&root)
            .config(cfg)
            .open()
            .unwrap();
        exercise(store, id, seconds)
    } else {
        let store = StoreBuilder::<Bypass, ManagedDirectory>::new()
            .path(&root)
            .config(cfg)
            .open()
            .unwrap();
        exercise(store, id, seconds)
    };
    std::fs::write(
        root.join(format!("stress-{id}.json")),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
}
struct Workers(Vec<Child>);
impl Drop for Workers {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn six_processes_share_a_store_with_live_builds_readers_pruning_and_recovery() {
    let root = tempfile::tempdir().unwrap();
    let seconds: u64 = std::env::var("COMPCACHE_STRESS_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
        .max(3);
    let mut workers = Workers(Vec::new());
    for id in 0..6 {
        workers.0.push(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "stress_worker", "--ignored", "--nocapture"])
                .env("COMPCACHE_STRESS_ROOT", root.path())
                .env("COMPCACHE_STRESS_ID", id.to_string())
                .env("COMPCACHE_STRESS_SECONDS", seconds.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
    }
    let started = Instant::now();
    loop {
        let mut done = true;
        for child in &mut workers.0 {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "stress worker failed");
            } else {
                done = false;
            }
        }
        if done {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(seconds + 30),
            "stress workers timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut total = Report::default();
    for id in 0..6 {
        let report: Report = serde_json::from_slice(
            &std::fs::read(root.path().join(format!("stress-{id}.json"))).unwrap(),
        )
        .unwrap();
        println!("process {id}: {report:?}");
        total.operations += report.operations;
        total.runtime_reads += report.runtime_reads;
        total.blob_reads += report.blob_reads;
        total.pruned += report.pruned;
        total.skipped_busy += report.skipped_busy;
    }
    assert!(total.operations > 100 && total.runtime_reads > 20 && total.blob_reads > 20);
    assert!(total.pruned > 0, "stress must actually exercise pruning");
    let store = open(root.path());
    for index in 0u8..16 {
        if let Some(lease) = store.get_runtime(&runtime_key(index)).unwrap() {
            assert_eq!(
                std::fs::read(lease.path().join("code/image")).unwrap(),
                vec![index; 8192]
            );
            assert_eq!(
                std::fs::read(lease.path().join("manifest")).unwrap(),
                [index, 1]
            );
        }
    }
    let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM entries WHERE state<>2", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    println!("federated stress total ({seconds}s): {total:?}");
    if let Some(path) = std::env::var_os("COMPCACHE_STRESS_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&total).unwrap()).unwrap();
    }
}
