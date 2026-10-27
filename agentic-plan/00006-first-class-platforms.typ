#set document(title: "First-class platforms and lightweight CI")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= First-class platforms and lightweight CI
Date: 2026-10-27. Status: requirement and CI baseline recorded; Windows/macOS storage backends remain required work.

== Requirement
Linux, Windows and macOS are first-class targets because Reussir supports all three. Platform support includes native storage behavior, not only directory discovery or successful compilation. Maintain the synchronous API, shared SQLite WAL lifecycle, immutable recipe identity, retained reader leases and recoverable interrupted work on every target.

== Current baseline
Store opening currently supports only 64-bit Linux. Windows and macOS return UnsupportedPlatform. Standard directory resolution and entropy-seeded rapidhash indexing already have cross-platform implementations. Existing Linux storage/crash tests and injected Windows retry tests do not establish native storage acceptance on other hosts.

== Native implementation plan
+ Isolate locality checks, directory publication/sync and lock transitions in small platform modules. Keep the shared cache lifecycle and metadata schema.
+ Add macOS local-filesystem inspection, rejecting unsupported/remote mounts. Verify directory syncing and file durability, including fsync/F_FULLFSYNC and the SQLite side of publication.
+ Add Windows native local-volume inspection and supported directory-handle/durability operations. Reject remote/unsupported storage. Implement explicit exclusive-to-shared lease conversion; the current fs4 call leaves the exclusive lock held when adding the shared lock.
+ Exercise Windows sharing violations, transient scanner-held files and loaded DLL deletion. Keep Pruning recoverable after retry exhaustion and avoid depending on Defender being disabled.
+ Enable storage, migration and subprocess crash-recovery suites on each native runner as its backend is completed. Add retained-reader/concurrent-reader and lock-transition checks. Cross-compilation and process termination are not evidence of power-loss durability.

== Lightweight CI baseline
One debug-build matrix job per native OS: ubuntu-latest, windows-latest and macos-latest. Use rust-toolchain.toml, Cargo.lock and dependency caching. Run on pull requests, pushes to main and manual dispatch. Cancel superseded runs, skip documentation-only changes, use read-only repository permissions and cap jobs at 15 minutes.

Each runner checks all targets and runs portable unit, key and documentation tests. Windows/macOS also run the in-memory cache weighting/concurrency test. Linux runs formatting, Clippy with warnings denied, and the storage/schema/subprocess recovery suites. Do not suppress failures with continue-on-error. Windows/macOS storage suites are deliberately pending their backends, rather than silently skipping failed storage tests.

Benchmarks, six-process stress, P model checking and Nickel CLI evaluation stay outside routine CI. Separate defaults path tests from Nickel evaluation tests so path/default behavior remains covered without another CLI installation. Preserve local commands for the heavier validation.

== Acceptance and evidence
Storage support is complete only when native locality, publication/durability and lease behavior have implementations and passing native lifecycle tests. Claims must identify the OS, filesystem and tested failure model. At workflow creation, hosted results are pending the first push; record actual outcomes separately from this requirement. Existing timing and model-check records remain historical Linux evidence.

Key derivation continues to accept canonical bytes/digests without requiring serde at the call site. The nested serde adapter remains available; changing the persistent encoding or adding a new derive system is outside this platform/CI update.

== Local verification before push
Formatting, all-target compilation, Clippy with warnings denied and the complete Rust test suite pass on Linux, including Nickel configuration tests and the short six-process stress run. The CI-selected unit suite also passes with an empty PATH, demonstrating no Nickel executable is needed. Actionlint validates the workflow and all five changed/new Typst plans compile.

== First hosted verification
#link("https://github.com/reussir-lang/compcache/actions/runs/40588333375")[CI run 40588333375] for commit 366d1f8 completed successfully on all three native runners on 2026-10-27. Cold-job durations were 56 seconds on Ubuntu, 43 seconds on macOS and 121 seconds on Windows, including setup and cache creation. Each runner passed all-target compilation and its selected tests. Linux additionally passed Clippy and storage/schema/process-recovery tests. Windows/macOS storage support remains pending the native implementation plan above. This evidence update changes only plan documents and therefore does not trigger another routine CI run.
