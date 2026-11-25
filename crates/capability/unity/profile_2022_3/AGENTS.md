# Unity 2022.3 profile

> Context-only Unity 2022 profile for the Soul Knight 8.7.0 JNI surface; candidate pending a real launch.

## Role

Capability `unity_2022_3`, kind `engine_profile`. Package `rudroid-unity-profile-2022-3`, artifact `unity_2022_3`. The loader calls `rudroid_plugin_register_jni` in the early JNI phase; this profile exports no native-phase `rudroid_plugin_init`.

## Design

- The observed DEX shape uses `com/unity3d/player/UnityPlayer`, constructor `(Landroid/content/Context;)V`, initJni `(Landroid/content/Context;)V`, no extra player callbacks, and Activity publication. `ENGINE_SPEC` pins those values.
- `register_java_services` reuses the shared Context-only service set in `player_lifecycle::java::register_2021`; it is not an inherited engine patch.
- Registration delegates to `player_lifecycle::register_jni::<Unity2022>`. Descriptor id comes from `ENGINE_SPEC.id`.
- This profile intentionally has no offset, TLS, GC, Swappy, teardown-order or other native repair. Construction and Java service compatibility remain unverified until the game is launched on the target.

## Constraints

- Do not copy prepare/bootstrap/run here; shared behavior belongs in `player_lifecycle`.
- Keep crate-level `#![deny(unsafe_code)]`; the sole standard plugin export uses the repository's documented local `allow` and contains no unsafe operation.
- `spec_matches_observed_unity_2022_literal` and the `PROFILES` row in `crates/project/unity_loader/tests/profile_lifecycle_fixture.rs` pin the Spec; change them together.
- The profile name is a bounded interface candidate, not a claim that every Unity 2022.3 title is supported.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `unity_2022_3`; common and `player_lifecycle` |
| Profile | `src/lib.rs` | Descriptor, Spec and early JNI delegation |
| Unit tests | `tests/unit/` | Spec validation and empty-API rejection |
