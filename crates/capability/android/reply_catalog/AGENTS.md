# Reply Catalog capability

> Serves fixed JNI replies, keyed by (class, method, signature), from a same-build embedded blob.

## Role
Capability `reply_catalog`, `Kind::Runtime`; package `rudroid-android-reply-catalog` (cdylib + rlib `librudroid_android_reply_catalog.so`, deployed as `libreply_catalog.so` through an alias in `scripts/deploy-select.py`).
Deploy it with `android_jni_vm`: `android_base`'s session join fails without it. Other capabilities never link it; they use types from [`format/`](format/) and query through `HostApiV0.catalog_query_int/_bool`.

## Design
- `build.rs` encodes `format::source::SOURCE_TABLE` into `OUT_DIR/reply_catalog.bin`; `src/lib.rs` embeds it and decodes it once into a process `Arc<Catalog>`.
- `rudroid_plugin_early_init` registers a `CatalogProviderV0` whose `user` is a `HandleTable` handle to one `Arc` reference; `destroy` or a rejected registration removes exactly that one.
- Query callbacks borrow the caller's C keys without allocating; a miss leaves the out-parameter untouched.
- `rudroid_plugin_init` joins the shared session and checks `AudioManager.getStreamVolume` returns 100. No session: skip; bad session: -3.
- `ffi.rs` is a direct handle-based C API (`rudroid_reply_catalog_*`) whose names mirror `rudroid_common::symbols::reply_catalog`.

## Constraints
- All provider generations share one immutable catalog; never clone it per provider or per query.
- Null or invalid UTF-8 keys are misses and never write the output.
- Renaming the package or artifact requires updating the alias in `scripts/deploy-select.py`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib; `common`, `format/`; `libloading` for tests |
| Build script | `build.rs` | Encodes the source table into the embedded blob |
| Crate root | `src/lib.rs` | Descriptor, embedded catalog, Host API provider, session join |
| C API | `src/ffi.rs` | Handle-based catalog C API and borrowed-key parsing |
| Unit tests | `tests/unit/` | Provider Arc ownership, allocation-free keys, C API null safety |
| Integration tests | `tests/*.rs` | `ffi_load.rs`: dlopens the built cdylib and checks symbol names |
