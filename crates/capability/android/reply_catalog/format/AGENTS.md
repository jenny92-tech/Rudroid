# Reply Catalog format

> Plain library with the Reply Catalog blob codec, the read-only `Catalog` query model and the in-repo source table.

## Role
Plain library, no descriptor; package `rudroid-reply-catalog-format`, an rlib with no dependencies. `reply_catalog` (also as build-dependency), `android_jni_vm` and `android_base` each link their own copy; runtime queries still go through the `reply_catalog` provider.

## Design
- Blob v1 (little-endian): `BDRC` header with version, entry count and string-pool size; the UTF-8 pool; 44-byte records (key offsets, value kind 0 bool/1 int/2 str, 16-byte payload).
- `encode` is deterministic; `decode` validates header, version, bounds and value kinds.
- `Catalog` freezes owned keys in a sorted `Vec` (duplicates keep the last value) and uses binary search; typed helpers return the caller's fallback on miss or type mismatch.
- `SOURCE_TABLE` holds the fixed replies as `const` data: the `android_base` `setDefault` values and `org/fmod/FMOD.checkInit`.

## Constraints
- Stay dependency-free and build-script safe; never add a descriptor or a capability dependency.
- Source values must be byte-identical to the C++ oracle (`android_base.cpp`, `javastubs/android.h`, `sdk_fmod/fakefmod.cpp`); no placeholder entries.
- Any layout change bumps `FORMAT_VERSION`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library, no dependencies |
| Codec | `src/lib.rs` | `ReplyValue`, `Entry`, `encode`/`decode`, errors, format constants |
| Query model | `src/catalog.rs` | `Catalog` construction and binary-search lookup |
| Source table | `src/source.rs` | `SourceEntry` and `SOURCE_TABLE` |
| Unit tests | `tests/unit/` | Layout, round trip, rejection, lookup, fallbacks, source-table contents |
| Integration tests | `tests/*.rs` | `catalog_query_cost.rs`: hit/miss and construction cost profile |
