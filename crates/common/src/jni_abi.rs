//! INPUT: a `jni_env::JniEnv` of the shared session, JNI table slots and scalar argument values
//! OUTPUT: pure JNI slot reader, C-layout jvalue/native-method types, engine-neutral lifecycle shutdown and safe session helpers on `JniEnv`
//! POS: shared ABI definitions without session state or plugin exports; only `function_slot` reads slots by number
//! Pure JNI ABI helper shared by capabilities that call through the shared
//! session's `JavaVM*` / `JNIEnv*` (no plugin exports, no session state).
//!
//! JNI hands out handles, not tables: a `JavaVM*` points at a
//! `JNIInvokeInterface*` and a `JNIEnv*` points at a `JNINativeInterface*`,
//! so reading slot `index` is a double dereference — handle → function table
//! → entry. Slot numbers follow the JNI 1.6 spec order (the invoke interface
//! starts with three reserved slots, the native interface with four).

use std::ffi::{c_void, CStr};

pub use crate::jni_env::{declare_class, function_slot, JValue, NativeMethod};
use crate::jni_env::{Arg, JniEnv};

/// Install a complete, already verified application class-name catalog. The
/// VM then resolves queries from this catalog plus explicitly declared host
/// classes. Bodies, constructors and class-loader execution are not supplied.
pub fn install_class_catalog(env: JniEnv, names: &[String]) -> bool {
    let Ok(count) = i32::try_from(names.len()) else { return false; };
    let types = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let method = env.static_method(types, c"installClassCatalog", c"([Ljava/lang/String;)Z");
    let string = env.find_class(c"java/lang/String");
    let array = env.new_object_array(count, string, std::ptr::null_mut());
    let mut valid = method.is_some() && !array.is_null();
    if valid {
        for (index, name) in names.iter().enumerate() {
            let text = env.new_string(&name.encode_utf16().collect::<Vec<_>>());
            env.set_object_array_element(array, index as i32, text);
            env.delete_local_ref(text);
            if env.exception_check() { valid = false; break; }
        }
    }
    let result = valid && method.is_some_and(|method|
        env.call_static_boolean(types, method, &[Arg::object(array)]));
    for local in [array, string, types] { env.delete_local_ref(local); }
    result && !env.exception_check()
}

/// Declare a Java field in the shared VM using the internal metadata service.
pub fn declare_field(
    env: JniEnv,
    class_name: &CStr,
    name: &CStr,
    signature: &CStr,
    is_static: bool,
) -> i32 {
    let types = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let class = declare_class(env, class_name);
    let method = env.static_method(
        types,
        c"defineField",
        c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)V",
    );
    let name = env.new_string_utf(name);
    let signature = env.new_string_utf(signature);
    let handles = [types, class, name, signature];
    let valid = method.is_some() && handles.iter().all(|h| !h.is_null());
    if let (true, Some(method)) = (valid, method) {
        env.call_static_void(
            types,
            method,
            &[
                Arg::object(class),
                Arg::object(name),
                Arg::object(signature),
                Arg::bool(is_static),
            ],
        );
    }
    let failed = !valid || env.exception_check();
    for handle in handles {
        if !handle.is_null() {
            env.delete_local_ref(handle);
        }
    }
    if failed {
        -1
    } else {
        0
    }
}



/// Forward an SDK interface callback through the engine's generic Java proxy
/// service. All temporary metadata/argument handles stay in one local frame.
/// No plugin-owned object pointer or private HostApi slot crosses this boundary.
pub fn invoke_proxy_callback(
    env: JniEnv,
    receiver: *mut c_void,
    interface: &CStr,
    name: &CStr,
    signature: &CStr,
    argument: *mut c_void,
) {
    invoke_proxy_arguments(
        env,
        receiver,
        interface,
        name,
        signature,
        &[Arg::object(argument)],
        &[argument],
    );
}

