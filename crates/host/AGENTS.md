# Host core

> Engine-agnostic host kernel: kit discovery, Host API table, guest bring-up and engine profile runner.

## Role
Package `rudroid-host-core`, a plain `rlib` with no descriptor and no binary. Project entries (`rudroid-unity-loader` / `rudroid-loader`) and their integration tests link it and re-export the orchestration surface. Capabilities never link this crate.

## Design
- Discovery validates ABI/build/id/kind/test markers and orders `android_base` before dependents.
- Config and resource views publish under one immutable process-pinned owner.
  Host auto-registers read-only named service `rudroid.host.config_toml.v1`
  (`HostConfigTomlV1`) and re-inserts it in `clear_service_and_overlay_registries`.
  `config_set_string` accepts only first-write-wins `package.mainIntentBundle.<key>`;
  `config_array_size` is pure array length. File overlays skip `canonicalize`
  until one is registered.
- Guest plan construction is engine-agnostic: registered `RudroidEngineProfile::build_guest_plan` fills entries, reserved paths and multi-role `defer_jni_on_load`. Unity Mono startup uses `rudroid.unity.mono.v1` (published by `unity_mono`); that path queries the ELF capability’s process-pinned `rudroid.elf.guest_libdl.v1` table.
- Host API table is v7 (named services + file overlays) with typed `ServiceContract` helpers in common. Engine prepare → guest map → bootstrap → run follows the registered profile; see [ENGINE-PROFILE-CONTRACT](../docs/ENGINE-PROFILE-CONTRACT.md).
- `host_platform` selects the adapter; SDL owns the implemented native session.

## Constraints
- Plain library only — never a `cdylib`, never deployed as a capability.
- No production dependency on `rudroid-unity-common`. New engine behavior goes through `RudroidEngineProfile` / named services.
- `TEST_BUILD_MARKERS` must stay identical to `scripts/kit_markers.py` (`test_deploy_select.py` reads `host/src/lib.rs`).
- Unsafe stays at C-ABI boundaries with `// SAFETY:`; budgets live in `common/tests/unsafe_boundaries.rs`. Callers that authorize a trusted deploy use `unsafe { load_deploy_dir(...) }` directly — there is no safe `*_trusted` wash wrapper.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain `rudroid-host-core` |
| Kernel | `src/lib.rs` | Discovery, `LoadedCapability`, `bring_up`, `terminate_completed_guest` |
| Host API | `src/host_api.rs` | Process table, config/log/resource publication |
| Resources | `src/resources.rs`, `src/cpuinfo_view.rs` | Asset/cpuinfo projections |
| Phases / select / validate | `src/phases.rs`, `src/select.rs`, `src/validate.rs` | Phase order, allowlist, topology diagnostics |
| Registry / profile | `src/runtime.rs`, `src/runtime_catalog.rs`, `src/runtime_exec_table.rs`, `src/engine_profile.rs` | Shared registry and engine lifecycle |
| Guest | `src/guest.rs`, `src/guest_plan.rs`, `src/guest_elf_api.rs`, `src/guest_load_bridge.rs` | Plan, ELF API, post-Main bridge, bring-up |
| Platform | `src/host_platform.rs`, `src/platform_sdl.rs` | Backend session and SDL adapter |
| Unit tests | `tests/unit/` | Declared from sources with `#[path]` |
