mod common;
use common::*;
use compcache::{
    Bypass, EntryKind, EntryStatus, ExtraOnDiskData, Key, ManagedDirectory, Result, StoreBuilder,
};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn key() -> Key {
    #[derive(serde::Serialize)]
    struct Recipe {
        compiler: &'static str,
        target: &'static str,
        input: u64,
    }
    Key::from_serde(
        "process-runtime-v1",
        &Recipe {
            compiler: "v1",
            target: "x86_64",
            input: 1729,
        },
    )
    .unwrap()
}
fn signal_and_wait(root: &Path) -> ! {
    std::fs::write(root.join("worker-ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}
struct InterruptedRemoval {
    inner: ManagedDirectory,
    signal: PathBuf,
    after: bool,
}
impl ExtraOnDiskData for InterruptedRemoval {
    fn from_config(_: PathBuf, _config: &compcache::DiskConfig) -> Result<Option<Self>> {
        unreachable!("test supplies an instance")
    }
    fn root(&self) -> &Path {
        self.inner.root()
    }
    fn path(&self, k: &Key) -> PathBuf {
        self.inner.path(k)
    }
    fn prepare(&self, k: &Key) -> Result<()> {
        self.inner.prepare(k)
    }
    fn exists(&self, k: &Key) -> Result<bool> {
        self.inner.exists(k)
    }
    fn remove(&self, k: &Key) -> Result<()> {
        if self.after {
            std::fs::remove_file(self.path(k).join("lib/runtime.so")).unwrap();
        }
        signal_and_wait(&self.signal)
    }
}

// Invoked only as a subprocess; parent tests kill it at a durable boundary.
#[test]
#[ignore]
fn process_worker() {
    let root = PathBuf::from(std::env::var_os("COMPCACHE_PROCESS_ROOT").expect("parent sets root"));
    match std::env::var("COMPCACHE_PROCESS_MODE").unwrap().as_str() {
        "build" => {
            let store = open(&root);
            store
                .get_or_insert_runtime(key(), &|path: &Path| {
                    std::fs::write(path.join("partial"), b"partial").unwrap();
                    signal_and_wait(&root)
                })
                .unwrap();
        }
        mode @ ("prune-before" | "prune-during") => {
            let backend = InterruptedRemoval {
                inner: ManagedDirectory::new(root.join("objects")).unwrap(),
                signal: root.clone(),
                after: mode == "prune-during",
            };
            let store = StoreBuilder::<Bypass, InterruptedRemoval>::new()
                .path(&root)
                .config(config())
                .extra_on_disk(backend)
                .open()
                .unwrap();
            store.prune_before(now() - 10).unwrap();
        }
        "produce-blob" => {
            let store = open(&root);
            let blob = store
                .get_or_insert_blob(key(), || {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(root.join("producer-calls"))
                        .unwrap();
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(root.join("producer-calls"))
                        .unwrap()
                        .write_all(b"x")
                        .unwrap();
                    Ok(vec![7; 4096])
                })
                .unwrap();
            assert_eq!(blob.len(), 4096);
        }
        _ => panic!("unknown test mode"),
    }
}
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(root: &Path, mode: &str) -> Worker {
    Worker(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process_worker", "--ignored", "--nocapture"])
            .env("COMPCACHE_PROCESS_ROOT", root)
            .env("COMPCACHE_PROCESS_MODE", mode)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}
fn wait_ready(root: &Path, worker: &mut Worker) {
    let start = Instant::now();
    while !root.join("worker-ready").exists() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "worker exited before crash point"
        );
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "worker failed to reach crash point"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn crash_during_build_recovers_without_exposing_partial_runtime() {
    let root = tempfile::tempdir().unwrap();
    let mut worker = spawn(root.path(), "build");
    wait_ready(root.path(), &mut worker);
    let observer = open(root.path());
    assert_eq!(
        observer.status(EntryKind::Directory, &key()).unwrap(),
        Some(EntryStatus::Building)
    );
    assert_eq!(observer.recover().unwrap().skipped_busy, 1);
    drop(worker);
    assert_eq!(observer.recover().unwrap().removed, 1);
    assert!(observer.get_runtime(&key()).unwrap().is_none());
    let lease = observer.get_or_insert_runtime(key(), &produce).unwrap();
    assert!(!lease.path().join("partial").exists());
}

#[test]
fn crash_before_or_during_deletion_resumes_pruning() {
    for mode in ["prune-before", "prune-during"] {
        let root = tempfile::tempdir().unwrap();
        let store = open(root.path());
        drop(store.get_or_insert_runtime(key(), &produce).unwrap());
        age(root.path(), 1, &key());
        let mut worker = spawn(root.path(), mode);
        wait_ready(root.path(), &mut worker);
        assert_eq!(
            store.status(EntryKind::Directory, &key()).unwrap(),
            Some(EntryStatus::Pruning)
        );
        assert_eq!(store.recover().unwrap().skipped_busy, 1);
        drop(worker);
        assert_eq!(store.recover().unwrap().removed, 1);
        assert!(store.get_runtime(&key()).unwrap().is_none());
    }
}

#[test]
fn independent_processes_produce_blob_once() {
    let root = tempfile::tempdir().unwrap();
    let mut workers: Vec<_> = (0..4).map(|_| spawn(root.path(), "produce-blob")).collect();
    for worker in &mut workers {
        assert!(worker.0.wait().unwrap().success());
    }
    assert_eq!(
        std::fs::read(root.path().join("producer-calls")).unwrap(),
        b"x"
    );
}
