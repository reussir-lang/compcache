#set document(title: "Whole-key serde design and progress")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Whole-key serde identity

== Accepted user decision
Support general serde values through whole-key serialization. The user chose this after discussion of serializing the whole recipe versus retaining KEY_FIELDS with heterogeneous serialized values. Keep the existing borrowed-string SchemaKey path available.

== Work plan and implementation
+ Add `Key::from_serde(namespace, &recipe)` accepting any serde Serialize value, returning the existing Key type through the library Result.
+ Use a deterministic, versioned binary encoding with BLAKE3-256, canonical map ordering and ordered sequences. Preserve serde value kinds, variants, optional values and floating-point bits.
+ Keep the physical SQLite metadata schema and the existing SchemaKey digests intact. No new serialization-format dependency is needed.
+ Preserve typed errors for serialization failure and malformed/duplicate map entries.
+ Validate nested recipes, field/map ordering, integer widths, floats, enum shapes, serialization attributes, binary-mode custom serialization, and cross-process identity/persistence. Benchmark key derivation independently from cache lookup.

Implementation is divided into a public entry/error module, a serializer module and compound-value encoders. Each remains below 220 lines after formatting. The basic example now derives Serialize for a borrowed recipe containing input digests, optimization level and feature strings. Existing subprocess crash/deduplication tests derive their keys through serde.

== Encoding contract, version 1
The hash input starts with the literal bytes `compcache/serde-key/v1` followed by a zero byte, then an unsigned 64-bit little-endian namespace byte length, namespace UTF-8 bytes, and one encoded serde value. All lengths are unsigned 64-bit little-endian. Type tags are one byte.

#table(
  columns: (1fr, 2fr),
  [Tags], [Representation],
  [01], [Boolean, followed by 00 or 01],
  [02–06], [Signed integers i8/i16/i32/i64/i128, fixed-width little-endian],
  [07–0b], [Unsigned integers u8/u16/u32/u64/u128, fixed-width little-endian],
  [0c–0d], [f32/f64 IEEE bit patterns, little-endian],
  [0e], [Character code point as little-endian u32],
  [0f–10], [UTF-8 string / byte string, length followed by bytes],
  [11–12], [None / Some followed by its encoded value],
  [13–14], [Unit / unit struct],
  [15], [Unit variant, length-framed variant name],
  [16], [Newtype struct, followed by its encoded value],
  [17], [Newtype variant, framed variant name then encoded value],
  [20–22], [Sequence / tuple / tuple struct; encoded items then ff terminator],
  [23], [Tuple variant; framed variant name, encoded items, ff terminator],
  [24–25], [Map / named struct; entry count then sorted encoded key/value pairs],
  [26], [Struct variant; framed variant name then map entry count and pairs],
)

Map entries and struct fields are sorted lexicographically by the full encoded key bytes. Sorting applies recursively, including maps used as map keys. Duplicate encoded keys return a typed error instead of choosing a value based on traversal order. Sequence length hints do not affect identity; maps record their actual entry count. Rust struct/type names and enum declaration indices are omitted; the caller's namespace supplies semantic scope, while serialized field/variant names contribute to identity.

The implementation uses a cache-specific encoding of the serde data model. It preserves None versus Some(unit), integer width/signedness, byte versus text/sequence kinds, tuple versus sequence, and every floating-point bit, including signed zero and NaN payload. Type tags and boundaries distinguish values and compound structures. Future encoding changes require a new hash prefix; existing constructors are unchanged.

Custom Serialize implementations run with is_human_readable=false and must be deterministic. Only emitted fields/values participate, including serde rename/skip behavior. Unordered sets are sequences in serde and require stable caller ordering. This cannot infer semantic equality beyond the serialized projection. Keep recipes compact, preferably using BLAKE3 input digests, and compute keys once for repeated cache operations.

== Compatibility vector
Namespace `fixture-v1`, serde tuple `("rustc", 1u64, Some(true))` produces:
`fcd740165508a81bd1a9df0a4f183823ab6035f20497386f58d1ecc24abe9990`.
A fixed regression test protects persisted identities across dependency/compiler changes.

== Validation and performance
Completed inside the pinned Nix shell with stable Rust 1.98.0: formatting, Clippy with warnings denied, the complete cargo test suite (including subprocess crash/deduplication and six-process stress), and the Nickel-configured basic example all pass. Seven new serde-key integration tests cover nested recipes, recursive map ordering and non-string map keys, named field ordering, serde rename/skip projection, all primitive numeric widths, tuple/sequence/byte distinctions, all enum shapes, optional/unit distinctions, float bit identity, custom binary-mode serialization, typed errors, duplicate map rejection and the persisted digest vector. The vector passed with both the installed nightly compiler and pinned stable toolchain. The cache lifecycle/P model is unchanged.

Focused Criterion measurements used release builds, 30 samples, a 1 s warmup and 2 s measurement, on the previously recorded AMD Ryzen AI MAX+ 395 host. The benchmark ran after validation, separately from stress tests.

#table(
  columns: (2fr, 1fr),
  [Recipe], [Central estimate],
  [Existing borrowed-string SchemaKey, four fields], [495.75 ns],
  [Serde struct: compiler, target, empty input-digest list, optimization], [1.0535 µs],
  [Same serde struct, one 32-byte input digest], [1.3646 µs],
  [Same serde struct, sixteen 32-byte input digests], [5.1288 µs],
)

These measure complete key construction, including canonical sorting/allocation and BLAKE3. The two APIs encode different recipe representations; this is not a claim of equivalent work per measurement. The resulting digest is reused by cache lookups, which incur no serde cost. No new hot-path performance issue was found. Raw validation and benchmark output are saved under agentic-plan/evidence/serde-key-validation.txt and serde-key-benchmark.txt.

== 2026-10-27: Key-interface clarification
Key identity does not inherently require serde: Key.from_parts accepts canonical recipe bytes, Key.from_bytes accepts an existing BLAKE3-256 digest, and SchemaKey supports named borrowed fields. The serde constructor supplies convenient traversal of nested values under the encoding contract above. It does not run on repeated cache lookups. Standard Rust Hash does not promise a stable, portable input encoding and is not a replacement for that contract. Serde also remains a configuration dependency; no adapter removal or dependency feature split is implemented by this clarification.
