#set document(title: "Live payloads and Reussir-shaped cache contracts")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Live payloads and Reussir-shaped cache contracts
Date: 2026-10-28. Status: scoped compcache implementation tested and published as a draft; discussion record added after the user's documentation correction.

== Discussion and approved design
The user rejected a separate live-cache abstraction and a generic lifecycle framework as unnecessary complexity. The agreed direction was to extend the existing `InMemoryCache` trait, use an associated payload type, and expose materialization and lifetime-management hooks. The user then requested Reussir-shaped contract tests before authorizing implementation in compcache.

Ordinary byte consumers must remain straightforward. A live strategy can retain a PJRT-shaped `Arc<Executable>` while SQLite stores only serialized bytes. On a disk hit, a materialization hook uses caller-owned client context. A cold producer returns its already-created payload plus optional serialized bytes, avoiding a compile-serialize-deserialize round trip. A live payload need not retain its serialized representation.

Successful production is distinct from cache success. Serialization or persistence failure must preserve the usable produced object; a real producer/compiler error must remain distinguishable. Memory-only successful values must support hits without a SQLite row. Disabling memory admission, choosing bypass behavior, or exceeding admission weight must not discard the caller-owned value or remove the materialization hook.

== Ownership correction and shared disk identity
The user explicitly corrected the distinction between cache residency and object lifetime. Retaining a memory entry after disk pruning is a policy choice, not a lifetime fix. Use ordinary Arc ownership and Drop: eviction releases the cache's reference without invalidating caller-held Arcs. A native executable should retain its client when destruction requires that client to remain alive.

The user also confirmed that disk cache sharing is required. The client-identity fixture uses the same disk root and the same artifact key for clients A and B. A compiles once and persists bytes; B reuses those bytes without compiling and materializes its own client-owned executable. Client or session identity does not enter the persistent recipe. Process-local context belongs to the memory strategy; program, shape, donation, target, compiler, options and library identities belong to the deterministic recipe as applicable.

The implementation removes this Store's memory reference when it prunes a disk blob. Other Stores' resident snapshots may remain usable after the row disappears. Neither policy revokes externally owned objects or promises global invalidation.

== Force eviction and coordination decisions
Add only `force_evict_blob(key)`: remove the persisted blob and this Store's memory ownership, then let the caller retry ordinary get-or-insert. Invalid-byte repair is a caller-controlled bounded retry. No separate repair framework, expected-version API, or global revocation mechanism is required.

Reuse existing filesystem key leases and quick_cache capabilities. Publication, final memory admission and force eviction must share coordination so that a previously produced in-flight result cannot re-admit after eviction completes. A later request may legitimately repopulate the key. Maintain consistent lock ordering; do not run cache-controlled arbitrary cleanup under internal cache or SQLite locks. Runtime-directory reader leases remain unchanged, with no forced deletion of leased output.

== Agreed implementation plan and completed work
+ Inspect fresh compcache HEAD, local instructions, dependency behavior and existing tests. Work on a dedicated branch; keep Reussir and lean-to-reussir unchanged. The inspected base was `cd16fff9d102b5d4750fce75cc4b140970fec9df`; no repository AGENTS.md or local skill guidance was found.
+ Extend `InMemoryCache` with associated `Payload`, `materialize` and `weight` hooks. Generalize `MemoryEntry` and `QuickCache` storage, retaining byte defaults. Keep the strategy initialized even when admission is disabled. Custom strategies migrate from `MemoryEntry::bytes` to `payload` and return retired entries from mutations.
+ Add `Produced` and `Cached` results. `Produced` carries the usable payload and optional bytes or a serialization error. `Cached` preserves the payload and separately reports serialization, persistence or maintenance errors. Wrap disk-materialization failure in `Error::Materialize`; actual producer failure is `Error::Producer`. Preserve the existing byte convenience API's error behavior.
+ Coordinate disk misses, materialization, production and admission with the existing exclusive filesystem key lease. Keep resident hits free of filesystem locking: cloning a resident object cannot re-admit it. No new initializer/coordinator is introduced; the filesystem lease already coalesces misses, including same-store memory-only success. Do not add a quick_cache placeholder in a conflicting lock order.
+ Inspect quick_cache 0.6.24 rather than assume destructor behavior. Its default lifecycle buffers only two retired values, and ordinary clear drops values under shard locks. Use its lifecycle return API to collect all retired values, return them through the existing strategy interface, and release them after the Store's key lease. Clear a snapshot through removals; concurrent insertion can survive that clear.
+ Add force eviction under the same publication/admission lease. Preserve caller-owned snapshots, other Stores' loaded objects, runtime directory leases and partial-output exclusion. Keep SQLite transactions short, outside producer/materializer execution.
+ Add deterministic fixtures and a runnable live-payload example; document API migration, failure handling and ownership. Adapt existing tests and include the new contract suites in native CI. Run repository checks before publication.

