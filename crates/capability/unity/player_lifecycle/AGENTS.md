# Unity player lifecycle

> Shared library for Unity engine profiles: the `Spec`/`Profile` contract, validated early JNI registration and the prepare/bootstrap/run player lifecycle.

## Role
Plain library, no descriptor, not deployed on its own. Each profile cdylib (`profile_2021_3`, `profile_2022_3`, `profile_6`) links its own copy, so statics belong to that profile; cross-library interaction goes through the Host API and the shared JNI VM.

## Design
- `register_jni::<P>()` validates `P::SPEC` and registers a `RudroidEngineProfile` with `validate_config`/`build_guest_plan`/`prepare`/`bootstrap`/`run`. Returns -1 null table, -2 ABI mismatch, -3 invalid Spec, -4 no shared VM, -5 registry refusal.
- `Spec` holds the C++ `unity_engine::Spec` fields plus extra callbacks, `currentActivity` publication and Java services. `CallbackBody` fixes each native body together with its JNI signature. All profiles share terminal nativeDone ownership through common; there is no per-version terminal-pause selector or extra suspend before destruction.
- `java.rs` shares reflection/interface proxy, player queries and installed-pack services across profiles: `register_2021` is the Context-only set for 2021/2022, `register_6` for Unity 6. The immutable Host API is copied during prepare for later resource callbacks. Every profile publishes its actual Activity through the shared VM; no capability links another profile.
- Unity 6 also creates a VM-owned disabled UnityAccessibilityDelegate with idempotent cleanup, matching the observed init/cleanup native ABI. It owns no host view, worker or cached global/player reference. The production Java probe verifies type, global/local lifetime, repeated cleanup, failed null initialization and last-owner reclamation; this does not implement a screen reader.
- Reflection constructor lookup uses a scoped JNI string view; the shared helper releases it on every exit without allocating a temporary descriptor copy.
- The legacy profile explicitly reports unavailable orientation sensors and China Player launch-ad Activity queries, including a matching stop callback; these queries create no sensor/advertising workers and do not change the fixed host display.
- `prepare` registers callbacks, allocates player/UnityPlayerActivity/Surface global refs in the shared VM, publishes the same player through Activity.mUnityPlayer, reads `unity.*` config and captures the main module base from module events. For non-engine render-thread policy it builds a `BootView`, registers it through Host API `register_file_overlay`, and keeps the owner in the session. After boot overlay setup it invokes `rudroid.unity.mono.v1` `start_bridge` when that service is registered (no-op without `[unity.mono]`). Game-specific Activity extensions stay in Title capabilities.
- The legacy profile's soft keyboard keeps bounded UTF-16 text/limits on the VM-owned player, consumes keys only while active and uses the registered native text/selection/visibility/close callbacks. Its passthrough shares the main pump's typed `nativeInjectEvent` dispatch, including Unity 2022's `(InputEvent,I)Z` with mode 0; neither path guesses an unsupported shape or clears an unrelated exception. Confirm/cancel configuration matches the reference, X/Y can type names, and generation checks preserve a keyboard reopened by a callback. Stop clears text before player teardown. Idle frames perform no keyboard JNI work and no worker/cache of text is created.
- `bootstrap` calls the libmain-registered `NativeLoader.load`. `run` calls initJni, starts the player and renders with input pumping and pacing; each successful `nativeRender` fires `UnityFrame` on the player thread (including the first render; false/exceptional returns fire none). It fires `UnityMaintenance` (at most 1/s) and `UnityStopping` before `finish_player`.
- The optional `unity.fast_exit` process policy applies only after an engine-requested terminal render and a checked synchronous preference flush. It returns the distinct process-exit result with native/service state pinned; the CLI immediately uses its existing `_exit` boundary. Failed writes, errors, input shortcuts and smoke limits retain full teardown. Only the validated Silksong launcher enables it; no managed hook terminates the process.
- `unity.separate_player_thread` defaults false. Its bounded owner-protocol
  comparison keeps initJni, UI drain and SDL input on the creator; one attached
  UnityMain owns later native lifecycle, render and nativeDone. UI continues
  through service/native cleanup, input stops before teardown, and the owner
  joins before returning. Throwables cross threads only as globals; the first
  creator exception is retained while cleanup UI remains serviceable. A native
  call that never returns is not cancelled or detached. This policy rejects
  fast_exit before init, because joining an owner cannot preserve its live TLS
  for process exit. It is not a full Android Looper/lifecycle implementation.
  The owner prepares the framework's private CallerLooper after attachment,
  drains it before each input/render dispatch and finishes it after nativeDone
  and service cleanup, before detaching. Callback exceptions stop the next
  frame and retain their original throwable through cleanup.
