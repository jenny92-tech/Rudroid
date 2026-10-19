# Common

> Plain shared library of kit contracts: descriptor, Host API and registry ABI, export names, kit build identity, runtime policy values and pure ABI helpers.

## Role
Package `rudroid-common`, a plain library with no descriptor, never deployed; the loader, every capability, `test-support` and fixture crates each statically link their own copy. Runtime authority (registry, shared VM, guest mappings) lives in the loader behind the Host API. `host-config/` holds the separate Host Config reader crate.

## Design

- `build.rs`/`kit_hash.rs` stamp all workspace source, embedded RGBA pixels,
  manifests, lockfile and codegen/features into one kit identity. PGO training
  exports collectors; normal/feedback builds do not. See the build guide.
- Descriptors/registry PODs and typed `HostApiV0` connect independently linked
  capabilities. Strings/code stay process-pinned; singular slots are first-wins,
  callbacks run outside locks. No per-capability static becomes shared authority.
- JNI wrappers validate full descriptors and pair scoped borrows/local references
  with Drop. HandleTable uses never-reused ids; IL2CPP binds metadata and exact
  signatures once, with callback-local GC roots and verified GC-handle ABI.
  Nested-class metadata snapshots reject foreign handles, missing exports and
  repeated/nonterminating cursors; they own no managed objects or background work.
- Detours publish typed originals only after checked startup writes, cache flush
  and sealing. Unsupported instructions reject before writing; failed patches
  roll back fully. Mappings/code must remain owned and target execution idle.
- Managed hooks, HUD and scene helpers enforce owner-thread/stop checks, root
  objects across reentrant calls and release roots on error/unwind. Scalar hook
  ABIs use the private AAPCS64 gateway; no managed pointer persists in a plan.
- `finish_player` quiesces independent services, drains nativeDone and then joins
  final services/workers, preserving the first error. It never injects nativePause;
  see the [terminal fixture](../capability/unity/profile_2021_3/tests/fixtures/terminal-lifecycle.md).
- Filesystem snapshots, display geometry, frame timing and audio backpressure
  are shared bounded helpers. They add no device owner, periodic scan or worker.

## Constraints
- No descriptor, no `cdylib`, no capability or `host-config` dependency; runtime dependency `libc`, build-only `native-build` for the private managed AAPCS64 gateway. Statics are per-cdylib copies, so shared runtime state goes through `HostApiV0`.
- `HostApiV0`, registry PODs, `Kind` and `ModuleKind` mirror the C++ reference; grow `HostApiV0` only at the tail with a version bump. The kit hash takes no timestamps, git metadata or absolute paths, and `crates/scripts/kit-manifest.py` parses its format.
- Add a Unity build to `il2cpp_abi::supported_gc_handle_abi` only with native ABI evidence; unlisted builds skip the calls, with no nearby-version fallback or override.
- Capabilities read JNI tables only through `jni_env.rs`, call Host API entries through `HostApiV0` methods and bind IL2CPP only through `il2cpp.rs`; `tests/unsafe_boundaries.rs` fails on any other numbered slot read or fn-pointer call.
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
| Descriptor | `src/descriptor.rs` | `Kind`, C descriptor layout, `declare_descriptor!`, package gate |
| Display geometry | `src/display_dimensions.rs` | Pure validated output/render dimensions and coordinate mapping shared by native and Java/input facades |
| Graphics features | `src/graphics_features.rs` | Compiled backend feature bits and bounded launcher tokens; no hardware or title acceptance claim |
| Host filesystem | `src/filesystem.rs` | Real statvfs/fstatvfs snapshots, noexec queries and bounded kernel-sealed File owners; creation/adoption and scalar sealing are the only new native operations |
| ELF identity | `src/elf_identity.rs` | Bounded cold ELF64/AArch64 GNU build-note preflight over a borrowed file, no mapping/execution/cursor mutation |
| Frame timing | `src/frame_timing.rs` | Opt-in fixed-capacity frame work histograms |
| Audio backpressure | `src/audio_backpressure.rs` | Backend-independent bounded wait arithmetic from actual callback geometry and queue deficit |
| Gamepatch support | `src/gamepatch_support.rs` | Typed detour installer, hook install events, config key fallback chain |
| Handles | `src/handles.rs` | `HandleTable<T>`: serial-number handles for C `user`/session values |
| Host API | `src/host_api.rs` | Typed `HostApiV0` table and entry types, safe methods (`config_*` (including presence plus fallback), `log_line`, `guest_symbol`, `java_vm`, `fire_event`, `on_module_loaded`, `register_*`, `catalog_*`), `stubbed()` test table |
| Android assets | `src/android_assets.rs` | Safe extracted base/pack resolution, critical-payload identity, bounded replies, fonts/cache/analytics path policy; host owns publication |
| IL2CPP | `src/il2cpp.rs` | `Il2cpp` export binding, typed metadata and `StaticI32Method` (unsafe execution, verified ABI), Unity version, `GcHandles`/`GcRoot` |
| Managed scene objects | `src/unity_objects.rs` | Checked Behaviour/active Sprite queries and named GameObject.Find/SetActive, shared rooted helpers, exact Transform+Sprite membership, names and cancellation; no raw array layout or retained managed pointers |
| Configured managed hooks | `src/managed_hooks.rs`, `src/managed_gateway.S` | Exact assembly/unique method/full scalar-reference ABI plans, metadata-selected field operations, register-preserving original calls and unwind-local roots; process-pinned startup hooks |
| HUD layout | `src/hud_layout.rs` | Metadata-selected Component fields, rooted GameObject/Transform/RectTransform vector/visibility changes; callback-local ownership and stop/owner checks |
| Touch controls | `src/touch_controls.rs` | Metadata-checked TouchManager array/bool fields, bounded Array access and null-element fallback; valid stock setter preserved |
| IL2CPP ABI | `src/il2cpp_abi.rs` | Exact version and stripped native-build GC-handle evidence; safe bounded cold process-memory reads |
| SDK tasks | `src/jni_tasks.rs` | Shared completed local GMS/Play Core Tasks; VM-owned queued listener/result snapshots, main-Looper proxy callbacks, explicit queue refusal |
| JNI ABI | `src/jni_abi.rs` | `function_slot`, `JValue`, `NativeMethod`, safe session helpers on `JniEnv` (fields, superclasses, proxies, main Looper, player input/teardown) |
| JNI wrappers | `src/jni_env.rs` | Named JNI 1.6 slots, typed `JavaVm`/`JniEnv`, shared class-cast queries, descriptor parser, checked `Method`/`Arg` calls, primitive-array region copies and `Native` registration |
| Module kinds, registry | `src/module.rs`, `src/registry.rs` | Module kinds and lifecycle events; C-layout registration structs; `HostStr<'a>` for host-lent C strings |
| Native clock | `src/clock.rs` | Allocation-free OS monotonic nanoseconds shared by JNI frame time and input; no per-library epoch |
| Runtime policy | `src/runtime_policy.rs` | Memory modes, target FPS, remaining frame time |
| Thread placement | `src/thread_placement.rs` | Opt-in asymmetric topology plan and current-thread-only affinity; bounded cold snapshots, no TID ledger, hotplug, frequency changes or worker |
| Deploy scan, exports | `src/scan.rs`, `src/symbols.rs` | Deploy-dir layout and stems; C export names and callback types |
| Unit tests | `tests/unit/` | One file per module via `#[path]`, plus kit hash and Host API copy tests |
| Integration tests | `tests/*.rs` | `kit_identity.rs` kit hash over the real workspace; `unsafe_boundaries.rs` JNI/Host API boundary scan and per-crate `unsafe` budget |