These steps were implemented in commit `c576050b2fcd9cc4c443a811c013c3871f755587` on branch `typed-payload-hooks`. The numbered work record itself was omitted from that commit; this new entry corrects that omission without rewriting earlier records or changing implementation code.

== Reussir-shaped contract coverage
Polyffi fixtures use a counting fake compiler and recipes containing source, options, toolchain and library identities. They cover same-key reuse, changes to each component, invalid cached bytes followed by force eviction and bounded retry, fallback using an already-produced object on cache persistence failure, lookup-error fallback and propagation of real compiler failure.

PJRT fixtures use mock Client and Executable objects with compile, serialize, deserialize and destroy counters. They cover the original cold payload with no deserialize, concurrent same-client cold and warm coalescing, one warm materialization per resident strategy, and the shared-root/shared-key A/B scenario above. Shape and donation changes miss. Serialization and persistence errors preserve memory-only success; explicit no-serialization success, disabled admission, bypass and oversize return usable caller values. External Arcs survive force eviction, pruning and Store drop; native destruction occurs once with the client alive. Capacity eviction, replacement and clear defer all retired values, including more than the upstream default lifecycle's two slots.

The force-eviction fixture gates production and final admission, verifies the producer holds the existing filesystem lease, and races eviction against it. After eviction completes, no old result remains admitted or persisted; a fresh request can repopulate. Cleanup fixtures verify the key lease and SQLite writer lock are available when the native destructor runs.

Rene bake fixtures publish dummy runtime and dependency files plus a relative manifest in a managed directory. Reopening and using another build directory reuse the same artifact. Bundle, toolchain and target recipe changes miss. A held lease protects a simulated linker reader from pruning, and blob force eviction does not delete the leased runtime directory. Existing process-crash and concurrency tests continue to establish that partially built runtime output is not exposed; those tests were reused rather than duplicated.

There are 17 new deterministic mock contract tests across `tests/live_contracts.rs` and `tests/reussir_recipes.rs`. They require no LLVM, GPU, PJRT plugin or actual linker. They are compcache contracts shaped like Reussir consumers, not evidence of real Reussir integration.

== Validation outcomes and limits
Validation of implementation commit `c576050b2fcd9cc4c443a811c013c3871f755587` used Linux and Rust 1.98.0:

+ `cargo fmt --check`: passed.
+ `cargo check --locked --all-targets`: passed.
+ `cargo clippy --locked --all-targets -- -D warnings`: passed.
+ `cargo test --locked -- --skip builder::tests::application_defaults_config_loading`: 57 unit/integration tests and 5 doctests passed, including the 17 new contracts, process-crash recovery and five-second six-process stress. Three subprocess helper tests are ignored as standalone tests and invoked by their parent tests; one Nickel-dependent test was explicitly filtered out.
+ `cargo run --locked --example live_payload -- <temporary-directory>`: passed, demonstrating caller ownership after eviction and Store drop.
+ `git diff --check`: passed.

Nickel was unavailable, so its configuration-loading test was not run. Nix and P-model tooling were unavailable. Windows and macOS were not executed locally for this implementation; earlier native qualification records apply to their historical commits, not automatically to this change. No performance benchmark or physical power-loss validation is claimed.

At initial publication, the GitHub connector reported no PR-triggered workflow runs. An additional Actions listing through `gh run list` returned Forbidden, so hosted CI completion was not verified. These local results do not imply hosted CI success.

== Publication and later scope
Implementation was initially authorized for local work only. The user subsequently requested a PR, authorizing the scoped branch push and draft PR creation. The remote branch and PR head were verified to match implementation commit `c576050b2fcd9cc4c443a811c013c3871f755587` at publication. The PR remains a draft; no merge or readiness transition was authorized.

#link("https://github.com/reussir-lang/compcache/pull/1")[Draft PR 1: Retain typed live payloads in existing memory strategies]

The user then identified the missing agentic-plan discussion section and authorized this documentation correction on the same branch and PR. This entry follows record 00008's append-only policy; previous records remain verbatim. Documentation verification and its publication commit are reported with the correction.

Real Reussir and lean-to-reussir changes, consumer integration, C ABI export, PR revert decisions, and full LLVM/PJRT execution remain later scope. This implementation neither performs those changes nor resolves those later decisions. Shared disk reuse and caller-local native ownership are the agreed contracts for that future integration, subject to real integration validation.

== Documentation correction verification
This new record was compiled successfully with the Typst 0.15.0 Python package, installed into a temporary tool directory for validation. A byte-for-byte comparison against HEAD verified that all 20 previously tracked files under agentic-plan, including evidence files, were unchanged. No previously tracked implementation file changed. `git diff --check` passed. The correction adds only this numbered Typst source; the rendered PDF and temporary validation tooling are not repository deliverables. Rust tests are not repeated for this documentation-only correction; the implementation test results above remain tied to their tested commit.
