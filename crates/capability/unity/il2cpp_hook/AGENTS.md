# IL2CPP hook mechanism

> Config-driven IL2CPP hook mechanism: reads both hook config dialects, installs ARM64 inline detours and fires the one-shot `Il2cppReady` event.

## Role
Capability `il2cpp_patch`, kind `runtime`, no `requires`; deployed for Silksong. `unity_memory_policy`, `silksong_unity6`, `hollow_knight_viewport` and `vampire_survivors` require it because only it fires `Il2cppReady`.

## Design
- Legacy `[[il2cpp_patch]]` entries (dotted Host API keys, C++ defaults and filter, at most 8) and named `[il2cpp_hooks]` (HostConfig object only) both load. Legacy requests persist until successful init, then common resolves exact assemblies, unique overloads and complete signatures before installing scale/freeze/force_return/trace/bypass. Unsupported shapes/fields/builds produce rejection diagnostics.
- `HookRegistry::apply_detour` installs through `rudroid_common::arm64_detour`: a direct `B` within ±128 MiB, else a nearby trampoline veneer; `Detour::original` calls the relocated entry.
- `rudroid_plugin_init` subscribes to `ModuleKind::Il2cpp` and detours `il2cpp_init`. The first nonzero `int` success status fires `Il2cppReady`; zero stays silent so the guest can retry.
- Legacy `ui_layout.*` uses common's rooted HUD adapter: hide at most sixteen Component fields, copy pivot and move world position/right margin after the original method. Partial movement and overflowing numeric configuration are rejected. A package-gated Silksong TouchManager setter adapter preserves valid arrays and safely handles null arrays/elements; no array or object offsets are guessed.
- `UnityStopping` cancels extra managed work; metadata/code/trampolines remain process-pinned.
- Init returns -1 null table, -2 ABI mismatch, -3 rejected subscription; a repeated init returns 0.

## Constraints
- Mechanism only: no hard-coded Title offsets; callers supply resolved addresses.
- Targets are 4-byte-aligned entries in owned pages that no thread executes during install. Pages never stay W+X or lose execute.
- No runtime restore on the production path; `restore_detour` is for pre-guest machinery and tests.
- The detour and original use `C-unwind` for guest exceptions; registry callbacks must not unwind. The detour body must not panic, touches only pinned statics and holds no lock across the original call or broadcast. `Il2cppReady` fires at most once; failures never fail the module load.
- Tests must not link another capability rlib (descriptor export collision); the detour fixture dlopens the `android_elf_load` cdylib.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `il2cpp_patch`; non-harness `detour_fixture` test |
| Mechanism | `src/lib.rs` | Descriptor, legacy parsing, `HookRegistry`/`Detour`, `Il2cppReady` producer |
| Configured runtime | `src/configured.rs` | Retains requests, binds initialized module, installs common plans and handles stop events |
| HUD config | `src/layout_config.rs` | Safe legacy key/default parsing and owned strings/field selectors |
| Unit tests | `tests/unit/` | Dialects, filter and cap, real detour, producer once/retry/re-entry |
| Integration tests | `tests/detour_fixture.rs` | aarch64 Linux crash-isolated detour shapes, including elf_load-mapped code |
| Fixtures | `tests/fixtures/` | C prologue targets; standalone `patch_probe/` crate used by unity_loader tests |
