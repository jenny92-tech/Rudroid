# LevelPlay Android Addon

> Offline IronSource LevelPlay and AdQuality bridges whose initialization always fails, asynchronously, on the main Looper.

## Role
Capability `sdk_levelplay`, kind `android_addon`, requires `android_base`; package `rudroid-android-addon-levelplay` (cdylib + rlib). Optional: not in the Silksong profile; the loader's `real_game_startup` test kit deploys it.

## Design
- `rudroid_plugin_register_jni` (the only export) needs the shared VM (else -3) and registers natives on `LevelPlayBridge`, its job classes, `IUnityLevelPlayInitListener` and AdQuality. Null table -1, ABI mismatch -2, failed registration or pending exception -4.
- `getInstance` returns one global bridge; `initialize` posts an `org/rudroid/levelplay/Initialization` job that calls `onInitFailed` with error code -1.
- AdQuality `initialize` snapshots the stored listener into a `QualityInitialization` job that calls `adQualitySdkInitFailed(-1, ...)`; enum classes get `fromInt`/`getValue`.

## Constraints
- Never report ad or AdQuality initialization success; deliver the failure only from the queued main-Looper job.
- Listener, job and bridge objects live in the shared VM; no Rust pointers in JNI fields; delete every temporary local reference.
- The capability stays mapped while its registered JNI callbacks exist.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `sdk_levelplay`; depends on `rudroid-common` |
| LevelPlay bridge | `src/lib.rs` | Descriptor, register-JNI entry, bridge natives, `Initialization` failure job |
| AdQuality bridge | `src/quality.rs` | AdQuality natives, listener snapshot, enum helpers, failure job |
