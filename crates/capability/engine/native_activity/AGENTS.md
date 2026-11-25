# Native Activity engine profile

> Minimal `Kind::EngineProfile` proof that the host kernel can run a non-Unity guest through `ANativeActivity_onCreate`.

## Role
Capability `engine_native_activity`, kind `engine_profile`, requires `android_base,android_elf_load`. Package `rudroid-engine-native-activity`, artifact `engine_native_activity`. Zero Unity dependencies: only `rudroid-common`.

## Design
- Early JNI registration publishes a `RudroidEngineProfile` with `validate_config` / `build_guest_plan` / `prepare` / `bootstrap` / `run`.
- Config: `[native_activity] library` (required relative path) and optional `entrypoint` (default `ANativeActivity_onCreate`).
- Guest plan: optional `libc++_shared.so`, then the configured library as `ModuleKind::Main`.
- `run` fills a minimal NDK `ANativeActivity` POD (JavaVM when present; window/asset manager/env/clazz left null), calls the entrypoint, then invokes guest-registered `onDestroy` if set.
- Stubbed for this proof: `onStart`/`onResume`/`onNativeWindowCreated` drive, ANativeWindow/asset manager wiring, saved-state restore, and a frame loop.

## Constraints
- Never link `rudroid-unity-common` or `player_lifecycle`.
- Fail-closed on missing/unsafe library paths.
- Integration smoke must deploy a kit with no Unity/il2cpp/gamepatch members.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib; common only in production deps |
| Profile | `src/lib.rs` | Descriptor, EngineProfile callbacks, minimal ANativeActivity POD |
| Unit tests | `tests/unit/` | Path validation and default entrypoint |
| Smoke | `tests/native_activity_smoke.rs` | Zero-Unity kit bring-up + fixture observation |
| Fixture | `tests/fixtures/fake_native_activity.c` | Guest `ANativeActivity_onCreate` side effects |