/// Forward exact JNI arguments, with boxed equivalents for engine proxies.
/// `raw` must match the descriptor's parameter count (checked by the call).
pub fn invoke_proxy_arguments(
    env: JniEnv,
    receiver: *mut c_void,
    interface: &CStr,
    name: &CStr,
    signature: &CStr,
    raw: &[Arg],
    boxed: &[*mut c_void],
) {
    if env.push_local_frame(16) != 0 {
        return;
    }
    let interface = env.find_class(interface);
    if let Some(method) = env.method(interface, name, signature) {
        let bridge = env.find_class(c"org/rudroid/runtime/JavaProxy");
        if env.is_instance_of(receiver, interface) && env.is_instance_of(receiver, bridge) {
            let reflected = env.to_reflected_method(interface, method.id(), false);
            let base = env.find_class(c"java/lang/Object");
            let arguments = env.new_object_array(boxed.len() as i32, base, std::ptr::null_mut());
            for (index, value) in boxed.iter().enumerate() {
                env.set_object_array_element(arguments, index as i32, *value);
            }
            if let Some(invoke) = env.static_method(
                bridge,
                c"invoke",
                c"(Ljava/lang/Object;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;",
            ) {
                env.call_static_object(
                    bridge,
                    invoke,
                    &[Arg::object(receiver), Arg::object(reflected), Arg::object(arguments)],
                );
            }
        } else {
            let actual = env.get_object_class(receiver);
            if let Some(method) = env.method(actual, name, signature) {
                env.call_void(receiver, method, raw);
            }
            env.delete_local_ref(actual);
        }
    }
    env.pop_local_frame(std::ptr::null_mut());
}

/// Queue a VM-owned Runnable on the framework's single main Looper. The
/// framework retains the queued root.
pub fn post_main_runnable(env: JniEnv, runnable: *mut c_void) -> bool {
    if env.push_local_frame(8) != 0 {
        return false;
    }
    let class = env.find_class(c"android/os/Looper");
    let looper = env
        .static_method(class, c"getMainLooper", c"()Landroid/os/Looper;")
        .map_or(std::ptr::null_mut(), |method| {
            env.call_static_object(class, method, &[])
        });
    let class = env.find_class(c"android/os/Handler");
    let handler = env
        .method(class, c"<init>", c"(Landroid/os/Looper;)V")
        .map_or(std::ptr::null_mut(), |ctor| {
            env.new_object(class, ctor, &[Arg::object(looper)])
        });
    let queued = match env.method(class, c"post", c"(Ljava/lang/Runnable;)Z") {
        Some(post) if !handler.is_null() => {
            env.call_boolean(handler, post, &[Arg::object(runnable)])
        }
        _ => false,
    };
    env.pop_local_frame(std::ptr::null_mut());
    queued
}

#[cfg(test)]
#[path = "../tests/unit/jni_abi.rs"]
mod tests;

/// Declare a superclass using metadata in the shared VM, without plugin state.
pub fn declare_superclass(env: JniEnv, child: &CStr, parent: &CStr) -> i32 {
    let types = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let child = declare_class(env, child);
    let parent = declare_class(env, parent);
    let method = env.static_method(
        types,
        c"setSuperclass",
        c"(Ljava/lang/Class;Ljava/lang/Class;)V",
    );
    let valid = method.is_some() && [types, child, parent].iter().all(|h| !h.is_null());
    if let (true, Some(method)) = (valid, method) {
        env.call_static_void(types, method, &[Arg::object(child), Arg::object(parent)]);
    }
    for handle in [types, child, parent] {
        if !handle.is_null() {
            env.delete_local_ref(handle);
        }
    }
    if valid && !env.exception_check() {
        0
    } else {
        -1
    }
}

/// Quiesce independent runtime services and join core workers without touching
/// any engine-specific player method. Preserve the first exception. Used by
/// the Project CLI when no Unity nativeDone path is available.
pub fn shutdown_runtime_lifecycle(env: JniEnv) -> bool {
    let mut first = env.exception_occurred();
    env.exception_clear();
    let capture = |first: &mut *mut c_void| {
        let error = env.exception_occurred();
        env.exception_clear();
        if first.is_null() {
            *first = error;
        } else {
            env.delete_local_ref(error);
        }
    };
    let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
    capture(&mut first);
    crate::diagnostic!("[Lifecycle] quiescing service producers");
    if let Some(stop) = env.static_method(lifecycle, c"shutdown", c"()V") {
        env.call_static_void(lifecycle, stop, &[]);
    }
    capture(&mut first);
    crate::diagnostic!("[Lifecycle] joining core workers");
    if let Some(finish) = env.static_method(lifecycle, c"finish", c"()V") {
        env.call_static_void(lifecycle, finish, &[]);
    }
    capture(&mut first);
    env.delete_local_ref(lifecycle);
    crate::diagnostic!("[Lifecycle] cleanup completed");
    if first.is_null() {
        true
    } else {
        env.throw(first);
        env.delete_local_ref(first);
        false
    }
}
