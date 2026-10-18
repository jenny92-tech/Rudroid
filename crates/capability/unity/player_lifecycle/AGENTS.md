# Unity player lifecycle

> Shared library for Unity engine profiles: the `Spec`/`Profile` contract, validated early JNI registration and the prepare/bootstrap/run player lifecycle.

## Role
Plain library, no descriptor, not deployed on its own. Each profile cdylib (`profile_2021_3`, `profile_6`) links its own copy, so statics belong to that profile; cross-library interaction goes through the Host API and the shared JNI VM.

## Design
- `register_jni::<P>()` validates `P::SPEC` and registers a `UnityProfile` with `prepare`/`bootstrap`/`run`. Returns -1 null table, -2 ABI mismatch, -3 invalid Spec, -4 no shared VM, -5 registry refusal.
- `Spec` holds the C++ `unity_engine::Spec` fields plus extra callbacks, `currentActivity` publication and Java services. `CallbackBody` fixes each native body together with its JNI signature. All profiles share terminal nativeDone ownership through common; there is no per-version terminal-pause selector or extra suspend before destruction.
- `java.rs` shares reflection/interface proxy, player queries and installed-pack services between both profiles; the 2021 profile selects its Context-only PAD init, unavailable-delivery reply and empty launch URL. The immutable Host API is copied during prepare for later resource callbacks. Both profiles publish their actual Activity through the shared VM; no capability links another profile.
- Unity 6 also creates a VM-owned disabled UnityAccessibilityDelegate with idempotent cleanup, matching the observed init/cleanup native ABI. It owns no host view, worker or cached global/player reference. The production Java probe verifies type, global/local lifetime, repeated cleanup, failed null initialization and last-owner reclamation; this does not implement a screen reader.
- Reflection constructor lookup uses a scoped JNI string view; the shared helper releases it on every exit without allocating a temporary descriptor copy.
- The legacy profile explicitly reports unavailable orientation sensors and China Player launch-ad Activity queries, including a matching stop callback; these queries create no sensor/advertising workers and do not change the fixed host display.
- `prepare` registers callbacks, allocates player/UnityPlayerActivity/Surface global refs in the shared VM, publishes the same player through Activity.mUnityPlayer, reads `unity.*` config and captures the main module base from module events. Game-specific Activity extensions stay in Title capabilities.
- The legacy profile's soft keyboard keeps bounded UTF-16 text/limits on the VM-owned player, consumes keys only while active and uses the registered native text/selection/visibility/close callbacks. Confirm/cancel configuration matches the reference, X/Y can type names, and generation checks preserve a keyboard reopened by a callback. Stop clears text before player teardown. Idle frames perform no keyboard JNI work and no worker/cache of text is created.
- `bootstrap` calls the libmain-registered `NativeLoader.load`. `run` calls initJni, starts the player and renders with input pumping and pacing; each successful `nativeRender` fires `UnityFrame` on the player thread (including the first render; false/exceptional returns fire none). It fires `UnityMaintenance` (at most 1/s) and `UnityStopping` before `finish_player`.
- The optional `unity.fast_exit` process policy applies only after an engine-requested terminal render and a checked synchronous preference flush. It returns the distinct process-exit result with native/service state pinned; the CLI immediately uses its existing `_exit` boundary. Failed writes, errors, input shortcuts and smoke limits retain full teardown. Only the validated Silksong launcher enables it; no managed hook terminates the process.
- False returns are retried only before the first successful render. A later false is an engine-requested terminal stop and goes through nativeDone, service joins and preference flush; pending exceptions still fail. Do not turn in-game quit into a paced 20,000-iteration retry or an error.
- Optional `rudroid_spy_*` exports from the loader's C spy fixture echo events into the host log; real games lack them.

## Constraints
- `Spec` has no `Default`, and `register_jni` must call `Spec::validate`. No per-version branches in the lifecycle: use a Spec field or the profile's native phase.
- Never register a callback name with a body whose signature differs.
- A new profile needs a `PROFILES` row in `crates/project/unity_loader/tests/profile_lifecycle_fixture.rs` (enforced by a test).
- `UnityStopping` fires before `finish_player`; never turn async cleanup waits into render busy-waits.
- `spy.rs` mirrors `crates/project/unity_loader/tests/fixtures/rudroid53_spy.c`; change both together. Host API version is checked against the current shared constant.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Plain library; depends only on `rudroid-common` |
| Spec and registration | `src/lib.rs` | `Spec`, `CallbackBody`, `Profile`, `register_jni`, render-sleep clamp |
| Lifecycle | `src/lifecycle.rs` | Prepare/bootstrap/run, render loop, stop and teardown, spy echo |
| Java services | `src/java.rs`, `src/network_proxy.rs`, `src/network_connectivity.rs` | Shared safe reflection/proxies, profile-specific PAD signatures and Linux host queries |
| Soft keyboard | `src/soft_input.rs` | Safe bounded VM-owned legacy text, active key routing, confirm/cancel and reentrant cleanup |
| Spy surface | `src/spy.rs` | Spy export names, event kinds, `RudroidSpyEvent` layout |
| Unit tests | `tests/unit/` | Spec validation, callback signatures, clamp, spy decoding, API pin |
