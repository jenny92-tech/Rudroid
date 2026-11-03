# GMS Android Addon

> Offline Google Play Games / Task facade on the shared JNI VM: tasks complete, listeners fire on the main Looper, sign-in is never faked.

## Role
Capability `sdk_google_gms`, kind `android_addon`, requires `android_base`; package `rudroid-android-addon-gms` (cdylib + rlib). Optional: loaded only when Deploy Selection ships it (Silksong does not).

## Design
- `rudroid_plugin_register_jni`: without a shared VM it records `NoSession` and returns 0; otherwise it runs `java::register`, then joins the session (`Task.isSuccessful()` true, `AuthenticationResult.isAuthenticated()` false; failure -3, see `last_outcome()`).
- `isAuthenticated()`/`signIn()` return a completed `Task` whose `result` is an unauthenticated `AuthenticationResult`.
- `addOnCompleteListener`/`addOnSuccessListener` post the common VM-owned completion job to the main Looper that calls the listener via `invoke_proxy_callback`; `addOnFailureListener` queues nothing.

## Constraints
- Never report a successful login.
- Listener callbacks run only from the queued main-Looper job, never synchronously inside `addOn*Listener`.
- Task creation/listener snapshots reuse `common::jni_tasks`; no duplicate callback queue or capability linkage.
- Task/job state lives in VM fields declared through `jni_abi::declare_field`; no Rust pointers in JNI fields; delete every local reference.
- Call JNI only through `rudroid_common::jni_env::JniEnv` (no slot numbers here); never link `android_jni_vm`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `sdk_google_gms`; depends on `rudroid-common` |
| Descriptor and join | `src/lib.rs` | Descriptor, register-JNI session join, `RegistrationOutcome` |
| Java facade | `src/java.rs` | Natives, offline auth results, common Task creation; GMS authentication facade |
| Unit tests | `tests/unit/` | Owned interfaces, reply constants, outcome tracking |
| Fixtures | `tests/fixtures/` | Guest `libmain.so` probing GMS/Play Core from `JNI_OnLoad`; built by the loader's `addon_module_events` test |
