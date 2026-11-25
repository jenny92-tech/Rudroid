//! INPUT: an optional actual Application published through the shared JNI VM
//! OUTPUT: one rooted process Application for Context and ActivityThread
//! POS: first-wins identity owner; an escaped generic fallback cannot be replaced
#![forbid(unsafe_code)]

use std::ffi::CStr;
use std::sync::Mutex;
use rudroid_common::jni_env::{JniEnv, JObject, Native};

// Exactly one framework capability owns this process-pinned global. Its handle
// never changes or retires after publication, so promotion/comparison can run
// outside the mutex without racing a final global deletion.
struct ApplicationState { global: usize, explicit_pending: bool }
static APPLICATION: Mutex<ApplicationState> = Mutex::new(ApplicationState { global: 0, explicit_pending: false });

pub(super) fn current(env: JniEnv) -> JObject {
    let (current, pending) = {
        let state = APPLICATION.lock().unwrap();
        (state.global, state.explicit_pending)
    };
    if current != 0 { return env.new_local_ref(current as JObject); }
    // An explicit Application is still being created. Android queries cannot
    // return an object that does not exist yet, or invent a second identity.
    if pending { return std::ptr::null_mut(); }
    let local = super::java::allocate(env, c"android/app/Application");
    if local.is_null() || env.exception_check() {
        env.delete_local_ref(local);
        return std::ptr::null_mut();
    }
    let candidate = env.new_global_ref(local);
    env.delete_local_ref(local);
    if candidate.is_null() || env.exception_check() {
        env.delete_global_ref(candidate);
        return std::ptr::null_mut();
    }
    let winner = {
        let mut state = APPLICATION.lock().unwrap();
        if state.global == 0 && !state.explicit_pending { state.global = candidate as usize; }
        state.global
    };
    if winner != candidate as usize { env.delete_global_ref(candidate); }
    if winner == 0 { std::ptr::null_mut() } else { env.new_local_ref(winner as JObject) }
}

fn reject(env: JniEnv, class: &CStr, message: &CStr) -> u8 {
    if !env.exception_check() {
        let class = env.find_class(class);
        env.throw_new(class, message);
        env.delete_local_ref(class);
    }
    0
}

extern "C" fn reserve(env: JniEnv, _class: JObject) -> u8 {
    if env.exception_check() { return 0; }
    let accepted = {
        let mut state = APPLICATION.lock().unwrap();
        if state.global == 0 { state.explicit_pending = true; true } else { false }
    };
    if accepted { 1 } else {
        reject(env, c"java/lang/IllegalStateException", c"A process Application was already published or queried before reservation")
    }
}

extern "C" fn publish(env: JniEnv, _class: JObject, application: JObject) -> u8 {
    if env.exception_check() { return 0; }
    if application.is_null() {
        return reject(env, c"java/lang/NullPointerException", c"Application owner is null");
    }
    let class = env.find_class(c"android/app/Application");
    let valid = env.is_instance_of(application, class);
    env.delete_local_ref(class);
    if env.exception_check() { return 0; }
    if !valid {
        return reject(env, c"java/lang/ClassCastException", c"Application owner must extend android.app.Application");
    }
    let candidate = env.new_global_ref(application);
    if candidate.is_null() || env.exception_check() {
        env.delete_global_ref(candidate);
        return 0;
    }
    let existing = {
        let mut state = APPLICATION.lock().unwrap();
        if state.global == 0 {
            state.global = candidate as usize;
            state.explicit_pending = false;
            return 1;
        }
        state.global
    };
    let same = env.is_same_object(existing as JObject, application);
    env.delete_global_ref(candidate);
    if env.exception_check() { return 0; }
    if same { return 1; }
    // A fallback may already have escaped to native code. Replacing it would
    // leave two live Applications, so reject even during startup.
    reject(env, c"java/lang/IllegalStateException", c"A different process Application was already published or queried")
}

pub(super) fn register(env: JniEnv) -> i32 {
    env.register_class(c"org/rudroid/runtime/ApplicationOwner", &[
        Native::new0(c"reserve", c"()Z", reserve),
        Native::new1(c"publish", c"(Landroid/app/Application;)Z", publish),
    ])
}
