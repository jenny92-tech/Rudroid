//! INPUT: Java frame callbacks, configured refresh rate and the shared Handler scheduler
//! OUTPUT: one-shot frame callbacks on their owner, monotonic timestamps and cancellation
//! POS: Android frame-clock façade; VM fields own callback graphs, Rust owns scheduling
#![forbid(unsafe_code)]

use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static INSTANCES: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
static TASKS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static EPOCH: OnceLock<Instant> = OnceLock::new();
static INTERVAL: OnceLock<u64> = OnceLock::new();
static OWNER: OnceLock<usize> = OnceLock::new();
static HANDLER: OnceLock<usize> = OnceLock::new();
static CALLBACK: OnceLock<usize> = OnceLock::new();
static CANCELLED: OnceLock<usize> = OnceLock::new();
fn get(env: JniEnv, object: *mut c_void, field: &OnceLock<usize>) -> *mut c_void {
    env.get_object_field(object, *field.get().unwrap() as *mut c_void)
}
fn set(env: JniEnv, object: *mut c_void, field: &OnceLock<usize>, value: *mut c_void) {
    env.set_object_field(object, *field.get().unwrap() as *mut c_void, value)
}
fn weak(env: JniEnv, object: *mut c_void) -> usize {
    env.new_weak_global_ref(object) as usize
}
fn local(env: JniEnv, reference: usize) -> *mut c_void {
    env.new_local_ref(reference as *mut c_void)
}
fn delete_weak(env: JniEnv, reference: usize) {
    env.delete_weak_global_ref(reference as *mut c_void)
}
fn pending(env: JniEnv) -> bool {
    env.exception_check()
}
extern "C" fn instance(env: JniEnv, _class: JObject) -> JObject {
    let mut instances = INSTANCES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    // Dead owners retain neither a Java object nor a Rust worker. Retire
    // cleared weak cache entries whenever a new caller asks for its clock.
    instances.retain(|_, reference| {
        let object = local(env, *reference);
        if object.is_null() {
            if pending(env) {
                true
            } else {
                delete_weak(env, *reference);
                false
            }
        } else {
            super::preferences::delete(env, object);
            true
        }
    });
    if pending(env) {
        return std::ptr::null_mut();
    }
    let id = super::threads::current_id();
    if let Some(reference) = instances.get(&id) {
        return local(env, *reference);
    }
    let object = super::java::allocate(env, c"android/view/Choreographer");
    if object.is_null() {
        return object;
    }
    let handler = super::threads::current_handler(env);
    set(env, object, &HANDLER, handler);
    super::preferences::delete(env, handler);
    let reference = weak(env, object);
    if reference == 0 || pending(env) {
        super::preferences::delete(env, object);
        return std::ptr::null_mut();
    }
    instances.insert(id, reference);
    object
}
fn delay_ms() -> i64 {
    let interval = *INTERVAL.get_or_init(|| {
        let rate = super::HOST.get().unwrap().config_f64(c"device.displayRefreshRate", 60.0);
        let rate = if rate.is_finite() && rate > 1.0 {
            rate.min(1000.0)
        } else {
            60.0
        };
        (1_000_000_000.0 / rate) as u64
    });
    let elapsed = EPOCH.get_or_init(Instant::now).elapsed().as_nanos();
    let remaining = interval as u128 - elapsed % interval as u128;
    remaining.div_ceil(1_000_000) as i64
}
extern "C" fn frame_time(_env: JniEnv, _object: JObject) -> i64 {
    rudroid_common::clock::monotonic_nanos()
}

