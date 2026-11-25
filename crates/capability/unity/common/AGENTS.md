# Unity common

> Plain shared library of Unity/IL2CPP/Mono helpers: binding, managed hooks, ABI gates, startup and Mono config views.

## Role
Package `rudroid-unity-common`, a plain library with no descriptor, never deployed.
Unity profiles, IL2CPP hooks, memory policy, Title gamepatches and
`player_lifecycle` each statically link their own copy. `rudroid-host-core` must
not depend on this crate in production. Non-Unity engines must not depend on it.
Runtime authority stays in host-core behind Host API v7; this crate holds no
cross-cdylib static state.

## Design
- IL2CPP binding, exact typed methods, rooted construction/invocation and GC-handle ABI evidence live here. Managed hooks, HUD and touch helpers enforce owner-thread/stop checks with the private AAPCS64 gateway.
- `player_jni` owns Unity `nativeInjectEvent` dispatch (`PlayerInputDispatch` /
  `PlayerInput` / `dispatch_player_input_event`) and `finish_player` (`nativeDone`
  drain). Spec flags `RUDROID_UNITY_PROFILE_*` live at the crate root.
- Typed gamepatch detour install helpers and scene object queries reuse the same checked IL2CPP tools. Mono ABI constants and prepared-package Mono config resolution stay beside Unity startup policy projection.
- Host Config schema is unchanged: Mono and Unity startup views are free functions over `rudroid_host_config::HostConfig` (and Host API getters for prepare). Capabilities never link each other; shared process state goes through `HostApiV0`.
- `BootView` owns a sealed projected boot.config and exposes source/replacement paths for Host API `register_file_overlay`.

## Constraints
- No descriptor, no `cdylib`. Runtime deps: `rudroid-common`, `rudroid-host-config`, `libc`, `serde_json`, `toml`; build-only `native-build` compiles `managed_gateway.S`.
- Add a Unity build to `il2cpp_abi::supported_gc_handle_abi` only with native ABI evidence; unlisted builds skip the calls.
- Business code uses common's safe tools plus this crate's checked IL2CPP helpers; no cross-capability static becomes shared authority.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library `rudroid-unity-common` |
| Managed gateway | `build.rs`, `src/managed_gateway.S` | Private AAPCS64 slot dispatch archive |
| Crate root | `src/lib.rs` | Module list, Mono/startup/boot re-exports, `RUDROID_UNITY_PROFILE_*` |
| Player JNI | `src/player_jni.rs` | `PlayerInputDispatch` / `PlayerInput` / `dispatch_player_input_event` / `finish_player` |
| IL2CPP | `src/il2cpp.rs`, `src/il2cpp_managed.rs` | Export binding, typed methods, rooted calls |
| IL2CPP ABI | `src/il2cpp_abi.rs` | Exact version GC-handle evidence |
| Managed hooks | `src/managed_hooks.rs` | Config patch plans and scalar/reference detours |
| HUD / touch | `src/hud_layout.rs`, `src/touch_controls.rs` | Rooted layout and TouchManager helpers |
| Scene objects | `src/unity_objects.rs` | Checked Behaviour/Sprite and GameObject helpers |
| Gamepatch support | `src/gamepatch_support.rs` | Typed detour installer and install events |
| Mono ABI / config | `src/mono_abi.rs`, `src/mono.rs` | Bridge constants, `MonoServiceV1`, HostConfig/`HostApi` Mono path checks |
| Unity startup | `src/unity_startup.rs` | Render-thread policy and Host API / HostConfig projection |
| Boot view | `src/boot_view.rs` | Sealed boot.config owner for FILE_OVERLAY registration |
| Module roles | `src/module_roles.rs` | Stable u32 aliases for guest mapping and engine lifecycle |
| Guest entries | `src/guest_entries.rs` | Shared Unity ARM64 guest-plan `.so` order |
| Unit tests | `tests/unit/` | One file per module via `#[path]` |
| Fixtures | `tests/fixtures/` | GC-handle ABI and render-thread identity evidence |
