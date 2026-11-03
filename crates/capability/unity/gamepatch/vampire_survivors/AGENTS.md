# Vampire Survivors gamepatch

> Opt-in fallback for one missing debug shader: when `Shader.Find("Hidden/Internal-Colored")` returns null, retry once with `Sprites/Default`.

## Role
Capability `vampire_survivors`, kind `gamepatch`, package gate `com.poncle.vampiresurvivors`, requires `unity_6,il2cpp_patch`. Optional, off by default and not deployed for Silksong.

## Design
- Init (ABI -2) arms only when the package matches and `game_patches.vampire_survivors.missing_debug_shader` is true. It then subscribes to `Il2cpp` and `Il2cppReady` (one install attempt); -3 on rejection.
- Install (aarch64 Linux only): bind IL2CPP exports through the shared `rudroid_common::il2cpp::Il2cpp`, require the pointer-width GC-handle ABI for the exact Unity build, verify the `Shader.Find` signature, then redirect it with `arm64_detour::install_quiescent`. The typed `Original::install_quiescent` checks the C-unwind replacement/slot types and publishes the trampoline in the same idle window.
- The `extern "C-unwind"` hook calls the original first. Only a null result for exactly `Hidden/Internal-Colored` retries with a GC-rooted `Sprites/Default`, freed by a guard even on unwind. Log tag `VS_GAMEPATCH`.

## Constraints
- Stay opt-in and Title-gated; never modify the APK, metadata or any successful lookup.
- Pair every GC handle with a free, never make guest pointers Rust owners, add no C code, and hold no Rust lock between the two lookups.
- Guest exceptions cross the hook only through `C-unwind`, never a plain C ABI.
- Builds outside common's exact-build table (currently 6000.0.61f1/ARM64) keep the original entry.
- Title adaptation only; it does not explain missing builtin shaders under Docker Mesa.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `vampire_survivors`; depends only on common |
| Patch | `src/lib.rs` | Descriptor, enable check, ABI checks, quiescent detour, fallback lookup |
| Unit tests | `tests/unit/` | Exact-name gating, failure preservation, unwind release, enable rules |
