# Play Core Android Addon

> Local Play Core Tasks and review-manager construction on the shared JNI VM.

## Role
Capability `sdk_google_play_core`, kind `android_addon`, requires `android_base`; package `rudroid-android-addon-play-core` (cdylib + rlib). Optional; Silksong does not deploy it.

## Design
- Register-JNI installs the common completed-local-task implementation in the Play Core namespace, plus an executable ReviewManagerFactory.create(Context). It retains the existing explicit NoSession/Joined/Failed outcome.
- Complete/success listeners become VM-owned main-Looper jobs; success stores the registration-time result snapshot, completion retains the task. Failure listeners are not called for these successful local tasks. Job registration never invokes callbacks synchronously; queue refusal is explicit.
- Ordinary listeners dispatch through their actual class; engine JavaProxy listeners use the existing proxy bridge. There is no download, online review submission, entitlement or Google account success.

## Constraints
- Own the Play Core namespace; reuse common's task machinery rather than linking the GMS capability. Shared task/job ownership resides entirely in VM fields.
- Call JNI only through `rudroid_common::jni_env::JniEnv` (no slot numbers here); never link `android_jni_vm`.

## Contents

| Name | Path | Responsibility |
|------|------|----------------|
| Manifest | `Cargo.toml` | cdylib + rlib `sdk_google_play_core`; depends on `rudroid-common` |
| Descriptor and join | `src/lib.rs` | Descriptor, owned class names, session join, `RegistrationOutcome` |
| Unit tests | `tests/unit/` | Namespace, descriptor and status constants, outcome tracking |