- With both diagnostics and `RUDROID_DIAGNOSTIC_SYNC=1`, that creator loop polls
  Bionic's private `SyncDiagnostics.snapshot` service at most once per second.
  Its local class reference survives until the player owner joins; snapshot
  exceptions enter the same creator-error path before cleanup can replace them.
  No extra observer thread or guest synchronization result is introduced.
- False returns are retried only before the first successful render. A later false is an engine-requested terminal stop and goes through nativeDone, service joins and preference flush; pending exceptions still fail. Do not turn in-game quit into a paced 20,000-iteration retry or an error.
- Every creator-thread input pump first drains the framework Android UI queue
  and propagates callback exceptions before input/render. Unity 2022 does not
  call the legacy executeMainThreadJobs callback. This processes jobs between
  native frames; it is neither Unity's GL-job queue nor an independent UI loop
  during a blocked nativeRender. The CLI native spy checks a reposted job chain
  across all profiles and an exception stopping the next native render.
- Private timing investigations may set `RUDROID_DIAGNOSTIC_PRE_RENDER_HOLD_MS`
  to 1..=30000 only with `RUDROID_DIAGNOSTICS=1`. This holds after the real
  init/recreate/resume/focus sequence and before the first input/vsync/render
  dispatch while native workers continue. It changes no guest result and
  proves no SDK or graphics cause; normal runs ignore the flag.
- With diagnostics and `unity.log_frame_progress`, the first three input-pump
  and nativeRender calls report entry/return/time metadata. This bounded trace
  distinguishes a first true return from later progress; no JNI arguments or
  SDK return value are changed. Existing frame counts remain every60 frames.
- A private Linux investigation can set `RUDROID_DIAGNOSTIC_FRAME_PHASES`
  to a new absolute CSV path with `RUDROID_DIAGNOSTICS=1`. The inline
  UnityMain loop samples every 64th steady iteration after the first
  successful startup render. It records UI/input, nativeRender and
  post-render wall/thread CPU, with requested and observed host sleep.
  The bounded samples and two monotonic/realtime anchors are written at
  loop exit. Leave the flag unset during acceptance timing.
- An optional `RUDROID_DIAGNOSTIC_FRAME_SCHED_THREAD` may name one Linux
  thread when the phase trace is enabled. It brackets read-only scheduler
  counters for the actual player thread and that peer around each sampled
  render. It records unavailable counters, observer cost and monotonic bounds
  in the sibling `.sched.csv` at loop exit, with no scheduling policy changes.
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
| Manifest | `Cargo.toml` | Plain library; depends on `rudroid-common` and `rudroid-unity-common` |
| Spec and registration | `src/lib.rs` | `Spec`, `CallbackBody`, `Profile`, `register_jni`, render-sleep clamp |
| Guest plan | `src/guest_plan.rs` | Unity ARM64 `.so` order, mono adjustments, `validate_config` |
| Mono service query | `src/mono_service.rs` | Typed lookup for `rudroid.unity.mono.v1` |
| Lifecycle | `src/lifecycle.rs` | Prepare/bootstrap/run, boot overlay, Mono `start_bridge`, render loop, stop/teardown, spy echo |
| Phase sampler | `src/phase_timing.rs` | Optional `diag-probes` feature for bounded UnityMain phase and clock-alignment diagnostics |
| Sched peer counters | `src/render_schedstat.rs` | Optional `diag-probes` peer-thread scheduler samples used by `phase_timing` |
| Player owner | `src/player_thread.rs` | Joined native owner and creator UI/input scheduling; sticky stop/error |
| Java services | `src/java.rs`, `src/network_proxy.rs`, `src/network_connectivity.rs` | Shared safe reflection/proxies, profile-specific PAD signatures and Linux host queries |
| Soft keyboard | `src/soft_input.rs` | Safe bounded VM-owned legacy text, active key routing, confirm/cancel and reentrant cleanup |
| Spy surface | `src/spy.rs` | Spy export names, event kinds, `RudroidSpyEvent` layout |
| Unit tests | `tests/unit/` | Spec validation, callback signatures, clamp, spy decoding, API pin |
