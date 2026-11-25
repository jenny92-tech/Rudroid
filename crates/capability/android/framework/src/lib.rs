//! INPUT: shared JNI/Host API and safe Android service facades
//! OUTPUT: android_base registration, runtime hooks and JNI framework behavior
//! POS: keeps platform-facing Android mock business logic outside native pointer boundaries
//! Registers Android framework methods in the shared JNI session and supplies
//! the preference flush and vsync callbacks used by the Unity player.

use std::ffi::c_int;
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, OnceLock};

mod choreographer;
mod collections;
mod input;
mod intent_extras;
mod intent_filter;
mod jni_helpers;
mod java;
mod keycodes;
mod lifecycle;
mod listener_interfaces;
mod preferences;
mod services;
mod application;
mod statfs;
mod threads;
mod trust_store;
mod uri;

use rudroid_common::jni_env::{Arg, JNI_VERSION_1_6};
use rudroid_common::registry::RudroidAndroidHooks;

/// The Host API table copy the production hooks log through (ticket 77).
/// Installed by the register-JNI phase before any hook can fire — the loader
/// pins this library for the process life (ticket 46), and the table itself
/// is a process static on the loader side.
static HOST: OnceLock<rudroid_common::host_api::HostApiV0> = OnceLock::new();

/// Choreographer frame-signal counter (ticket 77): every `signal_vsync`
/// invocation bumps it atomically. Optional diagnostic logs expose the
/// count; the default hot path never formats, allocates or calls the logger.
static VSYNC_FRAMES: AtomicU64 = AtomicU64::new(0);
static LOG_VSYNC_FRAMES: AtomicBool = AtomicBool::new(false);

/// Table-bound hook; fires only after the register-JNI phase installed the
/// host table (the registration contract keeps the registered pointers
/// process-lifetime, ticket 45).
extern "C" fn flush_preferences_hook() {
    preferences::flush();
    if let Some(host) = HOST.get() {
        host.log_line(
            c"android_base",
            "flush_preferences: dirty shared preference stores flushed",
        );
    }
}

/// Table-bound hook; fires only after the register-JNI phase installed the
/// host table (the registration contract keeps the registered pointers
/// process-lifetime, ticket 45).
extern "C" fn signal_vsync_hook() {
    let frame = VSYNC_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if !LOG_VSYNC_FRAMES.load(Ordering::Relaxed) {
        return;
    }
    if let Some(host) = HOST.get() {
        host.log_line(
            c"android_base",
            &format!(
                "signal_vsync: frame {frame} signalled (Choreographer::signalAllVSync \
                 analogue, atomic frame counter — ticket 77)"
            ),
        );
    }
}

/// Register the production Android runtime hooks through the existing
/// `register_android_hooks` C ABI (ticket 77) — the reference android_base
/// registers the same pair inside its register-JNI phase and fails that
/// phase if the registration is refused (android_base.cpp:58-66). The
/// registry keeps its first-write-wins semantics unchanged; a rejection is
/// only reachable on an in-process re-bring-up (android_base is the
/// phase-first registrar in every valid kit, so the prior owner is this same
/// pinned library) and is logged instead of fatal — the C++ host has no
/// in-process re-bring-up, so its fatal arm has no Rust-side counterpart
/// without breaking the runtime-registry suites that bring a kit up twice.
fn register_android_runtime_hooks(api: &rudroid_common::host_api::HostApiV0) {
    // Read configuration once during registration, never in the frame hook.
    let log_frames = api.config_bool(c"unity.log_vsync_frames", false);
    LOG_VSYNC_FRAMES.store(log_frames, Ordering::Relaxed);
    let hooks = RudroidAndroidHooks {
        flush_preferences: Some(flush_preferences_hook),
        signal_vsync: Some(signal_vsync_hook),
    };
    // The host's only rejection code is REGISTRY_REJECTED (1).
    let message = if api.register_hooks(&hooks) {
        "android runtime hooks registered: flush_preferences + signal_vsync \
         (reference android_base parity — ticket 77)"
    } else {
        "android runtime hooks already registered — keeping the first \
         registration (registry first-write-wins, code 1; ticket 77)"
    };
    api.log_line(c"android_base", message);
}



