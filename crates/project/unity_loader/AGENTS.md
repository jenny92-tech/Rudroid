# Unity loader

> Thin Unity Project entry that discovers deployed capability libraries, validates and orders them, runs their phases, maps the guest SOs and drives the Unity profile lifecycle.

## Role
Package `rudroid-unity-loader`: binary `rudroid-unity-loader` plus a library used by the CLI and tests; not a capability, no descriptor. It owns the kit's shared runtime registry and reaches every capability only through `dlopen`/`dlsym`; Deploy Selection copies it only with `--include-loader`.

## Design

- Run order: config/policy/resource authority → validated capability phases →
  host session → prepare → guest loading → bootstrap → run. Discovery validates
  ABI/build/id/kind/test markers and orders android_base before dependents.
- Config and resource views publish under one immutable process-pinned owner.
  Explicit render-thread overrides need an exact verified native build and boot
  contract; unknown/ambiguous contracts reject. Engine-default skips boot I/O.
  The optional bounded cpuinfo snapshot defaults off and falls back to native reads.
- Real runs enter the existing absolute game-files root before constructors/JNI.
  Check/phase-only paths keep cwd. The `unityloader <toml>` alias resolves the
  sibling `rudroid.runtime` directory for existing launchers.
- After guest entry, ordinary success/failure finish owned services and use the
  terminal process boundary. Failure keeps its nonzero code; neither path may
  replay native atexit callbacks against state retired by nativeDone. The explicit
  process-exit completion instead preserves pinned workers/native state after
  engine stop and a checked preference save, then terminates immediately.
  Embedders must consume this distinct result without teardown or further work. Pre-guest
  failures retain ordinary process exit. The boundary never replaces cleanup.
- `host_platform` chooses the adapter at startup; SDL owns the implemented native
  session. Resolve/publish one measured output/render pair before preparation.
  Java/EGL/NDK/input share it; optional native Wayland presentation requires linear
  filtering and releases failures before fallback. No per-frame backend dispatch.
- Registry/resource callbacks run outside locks. Singular slots are first-wins;
  shared state lives in this registry/VM rather than per-DSO common statics.
  Diagnostics distinguish native loading, successful swaps and actual gameplay.
- Explicit `gpu.graphicsApi=vulkan` uses the optional SDL Vulkan entry, without
  GLES fallback. Its preflight verifies hardware/window support and reports zero
  game frames; it cannot satisfy the separate GLES verification contract.
- `--graphics-features` opens only the trusted same-kit SDL capability and reads
  its compiled feature tokens; no config, SDL initialization, phases or guest.
  Launchers intersect these tokens with title and platform support, never treating
  the query as GPU/shader acceptance. `--probe-vulkan` separately creates and
  drops a disposable native session, validates Vulkan 1.1+/zero game frames,
  and emits an exact success marker without phases, activation or guest.
  Launchers can fall back to GLES before guest startup after a failed probe.

## Constraints
- `load_deploy_dir` is unsafe: native constructors run before descriptor validation. Opened libraries are immediately process-pinned, including rejected members. `guest::bring_up` also requires trusted guest code and stable backing files. The CLI isolates these launches in two small helpers; other CLI code denies unsafe.
- Never link a capability statically in production; one test binary must not use two capability rlibs (each exports `rudroid_plugin_descriptor`).
- Capability handles, exec mappings, registered sources and the SDL session stay pinned for the process; add no `dlclose` or destroy on success paths.
- Keep the descriptor check order, `TEST_BUILD_MARKERS` identical to `crates/scripts/kit_markers.py`, and the compatibility alias `runtime::sdl_api` in step with `platform_sdl_runtime`.
- Never call callbacks or guest symbol backends under a registry or exec-table lock; `--check`, `RUDROID_SKIP_GUEST`, data-only mapping or a missing profile never report a game start.
- Tests build cdylibs and fixtures through `rudroid_test_support::cargo`; platform-gated tests use `#[ignore = "reason"]`, and `real_game_startup` is opt-in with local game data. `RUDROID_VAMPIRE_GAME_DIR` selects that directory; the fixed Docker entry mounts a host override read-only. Missing inputs fail the explicitly selected test.
- The production Java bridge probe covers both profiles' default absent presentation, explicit route enable/disable, retained-route behavior, repeated route/display identity reuse, invalid display lookup and independent system-UI flags; device gameplay/audio still require separate acceptance.

