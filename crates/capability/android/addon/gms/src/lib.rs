//! rudroid-android-addon-gms: GMS Android Addon (ticket 17; shared-session
//! registration ticket 56).
//!
//! Rust counterpart of the read-only `sdk_google_gms` plugin. Ticket 56
//! replaces the static registration data with the reference behavior: the
//! register-JNI phase joins the SHARED JNI session (ticket 52's session/registry
//! path — the same `JavaVM*`/`JNIEnv` every library and the guest see), resolves
//! the owned task-listener interfaces plus `Task`/`AuthenticationResult`, pulls
//! the registered fixed replies through the JNI function table
//! (`Task.isSuccessful == true`, `AuthenticationResult.isAuthenticated == false`
//! — oracle google_gms.cpp values) and anchors a global class reference. The
//! registration is therefore visible to any shared-session query (the guest
//! included), not only to this crate's static return values.
//!
//! Load order: after `android_base` (declared via `requires`), documented in
//! `crates/docs/MIGRATION-MAP.md`. The C++ proxy-provider surface (`register_java_proxy_
//! provider`) has no HostApiV0 v1 slot — recorded gap, the class/method
//! registration above is the v1 minimal registration.

mod java;
use std::ffi::{c_int, CString};
use std::sync::Mutex;

use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::{JavaVm, JniEnv, JNI_VERSION_1_6};

rudroid_common::declare_descriptor!(
    "sdk_google_gms",
    rudroid_common::Kind::AndroidAddon,
    package = None::<&str>,
    requires = Some("android_base")
);

/// Interfaces the provider owns (C++ `kInterfaces`).
pub const INTERFACES: [&str; 3] = [
    "com/google/android/gms/tasks/OnCompleteListener",
    "com/google/android/gms/tasks/OnSuccessListener",
    "com/google/android/gms/tasks/OnFailureListener",
];

/// The class carrying the registered completion semantics (oracle: `Task`).
pub const TASK_CLASS: &str = "com/google/android/gms/tasks/Task";
/// Oracle `tasks::Task::isSuccessful()` — returns `true`.
pub const IS_SUCCESSFUL: (&str, &str) = ("isSuccessful", "()Z");
/// The games surface the oracle serves: `AuthenticationResult` —
/// `isAuthenticated()` returns `false` (verbose "-> false").
pub const AUTHENTICATION_RESULT_CLASS: &str =
    "com/google/android/gms/games/AuthenticationResult";
pub const IS_AUTHENTICATED: (&str, &str) = ("isAuthenticated", "()Z");

/// One proxy invocation handed to the runtime (kept for the proxy-provider
/// follow-up: HostApiV0 v1 has no `register_java_proxy_provider` slot).
#[derive(Debug, Clone, PartialEq)]
pub struct ProxyInvocation {
    pub interface_name: &'static str,
    pub method_name: &'static str,
    /// Guest-side handle the invocation belongs to.
    pub native_handle: i64,
}

/// Build the completion invocation the C++ plugin fires when the guest task
/// finishes (`onComplete` on `OnCompleteListener`).
pub fn complete_invocation(native_handle: i64) -> ProxyInvocation {
    ProxyInvocation {
        interface_name: INTERFACES[0],
        method_name: "onComplete",
        native_handle,
    }
}

/// Registry entry shape (provider id plus owned interfaces).
pub fn provider_registration() -> (&'static str, [&'static str; 3]) {
    ("sdk_google_gms", INTERFACES)
}

/// The observed shared-session registration outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// register-JNI has not run (or ran without a session: the framework
    /// parity contract degrades to an explicit logged skip).
    Pending,
    /// No JNI session was registered in the runtime registry — logged skip.
    NoSession,
    /// The owned surface resolved and answered through the shared session;
    /// a global class anchor was taken.
    Joined,
    /// The session rejected the registration (missing table slot, failed
    /// method resolution, or an unexpected fixed reply) — named failure.
    Failed { reason: &'static str },
}

static OUTCOME: Mutex<Option<RegistrationOutcome>> = Mutex::new(None);

fn set_outcome(outcome: RegistrationOutcome) {
    *OUTCOME.lock().unwrap() = Some(outcome);
}