rudroid_common::declare_descriptor!("android_base", rudroid_common::Kind::AndroidBase);

/// Deployed-kit behaviour of ticket 45: the register-JNI phase queries the catalog
/// provider through the Host API registry — proving the query happens across
/// the dynamic boundary (the provider lives in `reply_catalog`'s library),
/// not through any static link. Ticket 52: the phase additionally joins the
/// shared JNI session registered by `android_jni_vm` — same `JavaVM*`, same
/// reference lifecycle as the guest — and pulls the catalog's known value
/// through the JNI function table. A kit without the session capability
/// degrades to a logged skip; a session that fails mid-join fails the phase.
#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(
    api: Option<&'static rudroid_common::host_api::HostApiV0>,
) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    // The production hooks log through this table; install it before any
    // hook can fire (the registration below is the earliest fire point).
    let _ = HOST.set(*api);
    let (hit, value) = api
        .catalog_int(c"android/media/AudioManager", c"getStreamVolume", c"(I)I")
        .map_or((0, -1), |value| (1, value));
    api.log_line(c"android_base", &format!("catalog getStreamVolume hit={hit} value={value}"));

    // Ticket 77: the reference android_base registers the Android runtime
    // hooks inside this phase (before any session work); the profile-prepare
    // gate refuses to run without them.
    register_android_runtime_hooks(api);

    join_shared_session(api)
}

/// Join the shared JNI session (ticket 52): resolve the same fixed reply
/// through the JNI tables the guest sees, keep a global class reference as
/// this library's session anchor, and log the provenance. Missing session:
/// explicit logged skip (the registry degrades the same way for the catalog
/// provider); broken session: non-zero phase failure.
fn join_shared_session(api: &rudroid_common::host_api::HostApiV0) -> c_int {
    let Some(vm) = api.java_vm() else {
        api.log_line(
            c"android_base",
            "no JNI session registered — session join skipped \
             (deploy the android_jni_vm capability, ticket 52)",
        );
        return 0;
    };
    let log = |message: String| api.log_line(c"android_base", &message);
    let fail = |message: String| -> c_int {
        log(format!("session join FAILED: {message} (ticket 52)"));
        -3
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return fail("GetEnv on the shared VM did not yield a JNIEnv".to_owned());
    };
    let class_name = c"android/media/AudioManager";
    let local = env.find_class(class_name);
    if local.is_null() {
        return fail("FindClass(android/media/AudioManager) returned null".to_owned());
    }
    let global = env.new_global_ref(local);
    env.delete_local_ref(local);
    if global.is_null() {
        return fail("NewGlobalRef on the framework class returned null".to_owned());
    }
    // `global` is deliberately never deleted: this library's anchor in
    // the shared session's reference tables (process-pinned, ticket 46).
    let Some(method) = env.static_method(global, c"getStreamVolume", c"(I)I") else {
        return fail("GetStaticMethodID(getStreamVolume) returned null".to_owned());
    };
    let volume = env.call_static_int(global, method, &[Arg::int(3)]);
    if volume != 100 {
        return fail(format!(
            "CallStaticIntMethodA(getStreamVolume) returned {volume}, want the \
             catalog fixed reply 100"
        ));
    }
    if java::register(env) != 0 {
        return fail("Android Java method registration failed".to_owned());
    }
    log(format!(
        "session join ok volume={volume} (same JavaVM as the guest, \
         global class ref held — ticket 52)"
    ));
    0
}

/// Test-only fake JNI tables for the session join: a VM table whose GetEnv
/// yields an env table with the join's slots filled by recording shims, so
/// the join logic is exercised without linking the `android_jni_vm`
/// capability (ticket 43; the real pair runs in the ticket-52 integration
/// test).
#[cfg(test)]
#[path = "../tests/unit/lib_jni_fake.rs"]
mod jni_fake;

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