## Contents

`tests/fmod_teardown.rs` exercises real VM/framework/FMOD DSOs with a bounded
synthetic output and a nativePause that really stops audio: streamed native releases need mixer progress during nativeDone,
the owner joins before freeing state, and pre/post hook exceptions retain their
identity through final cleanup. The default terminal path must drain without an
injected pause; a separate ordinary pause still stops its producer. Both profile
spy contracts require nativeDone exactly once and no terminal nativePause. This
is separate from real-driver/game audio.
The same fixture checks recovery after zero-accept output stalls and native
release/owner-stop progress while the output keeps rejecting PCM.

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | Binary and library targets; test-only dev-dependencies |
| Build script | `build.rs` | `cfg(rudroid_kit_sdl)` for tests from `RUDROID_KIT_FEATURES` |
| CLI | `src/main.rs` | Argument parsing, run/check modes, result banners, service cleanup and post-guest success/failure completion |
| Loader library | `src/lib.rs` | Descriptor loading, `TEST_BUILD_MARKERS`, `load_deploy_dir`, `bring_up` |
| Process completion | `src/lib.rs`, `tests/process_completion.rs`, `tests/fixtures/process_finalizer.c` | Explicit post-guest terminal exit with preserved success/failure status and no retired atexit callbacks |
| Guest bring-up | `src/guest.rs` | `GuestPlan`, symbol sources, exec mapping, module events, `JNI_OnLoad` |
| Host API | `src/host_api.rs` | Installed config and log sink, `HostApiV0` table, phase calls |
| Phases | `src/phases.rs` | Phase order and known divergences from the C++ host |
| Resource authority | `src/resources.rs` | Immutable base/pack snapshots, bounded Host API v4 path/directory replies; common does not hold cross-library statics |
| Startup file view | `src/boot_view.rs` | Safe bounded extracted boot read, unlinked owned file, independent cursors and exact-source redirection |
| CPU identity view | `src/cpuinfo_view.rs` | Opt-in bounded sealed proc text with exact-path projection and process-owned backing File |
| Host platform startup | `src/host_platform.rs` | Safe backend/session transaction, verified output and render publication; independent of Unity/title details |
| SDL adapter | `src/platform_sdl.rs` | Pinned SDL C ABI, failed-session cleanup, EGL/ELF wiring and historical API symbols |
| Runtime registry | `src/runtime.rs` | Registry, catalog state machine, exec table, profile runner and compatibility aliases |
| Selection | `src/select.rs` | `[plugins].enabled` allowlist validation and stem matching |
| Validation | `src/validate.rs` | Topology checks, deterministic order, diagnostic JSON |
| Unit tests | `tests/unit/` | Loading, allowlist, validation, getenv pointers, catalog leases and backend-independent display failure/unwind ownership |
| Integration tests | `tests/*.rs` | CLI, guest bring-up, Bionic/zlib/media/events guests, registry, SDL, real game, production JNI casts/critical strings/scalar and nonvirtual return ABIs/primitive arrays/persistence, both Unity profiles' Java/PAD/proxy dispatch, isolated configured storage-directory/reflection/ownership checks and StatFs snapshot/error/wide-integer/ownership contracts, legacy soft-input ownership/limits/keys/reentrant close and event-consumer unwind, Terraria Activity graph/package isolation/float callback, creator-thread input/hotplug bridges and main-Looper GMS/Play Core task snapshots |
| Fixtures | `tests/fixtures/` | Guest C probes and out-of-workspace fixture capability crates |
