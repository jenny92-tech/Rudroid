# Unity 6 profile

> Unity 6 engine profile: a `Spec` literal, the Unity 6 Java services and the runtime-invoke Bionic TLS guard.

## Role
Capability `unity_6`, kind `unity_profile`; deployed for Silksong and required by `vampire_survivors`. The loader calls `rudroid_plugin_register_jni` (early JNI phase), then `rudroid_plugin_init` (native phase).

## Design
- `ENGINE_SPEC` mirrors C++ `plugins/unity_6`: player `UnityPlayerForActivityOrService(Context)`, initJni `(Landroid/content/Context;I)V`, extra `initializeGoogleAr` callback, `currentActivity` published, shared `java::register_6` services.
- The native phase requires a successful JNI phase, subscribes to `ModuleKind::Il2cpp` and installs the TLS guard on `il2cpp_runtime_invoke`.
- Terminal stop follows common's nativeDone ownership without an extra suspend; ordinary registered pause/resume remains available.
- The shared Java bridge supplies the disabled accessibility delegate's init/cleanup contract, so nativeDone never has to call cleanup on a missing delegate. Its object belongs to the shared VM and allocates no host accessibility resources.
- TLS guard: Bionic's stack guard at TP+40 is ordinary TLS on glibc threads. After matching the full four-word `ldr/ldur/cmp/b.ne` pattern in the first 256 words, only that `b.ne` becomes `NOP`. Installation checks readable mapping bounds before following the branch and scanning.
- Shared `player_lifecycle::java`: interface proxies dispatched by `invokeKind`, `ReflectionHelper` lookups via `TypeRegistry`, `UnityPlayer` no-op natives, local installed-only Play Asset Delivery queries through the loader resource authority. `removeAssetPack` preserves immutable extracted local inputs, matching the reference bookkeeping no-op.
- `UnityPlayer.getNetworkProxySettings` explicitly returns null in the runtime's default no-Java-proxy environment, matching C's fallback for this query. Null URL input throws; no proxy configuration or routing service is added.

## Constraints
- Do not copy prepare/bootstrap/run here; differences go in `ENGINE_SPEC`, and the descriptor id stays `ENGINE_SPEC.id`.
- The shared VM owns all objects and fields. Never store Rust owner pointers in JNI fields; register natives with full descriptors.
- The TLS guard patches only the comparison branch after a complete pattern match; host and capability stack protection stay intact.
- PAD answers only local installed-file queries; never synthesize download, DLC or network-delivery success.
- `spec_matches_reference_literal` and the `PROFILES` row in `crates/project/unity_loader/tests/profile_lifecycle_fixture.rs` pin the Spec.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | `cdylib` + `rlib` `unity_6`; common and `player_lifecycle` |
| Profile | `src/lib.rs` | Descriptor, `ENGINE_SPEC`, JNI delegation, native init, `tls_guard` |
| Java services | `../player_lifecycle/src/java.rs` | Shared safe interface proxies, reflection, host queries and local PAD; this profile selects Unity 6 signatures |
| Unit tests | `tests/unit/` | Spec literal, registration, aarch64 pattern scan and branch patch |