/// The outcome of the last register-JNI run (`None` before it ran).
pub fn last_outcome() -> Option<RegistrationOutcome> {
    OUTCOME.lock().unwrap().clone()
}

#[doc(hidden)]
pub fn reset_state_for_tests() {
    *OUTCOME.lock().unwrap() = None;
}

/// Behavior contract in the crate docs (ticket 56).
pub fn register_jni(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let Some(vm) = api.java_vm() else {
        set_outcome(RegistrationOutcome::NoSession);
        api.log_line(
            c"GMS",
            "no JNI session registered — registration skip (deploy \
             android_jni_vm, ticket 52)",
        );
        return 0;
    };
    let join = join_shared_session(api, vm);
    match join {
        Ok(()) => {
            set_outcome(RegistrationOutcome::Joined);
            api.log_line(c"GMS", "Google GMS JNI/proxy plugin registered");
            0
        }
        Err(reason) => {
            set_outcome(RegistrationOutcome::Failed { reason });
            api.log_line(c"GMS", &format!("session registration FAILED: {reason}"));
            -3
        }
    }
}

/// The shared-session join: GetEnv → FindClass(owned surface) → method
/// resolution → the oracle fixed replies → global class anchor.
fn join_shared_session(api: &HostApiV0, vm: JavaVm) -> Result<(), &'static str> {
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return Err("GetEnv failed on the shared session");
    };
    join_with_env(api, env)
}

fn join_with_env(api: &HostApiV0, env: JniEnv) -> Result<(), &'static str> {
    let _ = api;
    let name = |value: &str| CString::new(value).map_err(|_| "class name contained NUL");

    // The owned listener interfaces must resolve (session-wide class table).
    for interface in INTERFACES {
        let c_name = name(interface)?;
        let class = env.find_class(&c_name);
        if class.is_null() {
            if env.exception_check() {
                env.exception_clear();
            }
            return Err("an owned interface did not resolve");
        }
    }

    // Task.isSuccessful ()Z == true (oracle) — the registered method surface
    // answers through the shared session's catalog-backed resolution.
    let task_class = name(TASK_CLASS)?;
    let task = env.find_class(&task_class);
    if task.is_null() {
        if env.exception_check() {
            env.exception_clear();
        }
        return Err("Task did not resolve");
    }
    let (method, sig) = IS_SUCCESSFUL;
    let c_method = name(method)?;
    let c_sig = name(sig)?;
    let Some(method_id) = env.static_method(task, &c_method, &c_sig) else {
        if env.exception_check() {
            env.exception_clear();
        }
        return Err("Task.isSuccessful did not resolve against the catalog");
    };
    let successful = env.call_static_boolean(task, method_id, &[]);
    if !successful {
        return Err("Task.isSuccessful did not answer the oracle value true");
    }

    // AuthenticationResult.isAuthenticated ()Z == false (oracle).
    let auth_class = name(AUTHENTICATION_RESULT_CLASS)?;
    let auth = env.find_class(&auth_class);
    if auth.is_null() {
        if env.exception_check() {
            env.exception_clear();
        }
        return Err("AuthenticationResult did not resolve");
    }
    let (method, sig) = IS_AUTHENTICATED;
    let c_method = name(method)?;
    let c_sig = name(sig)?;
    let Some(method_id) = env.static_method(auth, &c_method, &c_sig) else {
        if env.exception_check() {
            env.exception_clear();
        }
        return Err("AuthenticationResult.isAuthenticated did not resolve");
    };
    let authenticated = env.call_static_boolean(auth, method_id, &[]);
    if authenticated {
        return Err("AuthenticationResult.isAuthenticated did not answer false");
    }

    // The session anchor: a global class reference (the framework join's
    // parity act — it lives in the session-wide reference table).
    let anchor = env.new_global_ref(task);
    if anchor.is_null() {
        return Err("NewGlobalRef anchor failed");
    }
    Ok(())
}

#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let Some(vm) = api.java_vm() else {
        return register_jni(api);
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return -3;
    };
    let result = java::register(env);
    if result != 0 {
        return result;
    }
    register_jni(api)
}

/// Main init: the Host API version check (registration happens in the
/// register-JNI phase).
pub fn plugin_init(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    0
}

#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    plugin_init(api)
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
