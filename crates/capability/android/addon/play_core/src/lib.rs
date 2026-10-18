//! INPUT:  shared JNI VM and local Play Core Task/review-manager construction requests
//! OUTPUT:  executable listener jobs and an offline ReviewManager object; registration outcome
//! POS:    optional Play Core facade using common task snapshots and the framework main Looper
#![deny(unsafe_code)]
use std::ffi::{c_int, CString};
use std::sync::Mutex;

use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::{JObject, JavaVm, JniEnv, JNI_VERSION_1_6};

rudroid_common::declare_descriptor!(
    "sdk_google_play_core",
    rudroid_common::Kind::AndroidAddon,
    package = None::<&str>,
    requires = Some("android_base")
);

/// Interfaces the provider owns (oracle google_play_core.h: the play-core
/// tasks namespace, NOT the GMS one — fixed in ticket 56).
pub const INTERFACES: [&str; 3] = [
    "com/google/android/play/core/tasks/OnCompleteListener",
    "com/google/android/play/core/tasks/OnSuccessListener",
    "com/google/android/play/core/tasks/OnFailureListener",
];

/// The play-core task class carrying the listener registration methods.
pub const TASK_CLASS: &str = "com/google/android/play/core/tasks/Task";
/// Oracle Task surface (google_play_core.cpp descriptors): the listener
/// registration methods, backed by the shared VM-owned callback queue.
pub const ADD_ON_COMPLETE_LISTENER: (&str, &str) = (
    "addOnCompleteListener",
    "(Lcom/google/android/play/core/tasks/OnCompleteListener;)Lcom/google/android/play/core/tasks/Task;",
);
/// The offline ReviewManagerFactory has an executable local object constructor.
pub const REVIEW_MANAGER_FACTORY_CLASS: &str =
    "com/google/android/play/core/review/ReviewManagerFactory";

/// Play Core asset-pack states served as documented constants (oracle scope;
/// the v1 catalog has no asset-pack entries).
pub const ASSET_PACK_STATUS_ACTIVE: i64 = 3;
pub const ASSET_PACK_STATUS_COMPLETED: i64 = 4;

/// Registry entry shape (provider id plus owned interfaces).
pub fn provider_registration() -> (&'static str, [&'static str; 3]) {
    ("sdk_google_play_core", INTERFACES)
}

/// The observed shared-session registration outcome (GMS parity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    Pending,
    /// No JNI session registered — logged skip (framework parity).
    NoSession,
    /// The owned surface resolved through the shared session; anchored.
    Joined,
    Failed {
        reason: &'static str,
    },
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

pub fn register_jni(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let Some(vm) = api.java_vm() else {
        set_outcome(RegistrationOutcome::NoSession);
        api.log_line(
            c"PLAYCORE",
            "no JNI session registered — registration skip (deploy \
             android_jni_vm, ticket 52)",
        );
        return 0;
    };
    let join = join_shared_session(api, vm);
    match join {
        Ok(()) => {
            set_outcome(RegistrationOutcome::Joined);
            api.log_line(
                c"PLAYCORE",
                "Google Play Core JNI/proxy plugin registered (play-core tasks namespace)",
            );
            0
        }
        Err(reason) => {
            set_outcome(RegistrationOutcome::Failed { reason });
            api.log_line(
                c"PLAYCORE",
                &format!("session registration FAILED: {reason}"),
            );
            -3
        }
    }
}

fn join_shared_session(api: &HostApiV0, vm: JavaVm) -> Result<(), &'static str> {
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return Err("GetEnv failed on the shared session");
    };
    join_with_env(api, env)
}

extern "C" fn review_manager(env: JniEnv, _class: JObject, _context: JObject) -> JObject {
    let class = env.find_class(c"com/google/android/play/core/review/ReviewManager");
    let object = env.alloc_object(class);
    env.delete_local_ref(class);
    object
}
fn join_with_env(api: &HostApiV0, env: JniEnv) -> Result<(), &'static str> {
    if rudroid_common::jni_tasks::register(env, rudroid_common::jni_tasks::Namespace::PlayCore)
        != 0
    {
        return Err("Play Core task native registration failed");
    }
    if env.register_class(
        c"com/google/android/play/core/review/ReviewManagerFactory",
        &[rudroid_common::jni_env::Native::new1(
            c"create",
            c"(Landroid/content/Context;)Lcom/google/android/play/core/review/ReviewManager;",
            review_manager,
        )],
    ) != 0
    {
        return Err("review-manager factory registration failed");
    }
    let _ = api;
    let name = |value: &str| CString::new(value).map_err(|_| "class name contained NUL");
    let resolve_class = |env: JniEnv, class: &str| -> Result<JObject, &'static str> {
        let c_name = name(class)?;
        let handle = env.find_class(&c_name);
        if handle.is_null() {
            if env.exception_check() {
                env.exception_clear();
            }
            return Err("an owned play-core class did not resolve");
        }
        Ok(handle)
    };

    for interface in INTERFACES {
        let class = resolve_class(env, interface)?;
        env.delete_local_ref(class);
    }
    let task = resolve_class(env, TASK_CLASS)?;
    let factory = resolve_class(env, REVIEW_MANAGER_FACTORY_CLASS)?;
    env.delete_local_ref(factory);

    // Resolve the executable native registered above; callback behavior is tested separately.
    let (method, sig) = ADD_ON_COMPLETE_LISTENER;
    let c_method = name(method)?;
    let c_sig = name(sig)?;
    let method_id = env.get_method_id(task, &c_method, &c_sig);
    if method_id.is_null() {
        if env.exception_check() {
            env.exception_clear();
        }
        return Err("Task.addOnCompleteListener did not resolve against the catalog");
    }

    // The session anchor (the framework join's parity act).
    let anchor = env.new_global_ref(task);
    env.delete_local_ref(task);
    if anchor.is_null() {
        return Err("NewGlobalRef anchor failed");
    }
    Ok(())
}

#[allow(unsafe_code)] // Export declaration only; native bodies use typed common methods.
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    register_jni(api)
}

#[allow(unsafe_code)] // Export declaration only; native bodies use typed common methods.
#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
