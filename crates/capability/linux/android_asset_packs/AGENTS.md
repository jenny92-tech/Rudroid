# Extracted Android asset packs

> Presence-selected platform policy enabling the loader's existing-file resource overlay.

## Role
Capability `platform_android_asset_packs`, kind platform; no capability links.

## Design
Three layers stay separate so directory placement is not misread as ownership:
- `capability/linux/android_asset_packs` (`Kind::Platform`): toggles the Linux host's
  already-extracted directory overlay via `api.enable_asset_packs()` only.
- `host::resources` + `common::android_assets::AssetResolver`: scan
  `game_files/assetpacks/<Pack>/assets` and publish the authoritative paths.
- `capability/android/assets` (`Kind::Runtime`): expose NDK `AAssetManager_*` to the
  guest against that same host authority.

Early init validates `HOST_API_VERSION` (currently 7) and enables one loader-owned
snapshot. Common owns safe path validation; this capability contains no unsafe code.

## Constraints
Only existing extracted directories are read. No extraction, links, downloads,
ownership results, game modifications or managed pointers are created here.
Duplicate registration fails. Publish only as part of a complete matching kit.

## Contents

| Name | Path | Responsibility |
|---|---|---|
| Manifest | `Cargo.toml` | cdylib/rlib capability with common only |
| Early registration | `src/lib.rs` | Validated once-only pack overlay enablement |
