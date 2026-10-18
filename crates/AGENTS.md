# Rust workspace

> ARM64 Android Unity compatibility runtime: Android / Linux / Unity capabilities built as dynamic libraries, deployed by copying a subset, loaded by a thin entry.

## Role

Contains Rudroid's Android, Linux and Unity capabilities and the thin loader.
Current bounded acceptance covers five MiniLoong and TrimUI/A523 GLES titles
([scope and 1 GiB limits](docs/ACCEPTANCE.md));
Host Config TOML stays compatible with existing launchers. The sibling Bogodroid
C++ tree is a read-only behavioral reference, not a build dependency.
Terms: [CONTEXT](../CONTEXT.md); comparison rules: [REFERENCE](docs/REFERENCE.md).

Development/build/deploy order and known pitfalls: [DEVELOPMENT.md](../DEVELOPMENT.md).

## Design

- `cargo build --workspace` builds everything; Deploy Selection (`scripts/deploy-select.py`) copies the chosen `lib<id>.so` files and the loader dlopens them. The id is the file stem; `reply_catalog` is the one alias (built as `librudroid_android_reply_catalog.so`). A kit needs exactly one `android_base`.
- Capabilities never link each other. Shared code lives in plain libraries without a descriptor (`common`, `common/host-config`, `reply_catalog/format`, `addon/after_unity_module`, `unity/player_lifecycle`); each cdylib links its own copy, so their statics are never shared. Runtime interaction goes through `HostApiV0` and the shared VM.
- `common/build.rs` stamps a kit-wide `runtime_build_id` (hash of all workspace sources, manifests, `Cargo.lock`, target, codegen flags and `RUDROID_KIT_FEATURES`); the loader rejects mixed kits. Feature-changing builds go through `scripts/build-kit.sh`.

## Constraints

- First-party code uses the root [noncommercial source-sharing license](../LICENSE).
  Keep third-party source notices intact and include their terms with binaries;
  the scope and previous GPL copies are recorded in [NOTICE](../NOTICE).
- Every native init/hook boundary requires the same-kit Host API to keep callback
  and resolved code mapped, deliver valid events and provide a quiescent installation window.


- Crate layout: `src/` is implementation only; unit tests in `tests/unit/` (declared from the source file with `#[cfg(test)] #[path]`), integration tests in `tests/*.rs`, guest C and fixture crates in `tests/fixtures/`. One English AGENTS.md per crate; none in between.
- Tests that dlopen a workspace cdylib rebuild it through `test-support` first (same profile, `CARGO_TARGET_DIR` or `crates/target`, kit features). A test that cannot run fails or is `#[ignore = "reason"]`, never returns early.
- Test-only features never ship: members exporting test markers are rejected by the loader, `deploy-select.py` and `kit-manifest.py` (keep `TEST_BUILD_MARKERS` and `scripts/kit_markers.py` in sync).
- Addons go under `capability/android/addon/`, host backends under `capability/linux/`, Title patches under `capability/unity/gamepatch/`. Capability ids are deploy/config contracts ([capabilities](docs/MIGRATION-MAP.md)). No static "assemble" binary.
- C/assembly compiles only through `native-build`. Preserve the workspace's explicit module layout; do not introduce directories named `build-*` or files named `mod.rs`. Inspect the current root `.gitignore` when adding files; historical ignore rules from the sibling repository are not authoritative here.
- Unsafe: business code (natives, Host API use, callbacks, IL2CPP lookups) has none; it uses the safe tools in `common` (`jni_env`, the typed `host_api` table, `registry::HostStr`, `handles`, `il2cpp`, `arm64_detour::Original`). `unsafe` remains only at C-ABI boundaries (guest libc, ELF mapping, the JNI VM, GL/EGL, detours and their hook bodies, calls on managed IL2CPP objects), each block with a `// SAFETY:` line. Modules without unsafe carry `#![forbid(unsafe_code)]`; `common/tests/unsafe_boundaries.rs` pins JNI/Host API access and an exact per-crate `unsafe` budget. Need unsafe in business code? Add a safe tool to `common` instead.
- New handheld/title work follows [the porting guide](docs/HANDHELD-PORTING-WORKFLOW.md).
  Map requirements to existing capabilities before adding code; reuse
  platform records and inspect actual guest/host ABI, engine/backend and
  dependencies. Review the closest reference and fix one issue per
  backup/build/test/log iteration within the authorized scope. Verify native
  loading, engine, display and gameplay separately, retaining platform/title
  and iteration evidence. ARM32/Mono and other engine execution are separate
  unverified capabilities, not implied by the broader guide.

## Docker

