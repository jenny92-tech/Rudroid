# Android framework services

> `android_base`: the Android/Java service facades (Context, preferences, scheduling, input, display, lifecycle) on the shared JNI VM.

## Role
Capability `android_base`, `Kind::AndroidBase`; package `rudroid-android-framework`, cdylib + rlib `libandroid_base.so`, the Rust counterpart of the C++ `android_base` plugin.
Every runnable kit deploys exactly one.

## Design

- Join the shared VM, register complete natives and publish first-wins runtime
  hooks. The VM owns every object/field; common supplies clocks/filesystem and
  the loader supplies resource resolution and measured guest geometry.
- Preferences use isolated editors and atomic JSON persistence; reads never
  migrate original stores, non-finite floats reject, apply flushes at lifecycle end.
  Lifecycle.flushPreferences returns true only after every dirty store was saved;
  failures retain dirty data and refuse the opt-in process-exit path.
  StatFs publishes one immutable snapshot on construction/restat; failed restat
  keeps the old value and numeric conversions follow Android.
- Context directories are created then canonicalized. AssetManager and installed
  split identities use the shared resource authority without copying assets.
  Input identity stays device-owned; Build.MANUFACTURER defaults to Rudroid to
  avoid Android libraries selecting Linux control layouts.
- Handler binds the current owned Looper; threads and Choreographer share monotonic
  time. Orientation requests do not rotate the host display. Unavailable services
  stay explicit, without implicit workers or fake hardware owners.
- MediaRouter has no presentation route by default. Profiles may enable the
  cached host-display facade only with build-specific evidence; it can trigger
  audio reinitialization. Enabling it does not prove an external monitor exists.
- Native Unity TLS receives bounded real system CA data, honoring SSL_CERT_FILE.
  Invalid roots/custom Java stores reject; peer/hostname verification stays active.
- At most 32 shutdown Runnables quiesce independent services before nativeDone,
  then join native-owned producers/workers before freeing state. Hooks run once,
  in reverse outside locks, release roots and preserve the first exception.

## Constraints
- Never link `android_jni_vm` or another capability; reach the VM through `HostApiV0::java_vm` and the typed `rudroid_common::jni_env` wrappers (no slot numbers here).
- The shared VM is the only object/field authority: declare fields via `common::jni_abi`/`TypeRegistry`; JNI fields hold integer ids, never Rust pointers.
- Register natives with complete descriptors; unregistered typed accessors stay unavailable (no silent defaults).
- Join every background worker at shutdown; hold no lock across reentrant native calls or a worker's final JNI detach.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_base`; `common`, `reply_catalog/format`, `serde_json` |
| Crate root | `src/lib.rs` | Descriptor, register-JNI phase, hooks, session join, `Framework` API |
| Context | `src/java.rs` | Context paths, package info, class loader, UI job queue |
| Storage statistics | `src/statfs.rs` | Safe StatFs path queries, atomic VM snapshot publication and cached 32/64-bit block/byte accessors |
| Preferences | `src/preferences.rs` | Per-name store cache, isolated editors, atomic JSON persistence |
| Collections | `src/collections.rs` | Preference snapshot Map, shared array-backed Sets/Lists/Iterator |
| Services | `src/services.rs` | Display, window, resources, configuration, locale, system services |
| URI conversion | `src/uri.rs` | UTF-16-preserving Uri.decode; escaped UTF-8, literal plus and malformed-input replacement |
| Native TLS roots | `src/trust_store.rs` | Bounded system PEM/DER bridge to JNI, initialized factory state, fresh certificate byte arrays; no Java TLS implementation |
| Threads | `src/threads.rs` | Thread/HandlerThread workers, deadline queues, shutdown join |
| Choreographer | `src/choreographer.rs` | One-shot frame callbacks at the configured refresh rate |
| Input | `src/input.rs` | Device-tagged events and calibrated axis getters/MotionRanges (HAT ranges require the producer-owned hasHatAxes flag); InputManager listener roots are promoted under the ownership lock, then callbacks run outside locks, optionally through Handler-owned Runnables; physical ids remain non-virtual |
| Key codes | `src/keycodes.rs` | Android 15 `KeyEvent` constant table |
| Lifecycle | `src/lifecycle.rs` | Bounded shutdown hooks, `shutdown`, `finish` |
| Unit tests | `tests/unit/` | Class set, catalog fallback, hooks, session join via fake JNI tables |
