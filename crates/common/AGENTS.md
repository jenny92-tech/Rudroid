# Common

> Plain shared library of kit contracts: descriptor, Host API and registry ABI, export names, kit build identity, runtime policy values and pure ABI helpers.

## Role
Package `rudroid-common`, a plain library with no descriptor, never deployed; the loader, every capability, `test-support` and fixture crates each statically link their own copy. Runtime authority (registry, shared VM, guest mappings) lives in the loader behind the Host API. `host-config/` holds the separate Host Config reader crate. Unity/IL2CPP/Mono helpers live in `rudroid-unity-common`.

## Design

- `build.rs`/`kit_hash.rs` stamp all workspace source, embedded RGBA pixels,
  manifests, lockfile and codegen/features into one kit identity. PGO training
  exports collectors; normal/feedback builds do not. See the build guide.
- Descriptors/registry PODs and typed `HostApiV0` (`HOST_API_VERSION` **7**) connect
  independently linked capabilities. Version 7 adds named services
  (`register_service` / `query_service`, plus typed `ServiceContract` helpers
  `register_service_typed` / `query_service_typed`), `config_set_string`, and
  exact-path file overlays (`register_file_overlay`). Strings/code stay
  process-pinned; singular slots are first-wins, callbacks run outside locks.
  No per-capability static becomes shared authority. No Unity/IL2CPP types here —
  those live in `rudroid-unity-common`.
- JNI wrappers validate full descriptors and pair scoped borrows/local references
  with Drop. HandleTable uses never-reused ids. Unity player input / `nativeDone`
  teardown live in `rudroid-unity-common::player_jni`. Engine-neutral
  `shutdown_runtime_lifecycle` only closes `org/rudroid/runtime/Lifecycle`
  (`shutdown` then `finish`), preserving the first exception.
- Host auto-publishes read-only named service `rudroid.host.config_toml.v1`
  (`HostConfigTomlV1`); callers use `HostApiV0::config_toml()`.
  `config_set_string` is limited to first-write-wins
  `package.mainIntentBundle.<key>`; `config_array_size` returns pure array length.
- Detours publish typed originals only after checked startup writes, cache flush
  and sealing. Unsupported instructions reject before writing; failed patches
  roll back fully. Mappings/code must remain owned and target execution idle.
- Filesystem snapshots, display geometry, frame timing and audio backpressure
  are shared bounded helpers. They add no device owner, periodic scan or worker.

- Trusted JNI class declarations use `TypeRegistry.declareClass`; registering
  natives, fields or parents must not depend on a guest lookup manufacturing
  an absent class. `install_class_catalog` transfers verified application names
  to the shared VM. The pure `dex_classes` reader indexes only class_defs from
  ordinary little-endian DEX035, with explicit size/name/encoding bounds; it
  executes no code and does not infer class linkability or method support.