fn forget_task(env: JniEnv, task: *mut c_void) {
    let mut tasks = TASKS.lock().unwrap();
    tasks.retain(|reference| {
        let remove = env.is_same_object(*reference as *mut c_void, task)
            || env.is_same_object(*reference as *mut c_void, std::ptr::null_mut());
        if remove {
            delete_weak(env, *reference);
        }
        !remove
    });
}
extern "C" fn post(env: JniEnv, owner: JObject, callback: JObject) {
    if callback.is_null() {
        super::preferences::error(env, "frame callback cannot be null");
        return;
    }
    forget_task(env, std::ptr::null_mut());
    let task = super::java::allocate(env, c"org/rudroid/runtime/FrameTask");
    if task.is_null() {
        return;
    }
    set(env, task, &OWNER, owner);
    set(env, task, &CALLBACK, callback);
    let reference = weak(env, task);
    if reference == 0 || pending(env) {
        super::preferences::delete(env, task);
        return;
    }
    {
        let mut tasks = TASKS.lock().unwrap();
        if tasks.try_reserve(1).is_err() {
            delete_weak(env, reference);
            super::preferences::delete(env, task);
            super::preferences::error(env, "frame callback queue allocation failed");
            return;
        }
        tasks.push(reference);
    }
    let handler = get(env, owner, &HANDLER);
    if super::java::post_delayed(env, handler, task, delay_ms()) == 0 {
        forget_task(env, task);
    }
    super::preferences::delete(env, handler);
    super::preferences::delete(env, task);
}
extern "C" fn remove(env: JniEnv, owner: JObject, callback: JObject) {
    forget_task(env, std::ptr::null_mut());
    let tasks = TASKS.lock().unwrap();
    for reference in tasks.iter() {
        let task = local(env, *reference);
        if task.is_null() {
            continue;
        }
        let task_owner = get(env, task, &OWNER);
        let task_callback = get(env, task, &CALLBACK);
        if env.is_same_object(owner, task_owner) && env.is_same_object(callback, task_callback) {
            env.set_boolean_field(task, *CANCELLED.get().unwrap() as *mut c_void, 1);
        }
        for object in [task_callback, task_owner, task] {
            super::preferences::delete(env, object);
        }
    }
}
extern "C" fn run(env: JniEnv, task: JObject) {
    forget_task(env, task);
    if env.get_boolean_field(task, *CANCELLED.get().unwrap() as *mut c_void) != 0 {
        return;
    }
    let callback = get(env, task, &CALLBACK);
    let class = env.get_object_class(callback);
    if let Some(method) = env.method(class, c"doFrame", c"(J)V") {
        env.call_void(callback, method, &[Arg::long(frame_time(env, task))]);
    }
    super::preferences::delete(env, class);
    super::preferences::delete(env, callback);
}
extern "C" fn default_frame(_env: JniEnv, _object: JObject, _nanos: i64) {}
pub(super) fn register(env: JniEnv) -> i32 {
    if declare_superclass(
        env,
        c"org/rudroid/runtime/FrameTask",
        c"java/lang/Runnable",
    ) != 0
    {
        return -1;
    }
    for (class, name, sig, cache) in [
        (
            c"android/view/Choreographer",
            c"handler",
            c"Landroid/os/Handler;",
            &HANDLER,
        ),
        (
            c"org/rudroid/runtime/FrameTask",
            c"owner",
            c"Landroid/view/Choreographer;",
            &OWNER,
        ),
        (
            c"org/rudroid/runtime/FrameTask",
            c"callback",
            c"Landroid/view/Choreographer$FrameCallback;",
            &CALLBACK,
        ),
        (
            c"org/rudroid/runtime/FrameTask",
            c"cancelled",
            c"Z",
            &CANCELLED,
        ),
    ] {
        if declare_field(env, class, name, sig, false) != 0 {
            return -1;
        }
        let class = env.find_class(class);
        let field = env.get_field_id(class, name, sig);
        super::preferences::delete(env, class);
        if field.is_null() {
            return -1;
        }
        cache.get_or_init(|| field as usize);
    }
    for (class, methods) in [
        (
            c"android/view/Choreographer",
            vec![
                Native::new0(c"getInstance", c"()Landroid/view/Choreographer;", instance),
                Native::new1(
                    c"postFrameCallback",
                    c"(Landroid/view/Choreographer$FrameCallback;)V",
                    post
                ),
                Native::new1(
                    c"removeFrameCallback",
                    c"(Landroid/view/Choreographer$FrameCallback;)V",
                    remove
                ),
            ],
        ),
        (
            c"android/view/Choreographer$FrameCallback",
            vec![Native::new1(c"doFrame", c"(J)V", default_frame)],
        ),
        (
            c"org/rudroid/runtime/FrameTask",
            vec![Native::new0(c"run", c"()V", run)],
        ),
        (
            c"java/lang/System",
            vec![Native::new0(c"nanoTime", c"()J", frame_time)],
        ),
    ] {
        if env.register_class(class, &methods) != 0 {
            return -1;
        }
    }
    0
}
