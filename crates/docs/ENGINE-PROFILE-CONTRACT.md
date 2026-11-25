# EngineProfile contract

How to add a second (or nth) engine that reuses Android/SDL/host without linking Unity.

Acceptance titles still ship Unity profiles. This contract is the host kernel seam
proven by `engine_native_activity`. Full NDK Activity ports remain out of scope
until separately accepted.

## Shape

| Piece | Rule |
|-------|------|
| Kind | `Kind::EngineProfile` (exactly one per kit) |
| Artifact | `lib<id>.so` via Deploy Selection; id == descriptor id |
| Deps | Prefer `rudroid-common` (+ `rudroid-host-config` only if needed). No `rudroid-unity-common` / `player_lifecycle` unless the engine is Unity |
| Register | Early JNI: fill `RudroidEngineProfile`, call `HostApiV0::register_profile` |
| Headers | English AGENTS; `INPUT`/`OUTPUT`/`POS` on sources; no `mod.rs` |

Host orchestration (discovery → phases → prepare → guest → bootstrap → run) lives
in `rudroid-host-core`. Project CLI `rudroid-unity-loader` is a thin re-export.

## Required callbacks

Registered through `RudroidEngineProfile` (`common` registry ABI). Host copies the struct.

| Callback | When | Contract |
|----------|------|----------|
| `validate_config` | After register-JNI, before prepare (**run mode only**; CLI `--check` does not invoke it) | Fail closed on missing/unsafe config. Use `HostApiV0` getters (`config_string`, …). Return `0` / negative |
| `build_guest_plan` | Guest plan construction | Append relative `.so` paths via `RudroidGuestPlanBuilder`. May run before prepare; prefer API for config/services |
| `prepare` | Before guest map | Allocate session (HandleTable). Subscribe to module events if needed. Null session = fail |
| `bootstrap` | After guest map | Receive `native_library_dir`. Non-zero = fail |
| `run` | After bootstrap | Drive the engine. `0` = full stop; `RUDROID_ENGINE_RUN_PROCESS_EXIT` = preferences saved, process must exit without teardown; negative = fail |

`android_base` must have registered `RudroidAndroidHooks` (`flush_preferences`,
`signal_vsync`) before prepare. Host flushes preferences after `run` returns.

## Guest plan builder

`add_entry(ctx, relative_path, role_id, required, after_role_id, flags)`:

| Argument | Meaning |
|----------|---------|
| `relative_path` | Guest-relative path under game files (no `.` / `..` escapes) |
| `role_id` | `ModuleKind as u32`; `0` = no module-loaded event |
| `required` | `1` fail if missing; `0` skip if absent |
| `after_role_id` | KindLoaded gate; `0` = none |
| `flags` | `RUDROID_GUEST_ENTRY_FMOD_JNI_OK`, `RUDROID_GUEST_ENTRY_AFTER_FMOD` (Unity/FMOD patterns) |

Optional helpers: `add_reserved_path`, `set_defer_jni_on_load` (deduping multi-role
list — call once per `ModuleKind` to defer). Builder `add_entry` rejects absolute
paths and `.` / `..` segments. Unity ARM64 order lives in
`rudroid-unity-common::guest_entries`; non-Unity engines own their list.

## HostApi named services and file overlays

`HOST_API_VERSION` is **7**. Grow the table only at the tail with a version bump;
same-kit rebuild required.

| Slot / method | Behavior |
|---------------|----------|
| `register_service` / `register_named_service` | First-wins; name + pointer process-pinned; ABI version stored |
| `query_service` / `query_named_service` | Null when missing or ABI mismatch |
| `register_service_typed` / `query_service_typed` | `ServiceContract` helpers: one audited cast in common; business code stays safe |
| `rudroid.host.config_toml.v1` / `config_toml` | Host-registered read-only `HostConfigTomlV1` service; gives schema-level TOML parsing without duplicating config tables or overloading `config_array_size` |
| `config_array_size` / `config_len` | Pure TOML array length (`0` when absent or non-array, including tables) |
| `config_set_string` / `set_config_string` | First-write-wins runtime override scoped to `package.mainIntentBundle.<key>` (returns `1` on acceptance, `0` outside that scope or on repeat writes); getters prefer overrides then HostConfig |
| `register_file_overlay` | First-wins exact canonical path → replacement path (host copies strings) |

Example consumers: `unity_mono` publishes `rudroid.unity.mono.v1` (`MonoServiceV1`
with validate / reserved-paths / start_bridge); `android_elf_load` publishes
`rudroid.elf.guest_libdl.v1`. Registration alone is not activation — Unity Mono
strips `libil2cpp` / starts the bridge only when `[unity.mono]` is present.
Unity `BootView` registers a sealed boot.config through `register_file_overlay`
and may publish render-thread argv via
`set_config_string("package.mainIntentBundle.unity", ...)`. New engines should
prefer named services over new Host API fields.

## Zero-Unity example

`capability/engine/native_activity/` → id `engine_native_activity`.

- Config: `[native_activity] library` (required relative path), optional `entrypoint`
- Plan: optional `libc++_shared.so`, then configured library as `ModuleKind::Main`
- `run`: minimal `ANativeActivity` POD + `ANativeActivity_onCreate`; window lifecycle stubbed
- Smoke: kit with probe/`android_base`, hooks fixture, `android_elf_load`, profile — **no** `unity_*` / `il2cpp_patch` / gamepatch

## Reused vs engine-owned

| Reused (do not fork) | Engine owns |
|----------------------|-------------|
| `android_*` (base, elf_load, jni_vm, assets, …) | Profile `validate` / plan / prepare / bootstrap / run |
| `platform_sdl_runtime` / other linux hosts | Guest `.so` order and Main role |
| `rudroid-host-core` bring-up + Host API v7 | Engine-specific config keys and session |
| `rudroid-common` descriptors, handles, JNI tools | NDK/engine POD and entrypoint ABI |

## Related

- Capability table: [MIGRATION-MAP](MIGRATION-MAP.md)
- Config ownership: [RUNTIME-POLICY](RUNTIME-POLICY.md)
- Host kernel: [host/AGENTS](../host/AGENTS.md)
- Unity helpers (Unity engines only): [unity/common/AGENTS](../capability/unity/common/AGENTS.md)
