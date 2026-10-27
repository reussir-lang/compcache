#set document(title: "Cache protocol, properties and platform decisions")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Protocol and correctness argument

SQLite transactions and per-key OS lock acquisition/release are the model's atomic operations. The P machine represents shared resources; it is not an implementation server. Each row is keyed by payload kind and a BLAKE3-256 recipe identity. Builders and pruners must first hold an exclusive key lease. Runtime consumers must hold a shared key lease for their entire use of a runtime path.

== Persistent transitions
Missing → Building: commit the row before creating output. Building → Ready: producer completes and syncs files and directories, then commit the recorded byte count and Ready state. Ready → Pruning: acquire exclusivity, recheck last-access deadline, then commit Pruning before removing anything. Pruning → Missing: finish deletion, sync the containing directory, then delete the row. Interrupted Building can only recover through Pruning; recovery never guesses that partial output is complete. SQLite blob publication/deletion is a single SQLite statement, so it needs no visible intermediate filesystem state.

== Properties and proof obligations
+ Publication safety: every returned runtime comes from Ready state, after producer completion and durable filesystem publication. The standard counted writer syncs each file and all directories it creates. A custom producer must honor the same contract.
+ Reader safety: a shared runtime lease excludes both construction and deletion. No runtime path is returned from Building or Pruning. The exclusive-to-shared lock conversion is rechecked; if the platform releases the old lock during conversion and another process changes the row, acquisition retries rather than returning a stale path.
+ Deletion ordering: Pruning is committed before the first destructive filesystem operation, including during recovery. Partial removal and retry exhaustion preserve Pruning for a later recovery attempt.
+ Exclusive ownership: one live process owns a key's writer lease. Recovery uses try-locks, so it cannot steal an active builder's work. OS process death releases locks while SQLite state remains durable. Persistent lock files prevent lock-domain splitting.
+ Deadline safety: a candidate is rechecked while holding its exclusive key lease. Only access seconds strictly below the deadline are eligible; recent entries survive above the trigger. Access updates never move timestamps backward. The optimization coalesces only timestamps that are identical at the declared resolution.
+ Immutable identity: a recipe key names a deterministic payload. Cache misses may rerun a producer after failure or pruning; successful concurrent callers share the first committed result. Already-returned blob byte arrays remain valid after disk eviction.
+ Accounting: external byte totals come from committed producer metadata. Database/WAL/SHM lengths include SQLite blob storage once. No traversal is used to discover payload sizes. Interrupted builds may temporarily consume unrecorded output until recovery removes it; the trigger is advisory.
+ Recovery progress: after owners cease and I/O operations succeed, finite recorded interrupted work is removed. Persistent I/O errors cause a typed failure rather than endless retries. Recovery under a perpetually active process or failed storage cannot promise successful completion.

The safety invariants are inductive under these operations: Missing has no readers; creating Building requires exclusive ownership; completing output preserves exclusivity; publishing makes Ready before releasing it; readers acquire only from Ready; pruning requires all readers to be absent; crashes release ownership without publishing partial output; recovery follows the same deletion ordering. The P monitors test the invariants and a hot-state recovery observer checks the conditional progress scenario. Bounded randomized checking is not an unbounded machine-checked proof, nor a proof of Rust/SQLite/kernel implementations. The separate deliberately broken pruning case confirms monitor sensitivity.

The guarantees assume a cooperating private cache root, SQLite's transaction guarantees, a local filesystem honoring sync and OS locks, and producers honoring their completion/size contract. Neither corrupt hardware nor out-of-protocol modifications can be repaired by inferring absent metadata from a tree scan. An unrecoverable database is left in place and reported.

== Backoff and Windows
The filesystem retry policy starts at 2 ms, doubles to at most 64 ms, uses equal jitter, and stops after a 1500 ms budget by default. Nickel exposes each setting. Only idempotent filesystem operations are retried; producers and transactions are not replayed blindly. Debug tracing reports actual retries, while permanent failure preserves the typed source error.

Windows error codes 32 (sharing violation), 33 (lock violation), 5 (possibly transient access denial), and 145 (directory not empty during removal) have explicit retry classification tests. A scanner-held or loaded DLL handle can prevent deletion/rename even after a compiler closes its own handle. The cache must keep Pruning durable and retry rather than advertise Missing or Ready while removal is partial. The library must not depend on disabling Defender. CreateFile delete-sharing flags and directory-handle semantics require native treatment; POSIX unlink behavior cannot be assumed on Windows.

Linux, Windows and macOS are first-class execution targets with native storage implementations and passing lifecycle/recovery tests. Local NTFS on Windows and local APFS/HFS+ on macOS join the existing 64-bit Linux allowlist. Hosted run 40592018954 qualifies native storage and strict Clippy on all three, including ten seconds of multiprocess stress. Windows also exercises held files and a loaded DLL. File sync and process-kill tests alone do not establish power-loss-safe directory publication.

Small platform modules implement local-filesystem inspection while retaining the shared synchronous SQLite state machine. Windows opens writable directory handles with backup semantics for flushing. Its writer-to-reader transition first acquires the overlapping shared lock and then explicitly unlocks the exclusive portion, retaining the reader lease. Unix conversion keeps row/reference revalidation in case ownership was released. Native tests verify reader concurrency, pruning exclusion, delete-sharing failures and loaded-DLL recovery.

On macOS, fstatfs locality flags and filesystem names reject remote/unvalidated mounts. The pinned standard library uses F_FULLFSYNC for sync_all; SQLite enables fullfsync and checkpoint_fullfsync. No directory flush error is ignored. Retained readers, interrupted construction/deletion, migration and recovery pass on the native runner under the shared producer contract.

== References
#link("https://www.sqlite.org/wal.html")[SQLite WAL constraints and durability]

#link("https://p-org.github.io/P/manual/monitors/")[P safety and liveness monitors]

#link("https://github.com/arthurprs/quick-cache")[quick_cache concurrent S3-FIFO implementation]

#link("https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea")[Microsoft CreateFile sharing and directory flags]

#link("https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers")[Microsoft FlushFileBuffers access and flush contract]

#link("https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex")[Microsoft shared/exclusive lock overlap and unlocking rules]

#link("https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html")[Apple fsync and F_FULLFSYNC guarantees]

== Logical and physical schema
The user-facing field names and borrowed values define identity only. The implementation extends one indexed SQLite entries table with kind, key digest, status, logical size, last access, optional blob payload, and optional object_key. A directory's object_key is committed in its Building row before construction; reads, deletion and recovery resolve the managed directory using that recorded reference rather than guessing from user fields. The initial object name is the recipe's BLAKE3-256 digest, but its physical reference is independently represented. Backend root identity is recorded separately in settings. SQLite schema version 2 migrates version 1 by setting directory references to the old key-based names without moving or scanning contents. Integration tests change a reference independently of the logical key and verify both reads and pruning follow metadata.
