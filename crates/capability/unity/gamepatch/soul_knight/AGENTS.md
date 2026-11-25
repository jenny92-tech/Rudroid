# Soul Knight bootstrap gamepatch

## Role
Capability `soul_knight`, kind `GamePatch`, exact package
`com.ChillyRoom.DungeonShooter`, requiring `android_base`, `android_jni_vm`,
`android_elf_load` and `unity_2022_3`. It is deployed only for Soul Knight
8.7.0/80700 and is inert for every other package.

## Protocol

- Plugin init verifies package, version name/code, the fixed `apks/base.apk`,
  `apks/config.arm64_v8a.apk`, `apks/UnityDataAssetPack.apk`, and exact SHA-256
  identities of the original container and `libanogs.so`/`libanort.so`.
  Any selected-title mismatch refuses startup before Main is mapped.
  The same gate requires the original APK's configured minSdkVersion23 and
  targetSdkVersion36; these are application declarations, not a change to the
  runtime's independent Android API level.
- The seven default base-APK DEX files are also checked by exact size and SHA.
  Each verified buffer supplies only its class_defs names to the shared VM's
  optional lookup catalog before guest constructors/JNI. Type references and
  `assets/audience_network.dex` are not default class definitions. This restores
  truthful missing-class queries without a title-specific class blacklist;
  it supplies no DEX bodies, constructor execution or dynamic-loader lifecycle.
  Host capabilities still explicitly declare their actual provided types.
- After that exact APK/catalog gate, the single provided
  `NativeUtil.getSignatureSHA1Hash` getter returns the original APK certificate's
  real uppercase SHA-1. This does not execute NativeUtil DEX or provide
  PackageManager, SDK/account initialization or a verification result.
- The adapter registers one loader callback at the generic post-Main-JNI
  point. It explicitly loads `anogs` then `anort` through the loader-owned
  load-and-JNI operation; neither is a preload and no Rust implementation is
  substituted for the original natives. `libu8t.so` is not preloaded: it needs
  `libanort`'s versioned exports and is loaded only after both SDK
  `JNI_OnLoad` calls and the original `initialize` succeed.
- After both original `JNI_OnLoad` calls, it allocates the manifest's concrete
  `com/chillyroomsdk/sdkbridge/BasePlayerApplication`, links its parent
  `com/ano/gshell/AnoApplication`, and calls that parent's original native
  `initialize(String,String,String,String,String[])I` with the exact
  package, absolute files directory, the canonical parent of the three
  verified original SDK libraries,
  absolute base APK, and the two real split APK paths.
- Return `0` is required. Any nonzero result or pending JNI exception is a
  hard failure before il2cpp. Fake success, `sem_post`, skipped `DT_INIT`, and
  synthetic split paths are forbidden.
- The actual manifest Application instance is published to the framework's private
  first-wins Application owner only after the original `initialize` returns0
  without an exception. The original call is inside `attach`: Android's
  LoadedApk/ActivityThread publish only after this attachment returns, so both
  application queries remain null during that native call. This restores the
  known observation point, without claiming the remaining Java attach/module
  callbacks have executed. Its inherited Context and
  ActivityThread queries must return that same object. A prior conflicting
  Application fails explicitly; no title-specific self-return override hides it.
  Plugin init first reserves this owner before guest static initializers run,
  so queries before instance creation see null rather than a generic fallback.
  Both original SDK JNI calls still precede instance creation and initialize;
  the receiver's local root stays alive through initialization and publication.
  Verified superclass metadata is declared during plugin init, before SDK JNI
  can inspect it; metadata declaration executes no constructor or DEX body.
  The two evidenced empty constructor layers do not execute attachBaseContext,
  MultiDex, module discovery, SdkConfig.load or onCreate. Fields remain at
  uninitialized defaults; neither IApplicationListener callbacks nor the full
  Java Application lifecycle are claimed by this bounded native bootstrap.
