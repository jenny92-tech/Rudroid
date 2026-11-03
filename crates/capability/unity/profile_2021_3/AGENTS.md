# Unity 2021.3 profile

> Unity 2020–2021.3 profile: shared player Spec, exact-image teardown repair and opt-in Swappy fallback.

## Role
Capability `unity_2021_3`, kind `unity_profile`; not in `crates/deploy/silksong.txt`. The loader calls `rudroid_plugin_register_jni` in the early JNI phase and `rudroid_plugin_init` in the native phase.

## Design
- `ENGINE_SPEC` mirrors C++ `plugins/unity_2021_3`: player `com/unity3d/player/UnityPlayer` with `()V`, initJni `(Landroid/content/Context;)V`, shared reflection/interface proxies, Context-only PAD init, explicit missing network delivery and actual Activity publication.
- Safe Java behavior comes from the plain `player_lifecycle::java` library rather than another profile. The original constructor and initJni ABI stay unchanged; Google AR returns false and the legacy launch URL is an empty string.
- Registration delegates to `player_lifecycle::register_jni`, which owns prepare/bootstrap/run, input, pacing and events.
- The native phase pins the Host API and subscribes to `ModuleKind::Unity`; -1 on null table, ABI mismatch or rejected subscription, and a repeated init returns 0.
- Swappy fallback (`unity.swappy_invalid_interval_fallback`, default false): resolves the hidden `SwappyGL_getSwapIntervalNS` via `so_symbol_all` and detours it so `UINT64_MAX` becomes `1e9 / device.displayRefreshRate` ns. A typed `Original` installer pairs the replacement with the original slot and publishes the trampoline after success.
- `teardown_order` repairs the observed native metadata-cache/allocator destruction order for one verified Unity 2020.2.2f1 image. Three exported addresses, GNU build-ID note, complete parent, both cleanup bodies and complete nativeDone/nativePause bodies must match. Unknown images are not patched; matching code swaps two existing no-argument BLs without skipping either cleanup or nativeDone. Installation is once at module load, independent of Swappy, with no title-name branch, allocation policy change, worker or frame hook. General terminal stop belongs to common, not this image gate. Evidence: `tests/fixtures/teardown-order.md` and `tests/fixtures/terminal-lifecycle.md`.
- That same verified image explicitly enables the framework's cached C-compatible host-presentation route through the shared VM, before guest startup, to select its working Android audio path. Other images retain the single-display default. Enabling this globally caused a measured audio-reset regression on Unity 2021.3.14f1c1; the early no-route control restored its output. This is a version quirk, not host multi-display discovery or an unconditional FMOD policy.

## Constraints
- Do not copy prepare/bootstrap/run here: shared behavior goes in `player_lifecycle`, version differences in `ENGINE_SPEC`.
- The descriptor id stays `ENGINE_SPEC.id`, equal to the artifact name. Never link another capability statically.
- The Swappy fallback stays opt-in, with at most one install attempt per process.
- `spec_matches_reference_literal` and the `PROFILES` row in `crates/project/unity_loader/tests/profile_lifecycle_fixture.rs` pin the Spec; change them together.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `unity_2021_3`; common and `player_lifecycle` |
| Profile | `src/lib.rs` | Descriptor, Spec, JNI delegation, native patch subscription and Swappy fallback |
| Teardown order | `src/teardown_order.rs` | Exact-image guards and checked two-call lifetime repair |
| Unit tests | `tests/unit/` | Swappy/Spec/registration plus unknown-image, guard rejection and exact cleanup-call targets |
| Fixtures | `tests/fixtures/` | Hidden Swappy symbol and teardown native ABI/device evidence; no original game binaries |
