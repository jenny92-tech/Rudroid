# NDK media failure boundary

> `android_media_fallback` lets a guest handle unavailable video through typed NDK media failures; it contains no decoder.

## Role
Capability `android_media_fallback`, `Kind::Runtime`, `requires = android_elf_load`, artifact `libandroid_media_fallback.so` (cdylib + rlib). Optional: deploy it only when the host has no Android media backend and the Title tolerates failed video setup; the Unity loader registers `rudroid_media_fallback_symbol` with `android_elf_load` before guest relocation and revokes it if bring-up fails. Guest end-to-end coverage lives in `project/unity_loader/tests/media_fallback.rs`.

## Design
- The source accepts only bare queries (null library and version) and forwards them to the C table; everything else returns 0.
- `media.c` exports 15 `AMEDIAFORMAT_KEY_*` pointer globals and 39 typed `AMediaExtractor_*`/`AMediaDataSource_*`/`AMediaCodec_*`/`AMediaFormat_*` functions returning permanent unsupported/invalid results.
- Extractor and DataSource factories `calloc` a 4-byte native token freed by the matching delete; no codec or format object is ever created.
- `rudroid_plugin_init` logs the unavailable-media policy once through the Host API.

## Constraints
- Failed setup never adopts, reads, closes or retains the caller's fd, userdata or callbacks, and never starts a stream, worker or callback.
- Keep prototypes and constants aligned with the Android 15 NDK declarations; invalid codec dequeue returns a permanent error, never `TRY_AGAIN_LATER`.
- No generic host `dlsym` fallback and no Rust allocation across the boundary; a Title that needs real video needs a separate implementation.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_media_fallback`; `common` only |
| Build script | `build.rs` | Private archive `rudroid_media` from `src/media.c` via `native-build` |
| Source | `src/lib.rs` | Descriptor, bare-name-only source, one-time policy log |
| Media ABI | `src/media.c` | Pointer globals, typed failure functions, factory tokens |
| Unit tests | `tests/unit/lib.rs` | Data-global indirection, library/version and unknown-name rejection |