- The manifest Activity is `com/chillyroomsdk/sdkbridge/BasePlayerActivity`,
  extending `com/unity3d/player/UnityPlayerActivity`; the shared profile still
  allocates the latter. Unlike the empty Application constructor layers, the
  concrete Activity initializes five ArrayLists and a lifecycle HashSet, and
  its onCreate loads SDK configuration and starts agent initialization after
  the Unity superclass call. Renaming an AllocObject result would skip real
  constructor/lifecycle state. This identity/lifecycle gap remains unimplemented;
  do not declare the original Activity initialized or add its private fields
  to the generic Unity Activity to conceal it.
- The observed NetworkUtilFragment.Get and DirectoryPickerFragment.Get paths
  require FragmentManager transactions and real attach callbacks; the former
  also registers a connectivity receiver, the latter owns SAF result/grant/I/O
  state. A name/object factory cannot stand in for these SDK protocols. The
  Activity's static getAndroidID is a real share_data preference-cache query
  followed by Secure Settings; unavailable settings may yield null, and null
  SPUtil.put commits without deleting the old value. Do not invent an ID or
  claim that providing this getter executes the missing Activity lifecycle.

## Contents

| Path | Responsibility |
|------|----------------|
| `src/lib.rs` | Descriptor, gate, post-Main callback, original load order and `initialize` |
| `src/protocol.rs` | Original Ano class, seven native declaration descriptors and static library order |
| `src/identity.rs` | Exact package/version/build paths and SHA-256 gate |
| `src/sha256.rs` | Bounded streaming SHA-256 for original file identity |
| `src/signature.rs` | Exact-APK-gated original signing-certificate SHA-1 getter |
| `src/offline_sources.rs`, `src/offline_patches.rs`, `src/offline_trace.rs` | Owner-thread managed source values, lifecycle observations and Title entry |
| `src/offline_isolation.rs` | Verifies the original manifest and executable identities, isolated network namespace and save directory before hooks are armed |
| `src/offline.rs` | Default-off exact-build original-data/time/local-login sources; isolated profile and owner-thread root lifetime |
| `src/android_id.rs` | Public Android ID / Settings path cache for offline |
| `tests/unit/` | Gate/order/identity/android_id/sha256/offline unit bodies via `#[path]` |

## Constraints

- The APK bytes and ABI SOs are local `gamefiles` inputs; never add them to
  source or the knowledge base.
- Keep title names, class names, native declarations, offsets and path rules
  in this crate. Shared loader files expose only generic, fallible mechanics.
- Do not stub missing SDK dependencies or JNI methods. Stop at the first
  actionable original-protocol failure.
- Explicit `game_patches.soul_knight.offline` requires `il2cpp_patch`, the exact
  original IL2CPP/packaged manifest hashes, and an ownership-marked independent
  profile containing all four Android writable roots. Before SDK bootstrap it
  also requires a network namespace distinct from init's with loopback as its
  only interface; the launcher creates it before starting the runtime.
  Il2cppReady installs
  only full-signature/code-guarded source detours; object construction waits
  for the original owner-thread call. Retain original event/load order, real
  completed C# Tasks and fresh BCL UTC results. UnityStopping frees cached roots
  on that owner before nativeDone. This remains a candidate until level entry,
  saving and a fresh-process reload are accepted; neither ready flags nor hook
  installation constitute gameplay acceptance.
- Its generated configuration preserves the original fallback's eight labels
  and 2.7.0/100 values, but explicitly closes only FallbackCloudPackageRestore
  (enum 20). The original WaitCloudPackageState disabled-entry branch then owns
  continuation. The other seven entry values stay open; no cloud hook, fake
  restore success, manual NextState call or ready-field write is added.
- Enabled offline failures never invoke a remote original source. They retain
  an atomic failure, return null/void rather than synthetic success, and set a
  fixed pending JNI exception on attached hook/profile threads. Real frame or
  maintenance callbacks propagate the failure to the player owner; its existing
  exception path performs normal teardown. No fabricated UnityStopping, direct
  process exit or phone firewall is used as this mode's shutdown/network policy.
