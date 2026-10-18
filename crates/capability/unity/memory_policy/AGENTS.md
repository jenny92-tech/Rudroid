# Unity memory policy

> Optional collector that lets the game handle low memory first, rechecks pressure, then calls Unity's own `UnloadUnusedAssets` on the player thread.

## Role
Capability `unity_memory_policy`, kind `runtime`, requires `il2cpp_patch`; optional and not in `crates/deploy/silksong.txt`. Configured by `[unity_memory]`; when the library is absent, nothing samples memory.

## Design
- Init (Linux only, `unity_memory.enabled` default true) validates config (-3) and subscribes to `Il2cpp`, `Il2cppReady`, `UnityMaintenance` and `UnityStopping` (-5 on rejection). No background thread.
- The first thread delivering `UnityMaintenance` owns the `Controller` and ticks it per event; `UnityStopping` records cancellation even during a reentrant guest call; the outer callback releases the controller/root on its owner thread before returning. A Drop guard restores the recursion flag on every exit.
- `engine.rs`: metadata-validated `Application.CallLowMemory` (zero-arg, or `ApplicationMemoryUsage` on verified builds) and `Resources.UnloadUnusedAssets`, rooted by one GC handle and polled per tick.
- `policy.rs`: `ReclaimPolicy` goes Idle → Notifying → Grace → Unloading; recovery during grace skips the unload, and completion starts the cooldown.
- `pressure.rs`: headroom is min(MemAvailable, cgroup v1/v2 RAM, v1 memsw); swap and PSI are reported separately. `PressureGate` adds hysteresis, warmup and cooldown.
- Explicit `unity_memory.diagnostic_only` forces notification/unload/GC off and samples only read-only residency plus public IL2CPP heap counters on the attached owner thread. Its 1–60s interval and 1–120 sample budget are validated; the controller disables itself after the budget, performs no pressure sampling in this branch and adds no worker. Missing/negative heap values remain unavailable. This is a bounded developer diagnostic, never a production performance default.
- Extra `UnloadUnusedAssets` and managed GC default off; explicit title configuration can enable them. Pre-warmup ticks skip sampling. Idle/cooldown samples retain all RAM/memsw hysteresis inputs but defer swap/PSI diagnostic I/O until a real pressure action; never skip recovery observations during cooldown.

## Constraints
- Validate static-ness, argument count, return type and byref flags before invoking; adapt through metadata, never offsets. Degrade explicitly on non-Linux or missing exports (no Mono).
- Never reclaim when memory is ample. Failed samples are skipped, never treated as zero headroom; never add `SwapFree` to RAM.
- Read procfs and cgroup files only; never change swappiness, swap, zram or cache settings.
- Hold at most one GC handle, released on completion, stop or drop. The handle ABI comes only from common's exact-build table (6000.0.61f1/ARM64); an unverified ABI never unloads.
- The host must let all in-flight player callbacks return before nativeDone; managed calls recheck stop after invocation and never chain rooting/unboxing/access after a stop request.
- Only the owning player thread uses the controller; never busy-wait on `AsyncOperation`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `unity_memory_policy`; depends only on common |
| Controller | `src/lib.rs` | Descriptor, config, event subscription, owner-thread `Controller` |
| Engine | `src/engine.rs` | Metadata checks over common `il2cpp::Il2cpp`, handle ABI gate, `GcRoot`-held operation |
| Policy | `src/policy.rs` | `ReclaimPolicy` notify, grace, recheck, unload, cooldown |
| Pressure | `src/pressure.rs` | Read-only meminfo/cgroup/PSI sampling and `PressureGate` |
| Unit tests | `tests/unit/` | Fake IL2CPP engine, policy paths, cgroup and gate sampling |
