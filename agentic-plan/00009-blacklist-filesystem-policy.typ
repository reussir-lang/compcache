#set document(title: "Blacklist-only filesystem policy")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Blacklist-only filesystem policy
Date: 2026-10-27.

== Requirement correction
Filesystem validation must use only blacklists. A local ReFS volume must not be rejected merely because its name differs from NTFS. Earlier name-restricted policies were implementation choices and are superseded by this entry. Historical plan text stays unchanged under the append-only rule recorded in 00008.

== Implementation changes
Windows removes filesystem-name inspection and rejects only remote, unknown and unavailable drive locations. macOS rejects non-local mount flags without a filesystem-name check. Linux rejects known network filesystem types: NFS, SMB/CIFS/SMB2, AFS, Coda, NCP, Ceph and 9p. Unknown types, overlay and FUSE are not rejected solely by type.

SQLite WAL initialization and actual locking/sync operations still have to succeed; failures are returned. Successful system calls do not certify filesystem durability or detect every remote-backed block device or FUSE transport. The caller must choose local storage honoring the shared cache contract.

The current README is changed to describe blacklist-only validation on every platform. Existing plan bodies are not edited. Classifier tests distinguish known rejected locations/types from unrecognized types that may proceed.

== Verification work
Run strict all-target Clippy and the complete local Rust suite, then verify the native CI matrix. A separate optional ReFS-volume test is being prepared so routine CI remains light. Record actual results in appended entries after those checks complete.

== 2026-10-27: Optional ReFS test implementation
Added the manual refs=true CI input and scripts/test-refs.ps1. The Windows fixture creates a unique disposable 2 GiB expandable VHDX, formats that new virtual disk as ReFS, redirects test temporary roots there, runs native storage/recovery suites plus ten seconds of six-process stress, then detaches and removes the fixture in a finally block. No existing volume is formatted. A Rust test verifies that the test temporary directory actually resides under the requested volume root. Routine CI does not create a virtual disk. README documents the optional check.

== 2026-10-27: Local verification before native CI
Formatting, pedantic/nursery all-target Clippy with warnings denied, the complete Rust suite including short multiprocess stress, workflow actionlint and compilation of this Typst record pass on Linux. README and source/test searches find no remaining filesystem-name restriction or Linux positive-list description. All tracked plan logs from commit 41823d3 remain byte-for-byte unchanged; this change adds only record 00009. Native and actual ReFS-volume results are still pending.
