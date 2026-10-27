#set document(title: "Compcache design and work record")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Compcache: decisions and execution record
Date: 2026-10-27. Status: implementation and validation complete; native Windows execution remains future platform work.

== User decisions
+ Set up VCS and a minimal Nix environment following ../reussir, including only required components.
+ Keep plans, design decisions, progress, validation, and performance results in agentic-plan/XXXXX-NAME.typ.
+ Build a pluggable on-disk compiler cache without a central server. All APIs are synchronous; avoid an async runtime.
+ SQLite is the authority for all metadata and may store payloads such as bytecode directly. Require WAL mode and reject remote filesystems.
+ Write a P-language cache-state model and correctness properties before Rust implementation.
+ The top layer uses quick_cache's concurrent S3-FIFO policy behind an InMemoryCache trait. Supply a direct bypass for runtime baking and other workloads.
+ Managed runtime directories are optional. Supply a separate extension point and caller-customized synchronous producers.
+ Configuration uses Nickel (.ncl). Expose useful in-memory options, on-disk pruning trigger size, and an access deadline. A pruning trigger is not a hard size cap: retain recent entries even if the cache remains over the trigger.
+ Account for the database itself as well as external payloads. Trust recorded payload sizes; do not scan runtime trees for sizing.
+ Recover interrupted work where possible; report an error when recovery cannot safely complete.
+ Publish pruning/building states so readers cannot receive partially built or partially deleted runtimes.
+ Prefer BLAKE3-256 wherever hashing is necessary.
+ Keep the in-memory path thread-safe and fast. Benchmark characteristics, record an initial baseline, and fix initial measured problems.
+ Use tracing for library diagnostics without excessive defensive machinery. Use typed thiserror errors with preserved causes instead of untyped error strings. The application installs a tracing subscriber.

+ Keep project structure aligned and modular; avoid overly long modules without justification.
+ Use a clean generic builder API: `StoreBuilder<InMemory, ExtraOnDiskData>`.new().path(...).config(...).open(). Backend traits define initialization from configuration.
+ Include real multi-process integration stress on a shared store, beyond single-process/thread tests.
+ Add a backoff strategy and account for Windows quirks such as Defender temporarily holding files during filesystem mutations.

+ Support caller-defined structured keys through `UserKey: SchemaKey`: a compile-time field count, static field names, and `key_value()` returning an array of borrowed strings. Derive the schema from those fields; no serialization requirement or manually formatted compound key.

+ Keep user key schemas separate from internal metadata such as byte size, last access, and hashed external-directory references. Either a foreign-key relation or extending the entry table is acceptable; the public schema need not match SQLite's physical schema.

== Implementation choices
These are implementation choices, distinct from the user requirements above.

The initial platform is Linux, with a fail-closed local-filesystem allowlist. Known local filesystems include ext, XFS, Btrfs, ZFS, tmpfs, ramfs, and F2FS. Unknown filesystems and FUSE are rejected. Network-backed block devices and malicious mount changes are outside the guarantee; filesystem type cannot detect those.

Keys are immutable BLAKE3-256 digests of a caller's domain-separated recipe (compiler version, target, inputs, flags). Callers must include every production input. Blob and directory keys use separate namespaces.

Per-key OS locks serialize builders and pruners across processes. Runtime readers own shared lock leases. Persisted states are Building, Ready, and Pruning; absence represents Missing. Commit Building before touching a runtime directory; sync produced files/directories before committing Ready; commit Pruning before deletion. Keep a Pruning row if deletion fails. Recovery uses nonblocking exclusive locks so it never mistakes a live builder or reader for abandoned work. Never remove lock files, since replacing their inodes would split the lock domain.

SQLite transactions are short and never encompass user production or directory deletion. Lock order is key lease then SQLite connection. Pruning chooses candidates from an indexed metadata query, then acquires each key exclusively and rechecks eligibility. Already-returned blob bytes are immutable snapshots; directory paths are valid only while their lease lives.

Access times have whole-second resolution. Pruning uses a strict before-deadline comparison. Access timestamps are monotonic per row. A memory entry may coalesce repeated touches within the same second; once the second changes it must touch SQLite before returning. Prune deadlines cannot be in the future. This allows a fast same-second hot path without buffering metadata for an unbounded period.

Directory producers report logical payload bytes, including all files they create; this is a trusted contract. The standard directory producer counts writes. Metadata accounting adds these bytes to constant-number stat calls for the database, WAL and SHM. SQLite blobs are already included in database/WAL lengths and are not added twice. Filesystem allocation rounding and directory/lock inode overhead are outside this logical size metric. A free SQLite page remains part of the database size until reused or explicitly vacuumed.

== Work plan
+ Record decisions and implement the P model, safety monitors, finite crash tests, and recovery progress test.
+ Initialize Git; create a minimal pinned Nix flake and project-local P tool manifest.
+ Implement typed configuration/errors, memory strategies, SQLite persistence, optional directory producers, leases, pruning, and recovery.
+ Exercise concurrency, crash boundaries, deadline/accounting behavior, and configuration.
+ Benchmark memory hits, SQLite hits/writes, runtime leases, contention, and pruning. Fix measured regressions and record reproducible results.
+ Update this record with actual checks, limitations, and completed work.

== Progress
The P model was authored before Rust cache implementation and checked with P 3.1.0. Git was initialized with main as its initial branch. The minimal Nix development and proof shells are functional. Core modules remain below 220 lines each after formatting, separating the builder, configuration, memory, directories, metadata queries, schema initialization, filesystem operations, pruning, recovery, and retry policy.

The builder returns `Store<I, D>` with concrete strategy types. InMemoryCache.from_config receives MemoryConfig; ExtraOnDiskData.from_config receives the managed root and DiskConfig and may return no backend. Bypass and NoExtraData select the corresponding omissions.

Bounded filesystem backoff uses exponential delays with equal jitter. Deletion retries preserve the committed Pruning row and exclusive lease; permanent failures return a typed I/O error and retain recoverable metadata. SQLite uses its bounded busy timeout, plus a separate OS initialization lease for first-open WAL/schema setup.

Six-process stress exposed a first-open journal-mode race that SQLite's busy handler did not cover. Serializing WAL/schema initialization fixed it. Stress also exercises live builds, runtime readers, pruning and recovery, with immutable payload checks under retained leases.

Two measured performance issues were fixed: hot-memory accesses no longer rewrite SQLite in the same access second, and SQLite-only/runtime reads first inspect access metadata instead of performing a redundant indexed UPDATE on every read. The deadline remains a strict comparison at whole-second resolution.

== Validation record
P safety: 10,000 schedules, seed 1729, 60 nondeterministic operations per schedule, two modeled processes, no violations. Recovery progress: 100 schedules, no violations. Negative control: unsafe pruning while readers hold leases fails on schedule 1 with the expected visibility assertion. Five-second six-process stress: 12,272 operations, 5,647 runtime reads, 2,878 blob reads, 92 pruned entries, 47 busy candidates skipped; all payload checks passed. A longer final run is recorded in the subsequent progress record.

 A P model is an abstraction, not a proof of Rust, SQLite, or filesystem implementations. Bounded scheduling checks must be reported as bounded checks, never as an exhaustive proof.
