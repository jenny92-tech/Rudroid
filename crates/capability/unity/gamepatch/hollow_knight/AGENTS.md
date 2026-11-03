# Hollow Knight viewport gamepatch

> Hollow Knight Title patch: full-screen camera rects and width-safe scaling for narrow screens through real IL2CPP detours.

## Role
Capability `hollow_knight_viewport`, kind `gamepatch`, no package gate, requires `il2cpp_patch` (hooks install on its `Il2cppReady` event). Ported from the C++ reference; presence in the deploy dir is the activation switch, so Silksong kits never contain it.

## Design
- `ViewportFeatures` (`debug`, `force_full_viewport`, `tk2d_width_safe`, `ui_orthographic_width_safe`): `game_patches.hollow_knight_viewport.<name>` wins over legacy `hk_viewport.<name>`, then the default; non-boolean values fall through.
- `rudroid_plugin_init` checks the ABI (-2), then subscribes to `Il2cpp` (bind exports through the shared `rudroid_common::il2cpp::Il2cpp`) and `Il2cppReady` (`install`); -3 on rejection.
- Hooks: `Camera.set_rect_Injected` forced to (0,0,1,1), `AutoScaleViewport` returns 1.0, `set_orthographicSize` and `tk2dCamera.UpdateCameraMatrix` scaled by `(16/9)/aspect` below 16:9. After the original AutoScaleViewport, SetOverscanViewport and GameCameras.SetOverscan, one shared owner fixup corrects tk2d/full rects and native-initialized UI cameras; the latter may never call the hooked managed setter.
- ForceCameraAspect and GameCameras references require metadata-resolved offsets and exact Camera/tk2d type identities. UI correction binds the checked instance-float getter and original setter with their own MethodInfo values. Sixteen scalar cache slots keep repeated post-calls idempotent; address tokens are never dereferenced from the cache, no managed objects are retained, and a full cache skips a new entry instead of compounding scale. No mutex spans a guest call. This fixes compatibility, not a measured frame-rate improvement.
- Screen getter entries stay paired with their immutable `MethodInfo` through common `StaticI32Method`; the width/height pair is copied before native calls, with no mutex held.
- tk2d keeps a per-camera base-value cache so passes stay idempotent. Logs use `HKVIEW`, capped per key. Hook installation and original publication use the shared typed `gamepatch_support::install_detour` installer; replacement and slot signatures must match at compile time.

## Constraints
- Both the Rust `plugin_init` and native init are unsafe: the same-kit Host API must keep callbacks and resolved native code mapped, deliver valid module events and provide a quiescent hook-installation window.
- Hook resolution validates staticness, parameter/return kinds and byref flags through common `method_signature`; Rect parameters and pointer fields require the exact metadata class identity from the expected image before raw layout access. Same-named foreign types are rejected; missing or mismatched metadata leaves the native entry intact.

- Managed-method hooks and original slots use `C-unwind`; guest exceptions may cross Rust frames. Temporary thread state must restore on unwind as well as return.
- Keep reference parity: no package gate; isolation comes from deployment.
- No hard-coded game offsets; tk2d and ForceCameraAspect layouts come only from `il2cpp_field_get_offset`; missing ForceCameraAspect fields skip secondary camera fixups.
- The new key keeps winning over legacy `hk_viewport.*`, which wins over the default.
- Detours come from `rudroid_common::arm64_detour`, only in the `Il2cppReady` window.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `hollow_knight_viewport`; common, host-config, libc |
| Patch | `src/lib.rs` | Descriptor, features, `plan()`, width-safe/tk2d helpers, hooks, init |
| Unit tests | `tests/unit/` | Capped log, ungated applicability, defaults, override chain |
| Integration tests | `tests/behavior_fixture.rs` | Production path, native-initialized HUD/repeated camera callbacks, wrong ABI and same-named foreign type rejection |
| Fixtures | `tests/fixtures/` | Fake `il2cpp_*` API and HK camera/tk2d model in C |
