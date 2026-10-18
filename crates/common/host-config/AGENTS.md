# Host Config

> Reader for the existing Host Config TOML dialect with the C++ reference's dotted-key getter semantics and memory-policy resolution.

## Role
Package `rudroid-host-config`, a plain library (no descriptor, never deployed) depending on `rudroid-common` and `toml`. The loader parses the device config with it and serves it through the Host API config getters; `capability/linux/sdl_runtime`, `capability/unity/il2cpp_hook` and the `silksong`/`hollow_knight` gamepatches link it for typed views.

## Design

- `HostConfig` preserves the TOML table and unknown keys. Dotted getters include
  numeric array indexes and toml++ conversions; set_string changes existing leaves only.
- Typed views resolve paths/package/device/plugins, bounded stream/audio memory
  defaults and independent exact/percentage render dimensions. Explicit malformed
  policy values reject; measured drawable geometry resolves before guest startup.
- Render-thread policy defaults to engine. Explicit single/multi/memory_mode
  projects only a bounded, unambiguous three-key boot rendering contract and
  requires the exact native-player allowlist in unity_startup.rs. The native ABI
  [fixture](tests/fixtures/unity-render-thread-build.md) owns the evidence;
  nearby versions or matching key names cannot extend it. FPS/jobs/GC stay separate.
- Optional wayland_native needs explicit guest dimensions and linear filtering;
  internal stays default. Never silently change resolution or discard sharp mode.
- `gpu.graphicsApi` defaults to `gles`; explicit `vulkan` is separate from render
  geometry and requires a Vulkan-enabled kit. Invalid API values fail validation.
- Resolution policy is shared across APIs. `gpu.renderScaleSharp` retains the
  GLES filter; Vulkan's separate `gpu.vulkanRenderFilter` accepts only `linear`
  (also its default), so switching APIs does not overwrite the GLES setting.

## Constraints
- Stay compatible with the existing schema: no rewrites, no new required keys, unknown sections tolerated; keep the Reply Catalog format out of this crate.
- Getter semantics match the C++ reference and toml++ 3.4; change them only with matching reference evidence and tests.
- Malformed memory-policy values are errors, never fallbacks; the loader resolves the policy once before any capability runs.
- Unit tests read `configs/unity.toml` and `work/config.silksong.device.toml` from the repository root.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library; depends on `rudroid-common` and `toml` |
| Reader | `src/lib.rs` | `HostConfig`, typed views, dotted-key getters, `set_string`, `memory_policy` |
| Unity startup policy | `src/unity_startup.rs` | Pure optional render-thread/argument/boot projection, strict three-key validation and 64KiB bound |
| Unit tests | `tests/unit/lib.rs` | Sample configs, conversion matrix, dotted keys, arrays, memory modes |
| Startup policy tests | `tests/unit/unity_startup.rs` | Explicit mode mapping, preserved/conflicting arguments, exact native identity and bounded three-key boot projection |
| Native startup evidence | `tests/fixtures/unity-render-thread-build.md` | Real ARM64 player identity and rejected file-length regression provenance |

