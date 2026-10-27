#set document(title: "Implementation validation and performance")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Validation and performance record
Date: 2026-10-27.

== Implemented scope
Structured SchemaKey field arrays and borrowed values, with length-framed BLAKE3-256 identity; synchronous generic `StoreBuilder<I, D>` and `Store<I, D>`; trait-defined construction from configuration; weighted concurrent quick_cache and Bypass; SQLite WAL blobs and metadata; optional managed directories and NoExtraData; custom producer contracts and counted streamed output; retained runtime leases; strict deadline pruning with an advisory size trigger; recovery of interrupted/missing output; typed thiserror errors; tracing events; bounded filesystem backoff with jitter; pinned Nix development and proof shells; Nickel defaults and a runnable example.

The implementation is split into focused modules. No source module exceeds 220 lines after rustfmt. SQLite query code and schema/opening code are separate. No async runtime or background maintenance thread is used.

== Checks
Formatting and Clippy with warnings denied pass inside the pinned Nix shell. Unit and integration checks pass: filesystem rejection, backoff classification/budget, immutable blob persistence, memory weighting/concurrency, concurrent independent handles, runtime reader leases, separate blob/runtime namespaces, typed producer errors/retry, strict deadline comparison, trigger-not-quota behavior, DB/WAL/SHM accounting, missing-directory recovery, failed pruning recovery, unsupported schema/corrupt database errors, recorded backend-root consistency, and monotonic access metadata. The Nickel-configured example runs successfully.

Subprocess crash tests kill a builder after partial output, and kill a pruner both before deletion and after deleting one runtime file. A live second handle observes Building/Pruning, skips recovery while the owner lives, then recovers safely after process death. Independent subprocesses also produce a single blob once.

P 3.1.0 compiles the model and checks 10,000 safety schedules at seed 1729 with 60 nondeterministic steps and two modeled process identities. It reports no bugs. The finite conditional recovery-progress test passes 100 schedules. The intentionally unsafe pruning test fails at its first schedule with the expected reader-visibility assertion. The script treats failure to find this counterexample as failure, and checks the trace so unrelated tool failures cannot count as success. This is bounded model checking, not exhaustive verification.

== Federated integration stress
Six independent OS processes share one actual store. Four mix bytecode and runtime production/reads; two repeatedly prune and recover. Half of the workers select quick_cache and half select Bypass. Producers yield between writing image and manifest. Readers sometimes retain a runtime lease across access-second boundaries, checking both files while deletion is active elsewhere. All workers independently open SQLite and the managed filesystem; no service or coordinating cache process exists.

The initial stress run exposed SQLITE_BUSY during concurrent first-open WAL setup. A short, process-shared initialization lease now serializes WAL/schema setup, before per-key recovery. This was a production race found by integration testing, not hidden by retrying the stress harness.

The final 20-second run in the Nix shell completed:
#table(
  columns: (1fr, 1fr),
  [Measurement], [Count],
  [Total operations], [50,085],
  [Runtime reads with verified contents], [23,425],
  [Blob reads with verified contents], [11,860],
  [Pruned entries], [503],
  [Busy candidates safely skipped], [301],
)
The final database quick_check is ok, all remaining rows are Ready, and all remaining known runtime keys have their expected image and manifest. The reproducible harness is tests/stress.rs; COMPCACHE_STRESS_SECONDS controls duration and COMPCACHE_STRESS_REPORT writes aggregate JSON. Parent cleanup kills and reaps child processes even when a test fails.

== Benchmark method
Final run: pinned stable Rust 1.98.0, Criterion 0.5.1, release profile, 30 samples, 1 s warmup and 2 s measurement per case (slow pruning extends the estimate automatically). Host: AMD Ryzen AI MAX+ 395, 16 cores / 32 threads, local ZFS. Payloads: 64 B, 4 KiB and 256 KiB for warm memory hits; 4 KiB for SQLite; 1 KiB files for the 100-runtime prune batch. SQLite uses WAL and synchronous FULL. These measurements include filesystem and scheduler effects; they are a local baseline, not portable latency promises. Reported values below are central Criterion estimates.

Initial/intermediate comparisons used installed Rust 1.100.0-nightly (2026-08-31); their raw logs establish the before/after improvement on the same compiler. The final run uses pinned stable Rust 1.98.0 and includes the final structured-key/directory-reference changes.

The initial implementation persisted an access update on every memory hit. This caused indexed SQLite writes and sync costs even when the whole-second timestamp had not changed. Repeated hits now reuse the already-persisted access second; the first hit in a new second still touches metadata. No unbounded deferred-access buffer or background flusher was introduced.

A second measured issue affected SQLite-only and runtime readers: UPDATE max(accessed, now) still performed unnecessary indexed writes for identical timestamps. They now SELECT the timestamp and write only when it advances, retaining a monotonic conditional UPDATE to handle other processes.

