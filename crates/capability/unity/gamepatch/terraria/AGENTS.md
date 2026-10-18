# Terraria Android Title compatibility

> `terraria_android` supplies the custom Activity contract used by Terraria's Android XNA bridge.

## Role
Package `rudroid-unity-gamepatch-terraria`, cdylib + rlib `libterraria_android.so`, kind `gamepatch`, exact package `com.and.games505.TerrariaPaid`; requires `android_base` and `unity_2021_3`.

## Design
- Register only the game's UnityPlayerActivity extensions: PressedStates `[Z`, MouseMode `I`, MouseInside `Z` and SetSurfaceFrameRate `(F)V`.
- The Main module event follows shared player preparation and initializes that actual currentActivity once. Its VM field owns a zeroed 330-key array; mouse defaults match the C reference (-1/true). Repeated events preserve existing input state.
- The frame-rate callback keeps the host compositor's fixed refresh policy, matching the reference facade; no display owner, polling or worker is created.
- The deployment sample `configs/terraria.toml` selects the existing Xbox Android input identity and HAT axes for InControl. The framework's default manufacturer avoids a Linux token that otherwise selects its desktop mapping. No game-specific axis rewrite or managed hook is needed.

## Constraints
- Keep business JNI code safe through common's typed wrappers; no raw JNI slots, Rust owner pointers, native hooks or game offsets.
- The shared VM owns the array graph. Do not retain global array/string caches or add polling for the reference's static keyboard snapshot.
- Custom fields stay in this Title; standard Activity/player identity belongs to player_lifecycle.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `terraria_android` dynamic capability |
| Native entries | `src/lib.rs` | Package gate, JNI/init entries and Main subscription |
| Java glue | `src/java.rs` | Safe fields, one-time VM-owned state and float callback |
| Real VM probe | `../../../../project/unity_loader/tests/native_terraria_activity.rs` | Production-kit Activity graph, state retention, reflection and exact float call |
