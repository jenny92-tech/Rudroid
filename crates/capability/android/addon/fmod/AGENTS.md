# FMOD Android Addon

> Registers the `org/fmod` Java surface, mixes FMOD audio into the selected `AudioOutput`, and hooks `SetOutput` in the mapped `libfmod.so`.

## Role
Capability `sdk_fmod`, kind `android_addon`, requires `android_base`; package `rudroid-android-addon-fmod` (cdylib + rlib). Silksong deploys it.
No SDL code: playback belongs to the Linux backend that implements `org/rudroid/runtime/AudioOutput`.

## Design
- `rudroid_plugin_register_jni` registers the `org/fmod` stubs (guest libraries present or not), then joins the session: `FMOD.checkInit()Z` must answer true.
- `FMODAudioDevice.start` takes the single mixer lease and spawns `rudroid-fmod-mixer`, looping `fmodProcess` → `AudioOutput.write` on a direct PCM16 buffer, reopening on format change. It consumes the reference clocked/silence/cached-format/diagnostic switches: transient invalid formats reuse only a complete validated cache when opted in; unready mixers output cleared PCM; clocked mode follows one block deadline. The VM zeros only owned PCM storage before synchronous mixing; borrowed buffers are refused.
- The pre-native Lifecycle hook blocks new starts but keeps existing mixers processing native streaming-release commands. The engine's AudioDevice.stop/close callback synchronously joins its producer before freeing mixer state; a post-native Lifecycle hook provides the final join and partial-init cleanup. It creates no extra worker or frame work.
- `rudroid_plugin_init` subscribes to Unity/Fmod/FmodStudio events: Unity checks the guest `.so` files (`StubbedMissingGuest` if both missing); Fmod detours the first resolved `SetOutput` symbol (`Bridged` or `HookFailed`).
- The `SetOutput` hook tries AUDIOTRACK → NOSOUND → OPENSL → AUTODETECT → requested, falling back to NOSOUND + OK; reentrant calls go straight to the original. The typed `Original` installer owns detour installation and original publication together.

## Constraints
- Both Rust `plugin_init` and native init are unsafe: host callbacks and resolved FMOD code stay mapped, and module events provide a quiescent installation window for the detour. A hook-local closure forwards reentrant and fallback calls through one typed boundary without retaining the System pointer.
- At most one producer is active. The native SDK owner must call stop/close before freeing callback state; do not stop the mixer before nativeDone can drain streaming releases. External stop joins before returning; a reentrant producer stop only requests cancellation and leaves its handle for a later external join, so its native owner must defer free until that join. Never hold START/runner/registry locks across host callbacks or worker detach. Code and VM buffer/global-reference owners remain valid through the last producer call.
- JNI fields hold integer identities (`runnerId`), not Rust pointers; PCM buffers are VM-owned direct buffers.
- The detour exists only on Linux aarch64 and is process-lifetime; other targets return an explicit error.
- Report outcomes truthfully via `last_outcome()`: a missing symbol is `HookFailed`, never a silent pass.
- Tests use the synthetic probe, never the licensed vendor `libfmod.so`/`libfmodstudio.so`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `sdk_fmod`; depends on `rudroid-common` |
| Plugin and hook | `src/lib.rs` | Descriptor, session join, module events, `SetOutput` detour, `FmodOutcome` |
| Mixer bridge | `src/audio.rs` | `org/fmod` natives, producer threads, `AudioOutput` writes, shutdown hook |
| Unit tests | `tests/unit/` | Reference constants, outcome tracking, `stop_all` joins producers |
| Fixtures | `tests/fixtures/` | Synthetic `libfmod.so` recording `SetOutput` tries; built by the loader's `addon_module_events` test |
