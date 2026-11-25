# Native build

> Native build library that fixes GNU compiler, archiver, flags and link directives for capability C/assembly boundaries and the optional external WSI layer.

## Role
Package `rudroid-native-build`, a plain library with no descriptor and no dependencies, never deployed. Capability/common build scripts use it through build-dependencies. Its WSI builder runs after the complete Rust kit build through `scripts/vulkan-kit.sh`.

## Design
- A crate describes its boundary with `NativeSources`: `archive` stem, `.c`/`.S` `sources`, `headers`, `extra_flags` and `link` libraries.
- `build()` always emits `rerun-if-changed` but compiles only for `aarch64-unknown-linux-gnu`; other targets compile nothing and get `false`.
- `toolchain()` picks `gcc`/`ar` when the host is the target, else `aarch64-linux-gnu-gcc`/`-ar`; flags are `-std=c11` (C only), `-fPIC -fvisibility=hidden -Wall -Wextra -Werror`, extras, then `-O2` (release) or `-O0`.
- Objects keep the full file name (`jump.c.o`, `jump.S.o`); `archive: Some` yields a static `lib<stem>.a` in `OUT_DIR`, `archive: None` is a compile-only ABI gate.
- Optional WSI uses pinned upstream MIT sources and Vulkan headers verified before
  Docker by `scripts/vulkan-input.py`. Existing GNU compilers and wayland-scanner
  build Wayland only, with the first-party Rust wsialloc static archive and exact GBM
  DMA-BUF layouts. No experimental FIFO thread or system installation.
  Null optional dispatch entries reject before native calls; Wayland surface
  creation checks the downstream entrypoint before allocating local objects.
  `RUDROID_WSI_HEADLESS_SURFACE=1` is sampled at instance creation and enables
  the lower headless extension for Wayland only. If native Wayland creation is
  absent, a real headless surface carries the existing Wayland object/map;
  system mode still rejects. Carrier creation rejects unsupported pNext/flags,
  preserves allocation callbacks and destroys the lower handle on map failure.
  Release keeps ERROR logs; ordinary logs require explicit diagnostics.
  Artifacts, the root noncommercial LICENSE/NOTICE, dependency notices and
  upstream provenance stay in `vulkan-wsi/`.
- Optional portable Wayland clients build the pinned SDL source with GNU
  configure/make and copy a pinned generic Vulkan loader. No system installation;
  these external libraries have their own target ABI audit and notices.

## Constraints
- Never read `CC`/`CFLAGS` or other env overrides: flags are source code and part of the kit fingerprint.
- New C or assembly boundaries build through this crate; no hand-written gcc/ar calls in a `build.rs`.
- Never a normal dependency of a capability. The optional post-build CLI is a build tool only.
- Do not rename the directory to `build-*`: the root `.gitignore` ignores that pattern.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library with no dependencies |
| Native contract | `src/lib.rs` | `NativeSources`, `build`, flags, object names, toolchain choice |
| External WSI dependency | `src/vulkan_wsi.rs`, `src/wsi_main.rs` | Optional fixed GNU ARM64 build after the Rust kit; never linked into a capability |
| Portable clients | `src/portable_wayland.rs` | Fixed Wayland SDL build and generic Vulkan loader bundle, selected only for an optional private compositor route |
| WSI ABI gate | `src/vulkan_wsi_abi.c` | Compile-time native wsialloc layout assertions |
| Vendor patch | `patches/vulkan-wsi-native-allocation.patch` | Allocation/sync ownership, null-dispatch rejection, Release logging and explicit real headless carriers; emits `wsi/wayland/surface_carrier.hpp` |
| Unit tests | `tests/unit/lib.rs` | Optimization per profile, C vs assembly args, object names, toolchain |
| WSI contract | `tests/unit/vulkan_wsi.rs`, `tests/fixtures/vulkan_wsi_contract.cpp` | Patched dispatch/log/carrier helpers, original-source negative control and CPU-only extension, ownership and cleanup checks |
