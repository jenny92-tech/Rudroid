# XLua Android Addon

> Watches the mapping of `lib/arm64-v8a/libxlua.so` after the Unity module loads.

## Role
Capability `sdk_xlua`, kind `android_addon`, requires `android_base,android_elf_load`; package `rudroid-android-addon-xlua` (cdylib + rlib). Optional; Silksong does not deploy it.

## Design
- Thin crate: descriptor, one `GuestModule` static `MODULE` (tag `XLUA`, kind `Xlua`, label `xlua`) and `rudroid_plugin_init`, which forwards to `after_unity_module::plugin_init_raw`.
- Re-exports `last_outcome`, `reset_state_for_tests` and `MappingOutcome`; all behavior lives in [`after_unity_module`](../after_unity_module/).

## Constraints
- Keep the logic in `after_unity_module`; this crate only states its data.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `sdk_xlua`; depends on `after_unity_module`, `rudroid-common` |
| Addon | `src/lib.rs` | Descriptor, `MODULE`, `rudroid_plugin_init` |
| Unit tests | `tests/unit/` | Pins `MODULE` to the reference values |
| Fixtures | `tests/fixtures/` | Synthetic `libxlua.so` counting `JNI_OnLoad` calls; built by the loader's `addon_module_events` test |