Only `bash crates/scripts/docker-rust.sh` may build/check/test Rust in Docker.
The fixed image/container, resources, cache, ThinLTO/PGO options and real-game
input rules live in [BUILD-DOCKER](../BUILD-DOCKER.md). Never create ad hoc
images/containers/volumes/builders, install toolchains or prune globally.
Keep workspace inputs unchanged during runs; deploy only a complete same-kit
production build. PGO training and fixture-enabled outputs cannot ship.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest / lock | `Cargo.toml`, `Cargo.lock` | Member list (fixture crates excluded); pinned versions |
| Common | `common/` | ABI types, Host API, descriptors, kit id, runtime policy, native filesystem snapshots, ARM64 detour, typed JNI wrappers, IL2CPP helpers |
| Host Config | `common/host-config/` | Host Config TOML reader |
| Vulkan allocation | `common/vulkan-wsi-allocator/` | Plain static library: exact-layout GBM DMA-BUF ownership for optional native WSI |
| Native build | `native-build/` | Every C/assembly boundary and optional external WSI post-build tool |
| Test support | `test-support/` | Dev-dependency: locks, log capture, nested cargo and C fixture builds |
| ELF loader | `capability/android/elf_load/` | `android_elf_load`: guest ELF mapping, relocation, symbols, libdl |
| Bionic | `capability/android/bionic/` | `android_bionic`: Android libc/libm/liblog over host glibc |
| JNI VM | `capability/android/jni_vm/` | `android_jni_vm`: the shared JNI session |
| Framework | `capability/android/framework/` | `android_base`: Android/Java facade, Context, Looper, input |
| Native assets | `capability/android/assets/` | `android_assets`: NDK file/map/directory ownership over the host resource authority |
| Extracted packs | `capability/linux/android_asset_packs/` | Presence-selected `platform_android_asset_packs` overlay, before JNI preparation |
| Native events | `capability/android/native_events/` | `android_native_events`: NDK Looper/epoll events |
| Media fallback | `capability/android/media_fallback/` | `android_media_fallback`: typed NDK media failures |
| zlib | `capability/android/zlib/` | `android_zlib`: zlib ABI over host libz |
| Reply catalog | `capability/android/reply_catalog/` | `reply_catalog`: fixed JNI replies; `format/` is its plain library |
| Addons | `capability/android/addon/` | `gms`, `play_core`, `levelplay`, `fmod`, and `unity_burst`/`unity_helpers`/`xlua` on `after_unity_module` |
| SDL runtime | `capability/linux/sdl_runtime/` | `platform_sdl_runtime`: window/GLES, optional Vulkan, guest EGL, audio, input |
| stderr log | `capability/linux/log_stderr/` | `platform_log_stderr`: optional log sink |
| Player lifecycle | `capability/unity/player_lifecycle/` | Plain library: Unity prepare/bootstrap/run shared by profiles |
| Unity profiles | `capability/unity/profile_2021_3/`, `profile_6/` | `unity_2021_3` / `unity_6`: Spec plus version patch |
| IL2CPP hooks | `capability/unity/il2cpp_hook/` | `il2cpp_patch`: metadata-driven IL2CPP hooks, `Il2cppReady` event |
| Memory policy | `capability/unity/memory_policy/` | `unity_memory_policy`: pressure-driven Unity reclamation |
| Title patches | `capability/unity/gamepatch/` | `silksong_unity6`, `hollow_knight_viewport`, `vampire_survivors`, `terraria_android` |
| Unity loader | `project/unity_loader/` | `rudroid-unity-loader`: loading, validation, registry, guest bring-up |
| Docker entry | `scripts/docker-rust.sh` | The only Docker build/check/test entry |
| Optional Vulkan sources | `scripts/vulkan-input.py` | Pinned, SHA256-verified WSI/headers in the existing target cache; no toolchain installation |
| Kit build | `scripts/build-kit.sh`, `scripts/vulkan-kit.sh`, `scripts/cross-build-linux.sh` | Feature-recording build, optional WSI post-build and Linux ARM64 cross build |
| Deploy | `scripts/deploy-select.py`, `scripts/kit-manifest.py`, `scripts/kit_markers.py` | Copy a kit subset; record/verify kit ids; test-marker table |
| ABI audit | `scripts/linux_abi.py` | Static ELF glibc-ceiling audit (default 2.28) |
| Executable game view | `scripts/guest-exec-view.py`, `scripts/test_guest_exec_view.py` | Opt-in Linux launcher namespace: read-only executable native-library view on noexec game media; existing ELF mapper/fallback, no resource copies or resident supervisor |
| PGO input | `scripts/pgo-input.py`, `scripts/test_pgo_input.py` | Bounded immutable profile snapshots, content-addressed compiler identity and isolation regressions |
| Script tests | `scripts/test_deploy_select.py`, `scripts/test_linux_abi.py` | Python regressions for deploy and ABI audit |
| Docker image source | `scripts/rudroid-rust-gles.Dockerfile` | Pinned public-base bootstrap recipe; cold reconstruction still needs validation and explicit authorization; builds use docker-rust.sh |
| Deploy profiles | `deploy/` | Per-Title member lists (`silksong.txt`, `terraria.txt`); pair Terraria with `../configs/terraria.toml` for its save-directory layout |
| Docs | `docs/` | Current porting, acceptance, performance, configuration, compatibility, capability and reference guides; index in ../docs/README.md |
