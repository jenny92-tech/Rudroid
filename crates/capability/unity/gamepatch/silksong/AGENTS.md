# Silksong gamepatch

> Silksong Title patch merging the reference `silksong_unity6` and `silksong_viewport` patches into one package-gated capability with real IL2CPP detours.

## Role
Capability `silksong_unity6`, kind `gamepatch`, package gate `com.game.silksong`, requires `il2cpp_patch` (hooks install on its `Il2cppReady` event); deployed for Silksong.

## Design

- Package-gated config supplies features; from_config/from_host_api agree.
  SignVerify.getAPKPath delegates only the package path, never ownership results.
- Il2cpp binds exports, Il2cppReady installs metadata-checked typed detours.
  Feature-disabled entries remain intact; originals publish only after success.
  Screen getters remain paired with MethodInfo, with unwind-safe viewport scopes.
- Successful localization enum switches preserve the original bool result, then
  save the current enum's managed name through PlayerPrefs under M2H_lastLanguage,
  the key read by this port's next startup. Metadata and Int32 enum identity are
  checked; temporary boxed/string objects use verified callback-local roots.
  Failed switches, unknown signatures, managed errors and stop keep native behavior;
  no frame polling, language forcing or Android-wide preference aliases.
- Application.Quit suppression excludes only the call chain of the checked
  Silksong.dll instance void UIManager.QuitGame() action and the validated
  GameManager.QuitGame IEnumerator's nested MoveNext callback. Discovery follows
  the declaring class's nested metadata, without hardcoding compiler ordinals.
  Original UI/factory/iterator behavior remains intact; each callback's thread-local
  scope restores on return/unwind and retains no receiver or between-frame permission.
  Background/startup requests keep the existing policy, unknown or
  ambiguous/generic signatures retain native entries, and no hook terminates the
  process or adds frame polling. Engine stop and final joins stay in the profile.
- Optional title-bundle unloading defaults off. Only Menu_Title advances its
  delay; leaving cancels, one attempt is consumed before the call and errors stop
  retries. No managed object survives between frame callbacks.
- Scene postprocessing preserves original Start, waits 30 successful frames,
  then applies selected rooted Behaviour/static-Sprite policies. Stop/reentry/
  exceptions cancel; no texture/bundle unload is hidden in pruning.
- Launcher/bootstrap preserve original events, validate signatures and defer the
  scene transition. Named mobile objects are deactivated with callback-local roots,
  not destroyed. Completed generations return before locks/engine work.
- The checked Android MsgBox layout hides only `/Launcher/MsgBox/Panel` after
  original Start and before drawing, under the existing skip-launcher switch.
  The launcher remains active for its button callback and delayed transition.
  Roots are callback-local; unknown layouts/missing panels retain native startup,
  and cancellation/error adds no retries or gameplay polling.
- TouchManager belongs to the required common il2cpp_patch adapter; this title
  never installs a second hook. Destroy, post-prune UnusedAssets and census/method/
  audio traces remain registered-only; see [capability limits](../../../../docs/MIGRATION-MAP.md#剩余差异与未支持范围).
  Fixture success and short gameplay paths are separate acceptance evidence.

## Constraints
- Both the Rust `plugin_init` and native init are unsafe: the same-kit Host API must keep callbacks and resolved native code mapped, deliver valid module events and provide a quiescent hook-installation window.
- Hook resolution validates staticness, parameter/return kinds and byref flags through common `method_signature`; missing or mismatched metadata leaves the native entry intact.

- `EnableParticles` uses native `bool` on both Rust and C fixture sides, not a same-sized integer.
- Managed-method hooks and original slots use `C-unwind`; guest exceptions may cross Rust frames. Temporary thread state must restore on unwind as well as return.
- The package gate matches exactly; gate mismatch stays side-effect free. Repeated init re-arms process state and requires an idle installation window.
- Disabled switches install nothing; a missing required export fails the bind without side effects.
- No game offsets: targets come from metadata. Detours come from `arm64_detour`, only in the `Il2cppReady` window.
- Registered-only entries must not be reported as installed until their runtime surface exists.
- Unit tests read `work/config.silksong.device.toml`; keep its flags and the tests consistent.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `silksong_unity6`; common, host-config, libc |
| Patch | `src/lib.rs` | Descriptor and gate, features, `plan()`, hook bodies, `install`, init |
| Java resource helper | `src/java.rs` | Title package path callback delegated to the shared Context |
| Language persistence | `src/language.rs` | Checked enum-switch completion, managed language name and normal PlayerPrefs save |
| Cinematic fallback | `src/cinematic.rs` | Checked original Begin followed by normal Skip, cancelled by stop |
| Startup transition | `src/launcher.rs` | Checked original button event and deferred scene loading with callback-local ownership |
| Startup UI and mobile policy | `src/bootstrap.rs` | Rooted startup-panel hiding while the launcher remains active; named mobile deactivation after accepted initialization-scene request |
| Scene render pruning | `src/postprocess.rs` | Original scene-start notification, 30-frame delay, feature-gated Behaviour/static Sprite disabling and cancellation |
| Title bundle policy | `src/after_title.rs` | Player-thread ownership, title-delay gate, stop/reentry guards; no managed objects retained between frames |
| Unit tests | `tests/unit/` | Gate, defaults, numeric windows, device-config flags, config parity |
| Integration tests | `tests/behavior_fixture.rs` | Production path against a compiled fake IL2CPP |
| Fixtures | `tests/fixtures/` | Fake `il2cpp_*` API and game model, including deferred menu/coroutine quit and metadata rejection |