## Constraints
- No descriptor, no `cdylib`, no capability or `host-config` dependency; runtime dependency `libc`. Statics are per-cdylib copies, so shared runtime state goes through `HostApiV0`.
- `HostApiV0`, registry PODs, `Kind` and `ModuleKind` mirror the C++ reference; grow `HostApiV0` only at the tail with a version bump. The kit hash takes no timestamps, git metadata or absolute paths, and `crates/scripts/kit-manifest.py` parses its format.
- Capabilities read JNI tables only through `jni_env.rs`, call Host API entries through `HostApiV0` methods; `tests/unsafe_boundaries.rs` fails on any other numbered slot read or fn-pointer call. IL2CPP binding lives in `rudroid-unity-common`.
- A safe tool must be sound for every input safe code can build: handle types have private constructors, strings handed to the host are `'static`, and anything touching a managed object or raw guest memory stays `unsafe fn`.
- `JniEnv::with_string_utf{,16}` lends only scoped immutable views and pairs every native borrow with one Drop guard, including unwind. Owned-string convenience methods reuse it; callbacks cannot return borrowed Rust views.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library `rudroid-common`; only dependency `libc` |
| Kit build id | `build.rs`, `kit_hash.rs` | Workspace hash injected as `RUDROID_RUNTIME_BUILD_ID` |
| Crate root | `src/lib.rs` | Module list and common re-exports |
| ARM64 detour | `src/arm64_detour.rs` | Inline detours, non-replacing trampolines, typed originals, checked quiescent image patches, whole-patch rollback and I-cache flush |
| Host PGO | `src/pgo.rs` | Conditional per-image LLVM counter write and non-production collector export; no guest buffers, worker or frame hook |
| Build id | `src/build_id.rs` | Compiled and active kit id, same-build check |
| Logging policy | `src/logging.rs`, `tests/unit/logging.rs` | Explicit cached diagnostics, lazy formatting, error tags and per-image Release/Debug exports |
| Descriptor | `src/descriptor.rs` | `Kind`, C descriptor layout, `declare_descriptor!`, package gate |
| Display geometry | `src/display_dimensions.rs` | Pure validated output/render dimensions and coordinate mapping shared by native and Java/input facades |
| Graphics features | `src/graphics_features.rs` | Compiled backend feature bits and bounded launcher tokens; no hardware or title acceptance claim |
| Host filesystem | `src/filesystem.rs` | Real statvfs/fstatvfs snapshots, noexec queries and bounded kernel-sealed File owners; creation/adoption and scalar sealing are the only new native operations |
| ELF identity | `src/elf_identity.rs` | Bounded cold ELF64/AArch64 GNU build-note preflight over a borrowed file, no mapping/execution/cursor mutation |
| Frame timing | `src/frame_timing.rs` | Opt-in fixed-capacity frame work histograms |
| Audio backpressure | `src/audio_backpressure.rs` | Backend-independent bounded wait arithmetic from actual callback geometry and queue deficit |
| Handles | `src/handles.rs` | `HandleTable<T>`: serial-number handles for C `user`/session values |
| Host API | `src/host_api.rs` | Typed `HostApiV0` (v7) and entry types; safe methods (`config_*` including `set_config_string` / `config_toml`, `log_line`, `guest_symbol`, `java_vm`, `fire_event`, `on_module_loaded`, `register_*` including profile/hooks/named service/`ServiceContract` typed helpers/file overlay, `catalog_*`); `HostConfigTomlV1` (`rudroid.host.config_toml.v1`); `stubbed()` test table |
| Native alias ABI | `src/native_alias.rs` | `NativeAlias` / `GuestLibdl`; `GuestLibdl` implements `ServiceContract` as `rudroid.elf.guest_libdl.v1` |
| Android assets | `src/android_assets.rs` | Safe extracted base/pack resolution, critical-payload identity, bounded replies, fonts/cache/analytics path policy; host owns publication |
| DEX class names | `src/dex_classes.rs`, `tests/unit/dex_classes.rs` | Bounded DEX035 definition-only index; referenced types are not definitions |
| SDK tasks | `src/jni_tasks.rs` | Shared completed local GMS/Play Core Tasks; VM-owned queued listener/result snapshots, main-Looper proxy callbacks, explicit queue refusal |
| JNI ABI | `src/jni_abi.rs` | `function_slot`, `JValue`, `NativeMethod`, safe session helpers on `JniEnv` (fields, superclasses, proxies, main Looper, `shutdown_runtime_lifecycle`) |
| JNI wrappers | `src/jni_env.rs` | Named JNI 1.6 slots, typed `JavaVm`/`JniEnv`, shared class-cast queries, descriptor parser, checked `Method`/`Arg` calls, primitive-array region copies and `Native` registration |
| Module kinds, registry | `src/module.rs`, `src/registry.rs` | Module kinds and lifecycle events; C-layout registration structs; `HostStr<'a>` for host-lent C strings |
| Native clock | `src/clock.rs` | Allocation-free OS monotonic nanoseconds shared by JNI frame time and input; no per-library epoch |
| Runtime policy | `src/runtime_policy.rs` | Memory modes, target FPS, remaining frame time |
| Thread placement | `src/thread_placement.rs` | Opt-in asymmetric topology plan and current-thread-only affinity; bounded cold snapshots, no TID ledger, hotplug, frequency changes or worker |
| Deploy scan, exports | `src/scan.rs`, `src/symbols.rs` | Deploy-dir layout and stems; C export names and callback types |
| Unit tests | `tests/unit/` | One file per module via `#[path]`, plus kit hash and Host API copy tests |
| Integration tests | `tests/*.rs` | `kit_identity.rs` kit hash over the real workspace; `unsafe_boundaries.rs` JNI/Host API boundary scan and per-crate `unsafe` budget |

Typed title settings share `config_reader`. `mapped_memory::MappedRegions` bounds process mapping snapshots; `mapped_library` owns checked guest/host mapping views. The SDL symbol/report contract lives in `sdl_abi`, and ELF and SDL providers share the `native_error` diagnostic reader. Instruction relocation lives in `arm64_instructions`.
