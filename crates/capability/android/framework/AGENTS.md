# Android framework services

> `android_base`: the Android/Java service facades (Context, preferences, scheduling, input, display, lifecycle) on the shared JNI VM.

## Role
Capability `android_base`, `Kind::AndroidBase`; package `rudroid-android-framework`, cdylib + rlib `libandroid_base.so`, the Rust counterpart of the C++ `android_base` plugin.
Every runnable kit deploys exactly one.

## Design

- Join the shared VM, register complete natives and publish first-wins runtime
  hooks. The VM owns every object/field; common supplies clocks/filesystem and
  the loader supplies resource resolution and measured guest geometry.
- The host facade object factory explicitly declares its provided class before
  allocation; it is not a guest class query. This also covers lazy singleton
  types such as ContentResolver under a verified application class catalog.
- Preferences use isolated editors and atomic JSON persistence; reads never
  migrate original stores, non-finite floats reject, apply flushes at lifecycle end.
  Lifecycle.flushPreferences returns true only after every dirty store was saved;
  failures retain dirty data and refuse the opt-in process-exit path.
  StatFs matches the legacy C++ compatibility view: construction/restat ignore
  the path and publish 4096-byte blocks with 32 GiB total/free/available. The
  host filesystem statistics and real writes are unaffected; numeric conversions
  and VM-owned snapshot lifetimes still follow Android.
- Context directories are created then canonicalized. AssetManager and installed
  split identities use the shared resource authority without copying assets.
  Input identity stays device-owned; Build.MANUFACTURER defaults to Rudroid to
  avoid Android libraries selecting Linux control layouts.
- `ApplicationInfo.minSdkVersion/targetSdkVersion` read the APK declarations
  from `package.minSdkVersion/targetSdkVersion` (legacy defaults23/26).
  Positive Java-int values are required. These package facts never change
  the independent advertised `Build.VERSION.SDK_INT` or Bionic API level.
- `Intent` keeps both Android constructors: `new Intent()` is the shared VM's
  fresh-object field default (null action/package/categories, zero flags) and
  writes nothing, while `new Intent(String)` sets the action. Neither merges
  with nor resets the distinct launcher Intent behind `Activity.getIntent`.
- String extras live in per-object VM fields/arrays, with null-key/value and
  replacement semantics. `Intent.getExtras` returns a detached Bundle snapshot;
  only the launcher Intent reads immutable `package.mainIntentBundle` defaults.
  Normal Intents and snapshot mutations cannot change the launcher bundle.
- `IntentFilter` constructors and action-list operations own a VM-rooted,
  per-object ordered string set. Equality uses UTF-16 contents, duplicates do
  not append, null additions and invalid indexes throw. This bounded surface
  supplies no broadcast registration or MIME/data/category matching.
- Handler binds the current owned Looper; threads and Choreographer share monotonic
  time. Orientation requests do not rotate the host display. Unavailable services
  stay explicit, without implicit workers or fake hardware owners.
  Private CallerLooper begin/drain/finish lets a native owner prepare and drive
  its own queue without spawning a thread or borrowing the UI queue. Due jobs
  run outside locks within a drain snapshot; finish drops queued roots and the
  Looper root, clears the current binding and makes stale Handler posts reject.
  Message dispatch first runs its actual Runnable callback, then the Handler
  callback, then handleMessage; exceptions never fall through. Message.obj is
  payload, not an implicit task.
- `android/app/ActivityThread` is a bounded process-identity facade: only
  `currentActivityThread()Landroid/app/ActivityThread;`, static
  `currentApplication()Landroid/app/Application;` and instance
  `getApplication()Landroid/app/Application;` are registered. Both Application
  accessors return the SAME rooted object as `Context.getApplicationContext`,
  because a second Application would split preferences, directories and package
  identity behind the guest's back. No other Android ActivityThread member
  (`mBoundApplication`, `mInitialApplication`, `getSystemContext`, activity or
  thread-scheduler bookkeeping) exists; those lookups stay explicit VM misses,
  and no zygote, activity stack or scheduler is implied.
- A title with an actual Application publishes it through private
  `org/rudroid/runtime/ApplicationOwner.publish(Application)Z` before any
  generic fallback escapes. Context and both ActivityThread accessors return
  fresh locals of that same rooted object. Repeated same-identity publication
  succeeds; null, wrong type or a different existing owner rejects. An escaped
  fallback is never silently replaced. Publication implies no DEX execution.
  An explicit owner can call `reserve()Z` before guest static initialization;
  until publication, queries return null for the not-yet-created Application.
  Reservation and fallback arbitrate under the same lock, including a fallback
  candidate allocated before reservation. Failures never release a reservation
  into a new generic identity; other titles retain their existing lazy fallback.
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
- The opt-in separate player owner keeps creator UI jobs open through
  nativeDone and post-native joins, then closes them in finish. Its player
  driver must acknowledge creator input quiescence before native cleanup;
  the default single-thread cutoff is unchanged. Private UI entry/nonempty
  drain diagnostics are capped at16 metadata records without task arguments.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `android_base`; `common`, `serde_json` |
| Crate root | `src/lib.rs` | Descriptor, register-JNI phase, hooks, session join |
| Context | `src/java.rs` | Context paths, package info, class loader, UI job queue, bounded ActivityThread identity facade |
| Application owner | `src/application.rs` | First-wins rooted process Application and private publisher, with generic fallback |
| String extras | `src/intent_extras.rs` | Per-Intent string Bundle state, snapshot copies and isolated launcher-config defaults |
| Intent filter actions | `src/intent_filter.rs` | Per-object UTF-16 action sets, constructors and bounded public action operations |
| Storage statistics | `src/statfs.rs` | Legacy 32 GiB capacity view, atomic VM snapshot publication and 32/64-bit block/byte accessors |
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

JNI object factories live in `src/jni_helpers.rs`, package path selection in `src/package_paths.rs` and service registrations in `src/service_registration.rs`. The VM owns boxed Integer, Long and Boolean fields and accessors.
