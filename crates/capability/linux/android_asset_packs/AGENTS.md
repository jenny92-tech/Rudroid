# Extracted Android asset packs

> Presence-selected platform policy enabling the loader's existing-file resource overlay.

## Role
Capability `platform_android_asset_packs`, kind platform; no capability links.

## Design
Early init validates Host API version 4 and enables one loader-owned snapshot
of `assetpacks/<Pack>/assets`. Java, NDK and Bionic query that same service.
Common owns safe path validation; this capability contains no unsafe code.

## Constraints
Only existing extracted directories are read. No extraction, links, downloads,
ownership results, game modifications or managed pointers are created here.
Duplicate registration fails. Publish only as part of a complete matching kit.

## Contents

| Name | Path | Responsibility |
|---|---|---|
| Manifest | `Cargo.toml` | cdylib/rlib capability with common only |
| Early registration | `src/lib.rs` | Validated once-only pack overlay enablement |