#table(
  columns: (1.5fr, 1fr, 1fr),
  [Case], [Before], [After],
  [Warm blob, 64 B], [284.00 µs], [105.61 ns],
  [Warm blob, 4 KiB], [293.47 µs], [105.49 ns],
  [Warm blob, 256 KiB], [559.80 µs], [105.67 ns],
  [SQLite hit, 4 KiB], [357.48 µs], [4.1574 µs],
  [Runtime reader lease], [349.40 µs], [6.2140 µs],
)

Four-field SchemaKey hashing (including field schema framing) takes 480.42 ns without allocation.

Other final baselines: SQLite insert of a new 4 KiB blob 581.97 µs; direct quick_cache hit 37.539 ns; quick_cache miss 26.066 ns; pruning 100 old runtime directories 107.99 ms. The prune benchmark excludes setup/teardown through iter_batched_ref and includes committed state changes, filesystem removal and parent syncing. Insert/prune costs are dominated by the local durable filesystem protocol and were not weakened to improve the score.

Shared-store reads use disjoint key ranges per worker, with thread creation outside the measured interval and joins inside. Aggregate estimates: 1 worker 9.06 million reads/s, 4 workers 27.89 million reads/s, 16 workers 40.91 million reads/s. Timer noise, second-boundary metadata writes and sharding affect these numbers; the raw output preserves confidence intervals and outliers. The 16-worker estimate in particular has a broad interval (23.0–76.7 million reads/s) and is not a scaling guarantee. Payload-size-independent warm reads return shared immutable bytes, avoiding payload copies.

Raw initial, first-optimization and final Criterion logs plus stress/validation evidence are in agentic-plan/evidence. Reproduce using cargo bench --bench cache -- --save-baseline NAME; compare matching named baselines through Criterion. The disk benchmarks disable automatic pruning so they measure the operation itself. Configuration-loading and cold filesystem startup are outside these timings.

== Remaining platform scope
Execution is validated on 64-bit Linux with the documented local-filesystem allowlist. Windows transient-error classification is injected and tested, but native Windows volume validation, directory durability and Defender/loaded-DLL integration tests remain future platform work. No Windows runtime support or power-loss experiment is claimed. Custom producer size/durability declarations are trusted; the library does not discover missing metadata by scanning payload trees.

== Structured-key refinement
At the user's direction, structured keys use `SchemaKey<const KEY_LEN: usize>`, `KEY_FIELDS: [&'static str; KEY_LEN]`, and `key_value() -> [&str; KEY_LEN]`. This encodes the length at the type level on stable Rust. `Key::new(&user_key)` hashes a versioned schema-key prefix, field count, and each field name/value with explicit lengths. No serde, string concatenation, heap allocation, or fallible encoding is involved. The field schema itself distinguishes otherwise identical values under different field names. The explicit raw binary constructor remains available as Key.from_parts. Examples and a schema/boundary regression test use the structured API.

== Final metadata refinement
The accepted physical schema extends entries with an optional object_key alongside size/access/status/blob metadata. Building records the object reference before producing files; runtime reads, pruning and recovery follow it. A relocation test proves this indirection is used, and the version-1 migration test preserves old names without scanning. All final Nix checks, the example, and the 20-second stress run pass after this refinement. Two worker entry points are intentionally marked ignored and invoked explicitly by their parent subprocess tests.

== 2026-10-27: Three-platform CI baseline
Linux, Windows and macOS are first-class targets. The first hosted run, 40588333375 at commit 366d1f8, passed on all three native runners. Cold-job durations were 56 seconds on Ubuntu, 43 seconds on macOS and 121 seconds on Windows. Windows/macOS checks covered compilation and portable behavior at that stage; only Linux ran storage/schema/process recovery.

== 2026-10-27: Native storage and strict Clippy qualification
Subsequent production qualification run 40592018954 at commit f43c6b3 passes the full native matrix and ten-second six-process stress on all three platforms. Durations including setup/cache handling were 74 seconds on Ubuntu, 53 seconds on macOS and 111 seconds on Windows. Pedantic/nursery Clippy, cross-platform defaults tests, native storage/schema/process recovery and Windows held-file/loaded-DLL tests pass. This closes the earlier portable-only native storage gap.

Routine CI now runs all-target compilation, strict Clippy, defaults/key/documentation tests and storage/schema/subprocess recovery on every native runner. Formatting runs on Linux. A manual stress=true input adds ten seconds of multiprocess stress per runner. Benchmarks, P checking and Nickel CLI evaluation remain outside routine CI; defaults path tests run without Nickel. No physical power-loss experiment is claimed. Producer size/durability declarations remain trusted.
