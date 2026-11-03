//! INPUT:  pre-native quiesce hooks, post-native joins and shared JNI references
//! OUTPUT:  bounded Lifecycle registration, checked preference flush, shutdown and finish phases
//! POS:    preserves native-owner drain/stop protocols before final service and core joins
#![forbid(unsafe_code)]
use rudroid_common::jni_env::{JObject, JniEnv, Native};
use std::{ffi::c_void, sync::Mutex};
struct State {
    hooks: Vec<usize>,
    after_native: Vec<usize>,
    closed: bool,
}
static STATE: Mutex<State> = Mutex::new(State {
    hooks: Vec::new(),
    after_native: Vec::new(),
    closed: false,
});
extern "C" fn add(env: JniEnv, _class: JObject, runnable: JObject) {
    add_hook(env, runnable, false);
}
extern "C" fn add_after_native(env: JniEnv, _class: JObject, runnable: JObject) {
    add_hook(env, runnable, true);
}
fn add_hook(env: JniEnv, runnable: JObject, after_native: bool) {
    if runnable.is_null() {
        super::preferences::error(env, "shutdown hook must be a Runnable");
        return;
    }
    let mut state = STATE.lock().unwrap();
    if state.closed || state.hooks.len() + state.after_native.len() >= 32 {
        super::preferences::error(env, "shutdown hook registry is closed or full");
        return;
    }
    let root = env.new_global_ref(runnable);
    if !root.is_null() {
        if after_native { state.after_native.push(root as usize); } else { state.hooks.push(root as usize); }
    }
}
extern "C" fn shutdown(env: JniEnv, _class: JObject) {
    let hooks = {
        let mut state = STATE.lock().unwrap();
        state.closed = true;
        std::mem::take(&mut state.hooks)
    };
    let first = run_hooks(env, hooks);
    super::java::discard_ui_jobs(env);
    super::input::shutdown(env);
    // Core queues and native-owned audio producers remain available to
    // nativeDone. Their native owner must stop/join callbacks before freeing
    // its state; finish provides final joins and partial-init cleanup.
    restore_error(env, first);
}
fn run_hooks(env: JniEnv, hooks: Vec<usize>) -> JObject {
    let mut first = env.exception_occurred();
    env.exception_clear();
    for hook in hooks.into_iter().rev() {
        let hook = hook as *mut c_void;
        let class = env.get_object_class(hook);
        if let Some(method) = env.method(class, c"run", c"()V") {
            env.call_void(hook, method, &[]);
        }
        env.delete_local_ref(class);
        env.delete_global_ref(hook);
        let error = env.exception_occurred();
        env.exception_clear();
        if first.is_null() {
            first = error;
        } else {
            env.delete_local_ref(error);
        }
    }
    first
}
fn restore_error(env: JniEnv, first: JObject) {
    if !first.is_null() {
        env.throw(first);
        env.delete_local_ref(first);
    }
}
extern "C" fn finish(env: JniEnv, _class: JObject) {
    let needs_shutdown = !STATE.lock().unwrap().closed;
    if needs_shutdown { shutdown(env, _class); }
    let hooks = {
        let mut state = STATE.lock().unwrap();
        std::mem::take(&mut state.after_native)
    };
    // Keep an earlier hook exception out of the way while joining every
    // producer/core worker. Restore it after all cleanup has actually run.
    let first = run_hooks(env, hooks);
    super::threads::shutdown(env);
    super::java::finish_ui_jobs(env);
    super::preferences::flush();
    restore_error(env, first);
}
extern "C" fn flush_preferences(_env: JniEnv, _class: JObject) -> u8 {
    u8::from(super::preferences::flush_checked())
}
pub(super) fn register(env: JniEnv) -> i32 {
    env.register_class(
        c"org/rudroid/runtime/Lifecycle",
        &[
            Native::new1(c"registerShutdown", c"(Ljava/lang/Runnable;)V", add),
            Native::new1(c"registerAfterNativeShutdown", c"(Ljava/lang/Runnable;)V", add_after_native),
            Native::new0(c"shutdown", c"()V", shutdown),
            Native::new0(c"finish", c"()V", finish),
            Native::new0(c"flushPreferences", c"()Z", flush_preferences),
        ],
    )
}
