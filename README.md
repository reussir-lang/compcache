# compcache

A synchronous local compiler cache. Independent processes share a directory and
SQLite WAL database, with no server or async runtime. BLAKE3-256 keys identify
immutable build recipes. Include the compiler version, target, inputs and flags
in every recipe.

Linux, Windows and macOS are first-class target platforms, matching Reussir.
Native storage backends cover 64-bit Linux, local APFS/HFS+ on macOS and local
NTFS on Windows. Native storage qualification is being run in CI, including
leases, migration, interrupted builds/deletion and Windows file-sharing behavior.

```rust
use compcache::{Config, ManagedDirectory, QuickCache, StoreBuilder};

let store = StoreBuilder::<QuickCache, ManagedDirectory>::new()
    .path("/tmp/compiler-cache")
    .config(Config::from_nickel("config/default.ncl")?)
    .open()?;
# Ok::<(), compcache::Error>(())
```

`InMemoryCache::from_config` and `ExtraOnDiskData::from_config` construct each
strategy. `Store<I, D>` keeps concrete backend types, without dynamic dispatch.
Choose `Bypass` for no memory layer and `NoExtraData` for SQLite-only payloads.
The builder can also accept initialized backends using `.in_memory(...)` and
`.extra_on_disk(...)`.
`QuickCache` indexes recipe keys with entropy-seeded
[`rapidhash::fast::RandomState`](https://docs.rs/rapidhash/4.5.1/rapidhash/fast/type.RandomState.html).

Applications can supply their name, fallback settings and optional environment
overrides through `StoreDefaults`:

```rust
use compcache::{Config, Store, StoreBuilder, StoreDefaults};

struct App;
impl StoreDefaults for App {
    const APP_NAME: &'static str = "my-compiler";
    const CONFIG_DIR_ENV: Option<&'static str> = Some("MY_COMPILER_CONFIG_DIR");
    const CACHE_DIR_ENV: Option<&'static str> = Some("MY_COMPILER_CACHE_DIR");

    fn config() -> Config {
        let mut config = Config::default();
        config.disk.prune_trigger_bytes = 8 << 30;
        config
    }
}

let store: Store = StoreBuilder::with_defaults::<App>()?.open()?;
# Ok::<(), compcache::Error>(())
```

`APP_NAME` must be a single directory name. The standard locations, resolved by
the [`dirs` crate](https://docs.rs/dirs/6.0.0/dirs/), are:

| Platform | Config directory | Cache directory |
| --- | --- | --- |
| Linux/XDG | `$XDG_CONFIG_HOME/my-compiler`, otherwise `$HOME/.config/my-compiler` | `$XDG_CACHE_HOME/my-compiler`, otherwise `$HOME/.cache/my-compiler` |
| Windows | Roaming AppData / `my-compiler` | Local AppData / `my-compiler` |
| macOS | `$HOME/Library/Application Support/my-compiler` | `$HOME/Library/Caches/my-compiler` |

Nonempty application environment overrides replace the complete directory,
without appending `APP_NAME`; relative overrides use the working directory.
Unset or empty overrides use the platform locations. XDG base paths must be
absolute; empty or relative XDG values use the home-directory fallback.
If a platform directory cannot be resolved, construction returns `Error::Config`.

On `open()`, the builder loads `config.ncl` from the selected config directory.
A missing file and fields omitted from a file use `StoreDefaults::config()`;
omitted nested fields also retain the application's defaults. Nickel is needed
only when a file exists. Invalid files and other I/O errors are returned.
The builder creates the cache directory when opening the store; it does not
create the config directory or write a config file. Subsequent `.path(...)` and
`.config(...)` calls take precedence, with `.config(...)` disabling file loading.
`new()` and the standard `Default` implementation retain their existing behavior.
Directory resolution supports Windows and macOS; store opening still has the
platform restrictions described below.

Key construction does not require serde. `Key::from_parts` accepts a stable
binary recipe, `Key::from_bytes` accepts an existing BLAKE3-256 digest, and
`SchemaKey` accepts named borrowed-string fields. `Key::from_serde` is a
convenience adapter for nested recipes; serde is also used for configuration.
Persistent identity requires deterministic, versioned input encoding; Rust's
standard `Hash` trait does not promise a portable encoding across platforms or
compiler versions ([Rust portability documentation](https://doc.rust-lang.org/std/hash/trait.Hash.html#portability)).
Key encoding happens outside repeated cache lookups.

For example, a complete recipe can use serde:

```rust
use compcache::Key;
use serde::Serialize;

#[derive(Serialize)]
struct UserKey<'a> {
    compiler: &'a str,
    target: &'a str,
    inputs: &'a [[u8; 32]],
    optimization: u8,
    features: &'a [&'a str],
}
let recipe = UserKey {
    compiler: "v1", target: "x86_64", inputs: &[],
    optimization: 2, features: &["simd"],
};
let key = Key::from_serde("bytecode-v1", &recipe)?;
let bytes = store.get_or_insert_blob(key, || Ok(vec![1, 2, 3]))?;
```

The namespace identifies the recipe meaning/version. A versioned, typed binary
encoding feeds BLAKE3-256; maps and named fields are sorted recursively, sequences
remain ordered, and options/enum variants/numeric kinds stay distinct. Float bits
are preserved, including signed zero and NaN payloads. Custom serialization runs
in binary mode. The identity is the serialized projection, so serde renames and
skipped fields have their normal effect. Unordered sets need caller-side
ordering because serde represents them as sequences. Rust type names and enum
indices do not contribute; serialized variant names do.

Serialization allocates for encoding and map sorting. Compute a digest once and
reuse it for lookups. Failures return `Error::KeyEncode` with a typed
`KeyEncodeError` source. No SQLite schema change or new dependency is needed.
The encoding contract and fixed compatibility vector are recorded in
[the serde work record](agentic-plan/00005-whole-key-serde.typ).

The allocation-free field-schema API is also available:

```rust
use compcache::{ArtifactWriter, Key, SchemaKey};

struct UserKey<'a> { compiler: &'a str, target: &'a str, source: &'a str }
impl SchemaKey<3> for UserKey<'_> {
    const KEY_FIELDS: [&'static str; 3] = ["compiler", "target", "source"];
    fn key_value(&self) -> [&str; 3] { [self.compiler, self.target, self.source] }
}
let key = Key::new(&UserKey { compiler: "v1", target: "x86_64", source: "source-digest" });
let bytes = store.get_or_insert_blob(key, || Ok(vec![1, 2, 3]))?;
let runtime = store.get_or_insert_runtime(key, &|path: &std::path::Path| {
    let mut output = ArtifactWriter::new(path);
    output.write_file("runtime.bin", b"compiled runtime")?;
    output.finish()
})?;
// Keep `runtime` alive until all compiler/linker access to runtime.path() ends.
```

The const generic is the schema's `KEY_LEN`; names and borrowed values must have
that same length. The store derives the key schema from `KEY_FIELDS` and hashes
each name/value pair with length framing. Field boundaries and different field
names cannot alias through concatenation. Compute the digest once and reuse it.
`Key::from_parts` accepts an existing stable binary recipe encoding; `from_bytes`
accepts an existing BLAKE3-256 digest.

Directory producers may invoke an external compiler instead. They must finish
and sync their writes before returning the total logical payload size. The
counted writer supports streaming through `copy_file`; neither it nor the store
walks runtime trees for sizing. Producers must not mutate a published runtime.
Blob and runtime keys have separate storage namespaces. A producer error can
preserve its concrete source with `Error::producer(error)`.

The caller's field schema is independent of SQLite's physical schema. One
`entries` row holds `kind`, the BLAKE3 `key`, `state`, `bytes`, `accessed`, optional
`payload`, and optional `object_key`. The latter identifies the hashed directory
under the recorded backend root; reads and deletion use this stored reference.
No metadata join is required. Schema migration preserves existing references.
SQLite holds metadata, statuses, access times, payload sizes, and optional blob
payloads. Runtime readers hold shared OS leases; builders and pruners hold
exclusive leases for that key. `Building` commits before construction and `Ready`
after complete durable output. `Pruning` commits before deletion. Readers never
receive an incomplete runtime. Recovery removes abandoned builds, resumes
pruning and invalidates missing directories; live leases are skipped. An I/O or
unrecoverable database error is returned instead of silently resetting the cache.
Lock files are retained so independent processes always lock the same inode.
Directory backend roots must match the root recorded in SQLite.

Access times are monotonic whole Unix seconds. Repeated hits within a second do
not rewrite SQLite. `prune_if_needed()` checks the size trigger and prunes entries
whose last access is strictly before `now - prune_older_than_secs`.
`prune_before(unix_second)` explicitly applies a deadline. Eligibility is checked
again under the exclusive key lease, and busy entries are skipped. With
`prune_on_write = true`, successful writes also check the trigger; maintenance
failure is reported as `Error::Maintenance`, with the payload already stored.
Recent entries may keep the store above its trigger size.

Accounting adds recorded external payload bytes to DB/WAL/SHM file lengths.
SQLite blobs are included only through those files. No runtime-content sizing
scan occurs. This metric excludes inode/directory overhead and allocation
rounding; in-progress producer output is recorded at publication. Deleting a
SQLite blob frees reusable pages without necessarily shrinking the DB file.
`checkpoint()` requests WAL truncation and returns whether it completed.

Nickel is evaluated once on configuration load using the `nickel` CLI. The
[default configuration](config/default.ncl) exposes all quick_cache construction
options (shards, estimated items, byte weight, hot and ghost allocation), maximum
entry weight, layer selection, SQLite page cache/busy timeout/checkpoint settings,
prune trigger/age, and bounded filesystem backoff. `quick_cache` exposes hit/miss
statistics. The library emits `tracing` lifecycle/retry events; applications
install the subscriber, as in [the example](examples/basic.rs).

Run development checks:

```sh
nix develop
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --example basic -- /tmp/compiler-cache
cargo bench --bench cache
nix develop .#proof --command bash scripts/check-model.sh
```

GitHub Actions runs one debug-build job each on Linux, Windows and macOS, using
the pinned Rust toolchain and cached dependencies. Each job compiles all targets
and runs unit, key, documentation, storage, schema and subprocess crash-recovery
tests. Windows also exercises held files and loaded DLLs. Pedantic and nursery
Clippy checks run with warnings denied on every platform; formatting runs on Linux.

Routine CI excludes benchmarks, six-process stress, the P checker, and Nickel
configuration evaluation. Run those locally with the commands above. Path and
default-value tests run without Nickel. Documentation-only changes skip CI,
superseded runs are cancelled, and each job has a 15-minute limit.
Manual workflow dispatch with `stress=true` adds a ten-second six-process stress
run on every native runner.

Nix uses the same pinned nixpkgs/fenix inputs as `../reussir`, with stable Rust
1.98.0, Nickel, Typst and SQLite tooling. The proof shell adds only .NET 8;
P 3.1.0 is pinned in the project-local tool manifest. No LLVM/MLIR, Node, JVM,
async runtime or compiler-framework packages are required.

The P model checks runtime visibility, reader/writer exclusion, publish/delete
ordering, interrupted work and conditional recovery termination. The script also
requires a deliberately unsafe pruning model to fail. These are bounded model
checks, not an exhaustive proof of the Rust/filesystem implementation. Rust
integration tests kill processes at construction/deletion boundaries and run six
independent processes producing, reading, pruning and recovering one shared
store. Increase duration and save a report with:

```sh
COMPCACHE_STRESS_SECONDS=30 COMPCACHE_STRESS_REPORT=/tmp/stress.json \
  cargo test --test stress -- --nocapture
```

All three platforms must provide the same publication, reader-lease and recovery
guarantees. Current storage implementation status:

| Platform | Storage status | Required native work |
| --- | --- | --- |
| 64-bit Linux | Implemented and validated | Maintain locality and lifecycle tests |
| Windows | Native NTFS backend; qualification in progress | Native storage, lock downgrade, held-file and loaded-DLL tests run in CI |
| macOS | Native APFS/HFS+ backend; qualification in progress | Native storage, durability, lease and recovery tests run in CI |

Platform-specific filesystem operations sit behind the shared synchronous
SQLite and lease protocol. Native tests cover retained readers,
concurrent builders, exclusive-to-shared lock conversion, interrupted builds and
deletion, pruning and recovery. Process-kill tests do not establish power-loss
durability; publication guarantees need separate platform evidence.

Linux local filesystem validation uses a fail-closed allowlist; NFS, SMB, Ceph,
FUSE, unknown filesystems and overlay mounts
are rejected. Filesystem type cannot detect remote-backed block devices or an
administrator replacing mounts. The protocol assumes cooperating processes and
a private cache root. Never place a WAL database on a network filesystem
([SQLite WAL documentation](https://www.sqlite.org/wal.html)).

Filesystem retries use bounded exponential delay and equal jitter; they never
rerun a producer or blindly replay transactions. Windows sharing/lock violations,
transient access denial, and nonempty-directory errors are covered by retry
classification tests. New platform backends must also reject remote/unsupported
filesystems. On Windows, a scanner or loaded DLL can hold a file without delete sharing, delaying
delete/rename until the handle closes; retry exhaustion must leave `Pruning`
recoverable. Delete sharing and directory handles require native treatment
([CreateFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea)),
and file flush semantics must not be confused with directory publication
([FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)).
Windows retains the shared lock and explicitly releases the exclusive lock
during a writer-to-reader transition, following
[LockFileEx](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)
overlap rules. Directory flushing opens writable directory handles with backup
semantics; flush errors are returned. Local NTFS is supported; remote drives,
FAT and ReFS are rejected.
On macOS, mount-locality checks allow APFS and HFS+. The pinned Rust toolchain's
`File::sync_all` uses `F_FULLFSYNC`; SQLite also enables `fullfsync` and
`checkpoint_fullfsync` for publication and checkpoints
([Apple fsync documentation](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html)).
