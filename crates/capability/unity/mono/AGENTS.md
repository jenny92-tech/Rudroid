# Unity Mono runtime

> Optional Runtime capability: prepared ARM64 Mono bridge/JIT, Host API named service, and scoped P/Invoke.

## Role
Capability `unity_mono`, `Kind::Runtime`, artifact `libunity_mono.so`. Select it only
with a trusted prepared `[unity.mono]` Host Config. The user contract is
[UNITY-MONO](../../../docs/UNITY-MONO.md). Package `rudroid-unity-mono`
(cdylib + rlib); depends on `rudroid-common`, `rudroid-unity-common`, and
`libloading`. Requires `android_elf_load` in the kit.

## Design
- `rudroid_plugin_init` refuses kits that also load `il2cpp_patch` and registers
  `rudroid.unity.mono.v1` (`MonoServiceV1`: validate / add_reserved_paths /
  start_bridge).
- `player_lifecycle` prepare calls `start_bridge` when the service is registered;
  the callback no-ops unless `[unity.mono]` is present. With config it resolves
  paths via `mono_config_from_api` and `rudroid.elf.guest_libdl.v1`, opens the
  bridge/JIT, binds the size-versioned setup ABI, and publishes a scoped
  `libil2cpp.so` alias before the Android player is mapped.
- Deploying `libunity_mono.so` without `[unity.mono]` must not strip IL2CPP
  guest-plan entries. Guest loader handles belong to ELF; Mono fallback handles
  stay in this capability.
- Desktop pacing registers exact scalar icalls and applies policy on
  `UnityFrame`; managed getters keep the game-requested value.
- Steam no-client stubs are attributed and limited; P/Invoke never opens
  arbitrary host paths.

## Constraints
- Ships no bridge, JIT, donor, or game files. Test real bridges only with local
  prepared inputs; keep regular IL2CPP startup tested separately.
- `UA_MONO64_RUNTIME` / `UA_MONO64_MANAGED` overrides are rejected.
- Native code, callback addresses, and strings remain process-pinned through
  guest teardown.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `unity_mono` |
| Crate root | `src/lib.rs` | Descriptor, init, alias registration, `start` entry |
| Named service | `src/service.rs` | `MonoServiceV1` validate / reserved paths / `start_bridge` |
| Frame pacing | `src/pacing.rs` | Desktop/observe/android UnityFrame policy |
| Steam stubs | `src/steam.rs` | Bounded no-client P/Invoke table |
| Unit tests | `tests/unit/` | Fake bridge / guest libdl doubles |
