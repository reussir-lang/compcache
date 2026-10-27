#set document(title: "Native storage qualification and strict Clippy")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Native storage qualification and strict Clippy
Date: 2026-10-27. Status: native storage and strict Clippy qualified on all three targets.

== Supported scope
Provide production storage on the documented local Linux filesystems, local NTFS on Windows, and local APFS/HFS+ on macOS. Require cooperating processes, a private cache root, complete immutable producer output and storage honoring the operating system's flush/lock operations. Unsupported or remote storage is rejected; no flush failure is converted to success. Physical power-cut experiments are outside the hosted process-crash tests.

== Native filesystem and lease implementation
Linux retains its existing filesystem allowlist. macOS uses fstatfs, MNT_LOCAL and a filesystem-name allowlist; SQLite enables fullfsync and checkpoint_fullfsync. The pinned standard library's sync_all uses F_FULLFSYNC on Apple targets.

Windows resolves the actual path and volume, rejects remote/unknown drive types and accepts only NTFS. Writable directory handles use FILE_FLAG_BACKUP_SEMANTICS and propagate sync_all/FlushFileBuffers failures. A runtime writer first acquires the overlapping shared LockFileEx lock, then explicitly releases the exclusive lock; other readers can proceed while the shared lease remains held. Unix lock conversion retains the existing metadata/reference revalidation.

ManagedDirectory canonicalizes its root so relative paths and Windows path aliases do not change backend identity. Defaults tests run with isolated child environments on all three targets. Invalid config-directory overrides are checked before file absence: Windows can otherwise report a child of a regular file as NotFound and silently fall back.

== Native regression coverage
All runners execute storage, migration and process-crash recovery suites. Platform tests verify that a newly published writer lease allows a second reader and that Unicode cache roots survive reopening. Windows tests hold an actual file without delete sharing, verify recoverable Pruning after bounded retry exhaustion, and retry recovery after releasing the file. A compiled test DLL is loaded with LoadLibraryW; pruning must either complete or leave a recoverable row until unloading. Locality classifier tests reject remote and unvalidated filesystem types.

The first native run found a signed mount-flag type mismatch on macOS and a Windows path-alias comparison in a migration/reference test. Both were corrected without suppressing failed tests. Native acceptance is based on the final passing run, not that initial attempt.

== Strict Clippy policy and cleanup
Enable pedantic and nursery warnings in Cargo.toml and deny warnings in CI on every native runner, including all targets. Preserve the project's non-const API style and explicit internal visibility through two documented style exceptions. The uninhabited NoExtraData implementation and Criterion's consuming group finish have narrow, reasoned lint allowances.

Use checked conversions for SQLite payload lengths, retention intervals and test timestamps. Document public error contracts, mark builders/writers and useful return values must-use, remove invariant-only path panics, explain subprocess-only ignored tests, and shorten database lock lifetimes only after their last dependent operation. No numeric conversion or synchronization lint is disabled.

== Verification
Linux formatting, strict all-target Clippy, the full test suite including Nickel and short six-process stress, and actionlint pass locally. Routine CI stays light; a manual stress=true workflow input adds ten seconds of multiprocess stress per native runner. Record hosted native results after qualification completes.

#link("https://github.com/reussir-lang/compcache/actions/runs/40592018954")[Final native qualification run 40592018954], commit f43c6b3, passes on all three native runners. Pedantic/nursery Clippy, defaults unit tests, key compatibility, documentation, native storage/migration/crash recovery and ten seconds of six-process stress all complete successfully. Windows also passes both held-file and loaded-DLL tests. Total job durations including setup/cache handling: Ubuntu 74 seconds, macOS 53 seconds, Windows 111 seconds. Public API documentation builds locally with warnings denied. Earlier failed attempts remain historical diagnostics; the passing qualification uses the corrected code.
