//! INPUT: a `jni_env::JniEnv` of the shared session, JNI table slots and scalar argument values
//! OUTPUT: pure JNI slot reader, C-layout jvalue/native-method types and safe session helpers on `JniEnv`
//! POS: shared ABI definitions without session state or plugin exports; only `function_slot` reads slots by number
//! Pure JNI ABI helper shared by capabilities that call through the shared
//! session's `JavaVM*` / `JNIEnv*` (no plugin exports, no session state).
//!
//! JNI hands out handles, not tables: a `JavaVM*` points at a
//! `JNIInvokeInterface*` and a `JNIEnv*` points at a `JNINativeInterface*`,
//! so reading slot `index` is a double dereference — handle → function table
//! → entry. Slot numbers follow the JNI 1.6 spec order (the invoke interface
//! starts with three reserved slots, the native interface with four).

use std::ffi::{c_char, c_void, CStr};

use crate::jni_env::{Arg, JniEnv};

/// Borrowed JNI native registration table. The session copies names and
/// signatures; plugin and guest function addresses stay process-pinned.
#[repr(C)]
pub struct NativeMethod {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub fn_ptr: *mut c_void,
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
    let class = env.find_class(class_name);
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

/// JNI's scalar argument union. A call's method descriptor determines which
/// member is read; references remain owned by the shared JNI session.
#[repr(C)]
#[derive(Clone, Copy)]
pub union JValue {
    pub z: u8,
    pub b: i8,
    pub c: u16,
    pub s: i16,
    pub i: i32,
    pub j: i64,
    pub f: f32,
    pub d: f64,
    pub l: *mut c_void,
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

/// Read function-pointer slot `index` of the table behind a JNI handle and
/// return it as `T` (an `unsafe extern "C" fn` of the slot's signature).
///
/// Panics when the handle, its table pointer or the slot itself is null — a
/// broken session table is a hard bring-up error, never a silent miss.
///
/// # Safety
/// `handle` must be null or a live `JavaVM*`/`JNIEnv*` whose first field
/// points at a function table with at least `index + 1` entries, and slot
/// `index` must hold a function of exactly `T`'s signature. `T` must be
/// pointer-sized.
pub unsafe fn function_slot<T: Copy>(handle: *const c_void, index: usize) -> T {
    assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<usize>(),
        "JNI slots hold pointer-sized function pointers"
    );
    assert!(
        !handle.is_null(),
        "JNI handle is null — the session handed out a broken JavaVM/JNIEnv"
    );
    // SAFETY: caller guarantees a live handle whose first field is the
    // function-table pointer.
    let functions = unsafe { *(handle as *const *const usize) };
    assert!(!functions.is_null(), "JNI function table pointer is null");
    // SAFETY: caller guarantees the table holds at least `index + 1` slots.
    let entry = unsafe { *functions.add(index) };
    assert!(entry != 0, "JNI slot {index} is null in the session table");
    // SAFETY: caller guarantees the slot holds exactly `T`; sizes match.
    unsafe { std::mem::transmute_copy::<usize, T>(&entry) }
}

#[cfg(test)]
#[path = "../tests/unit/jni_abi.rs"]
mod tests;

/// Declare a superclass using metadata in the shared VM, without plugin state.
pub fn declare_superclass(env: JniEnv, child: &CStr, parent: &CStr) -> i32 {
    let types = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let child = env.find_class(child);
    let parent = env.find_class(parent);
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

/// Cached input dispatch on the player/SDL creator thread. The VM owns events;
/// this scoped adapter retains only one local class handle and method IDs.
pub struct PlayerInput {
    env: JniEnv,
    source: *mut c_void,
    poll: crate::jni_env::Method,
    quit: crate::jni_env::Method,
    inject: Option<Option<crate::jni_env::Method>>,
    trace: bool,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl PlayerInput {
    /// `env` is the current thread's env; the adapter stays on this thread.
    pub fn new(env: JniEnv, trace: bool) -> Option<Self> {
        let source = env.find_class(c"org/rudroid/runtime/InputSource");
        let poll = env.static_method(source, c"poll", c"()Ljava/lang/Object;");
        let quit = env.static_method(source, c"quitRequested", c"()Z");
        let (false, Some(poll), Some(quit)) = (source.is_null(), poll, quit) else {
            env.delete_local_ref(source);
            return None;
        };
        Some(Self {
            env,
            source,
            poll,
            quit,
            inject: None,
            trace,
            _thread: std::marker::PhantomData,
        })
    }

    /// Pump bounded input before rendering. `Ok(true)` requests a clean exit.
    pub fn pump(&mut self, player: *mut c_void, class: *mut c_void) -> Result<bool, ()> {
        self.pump_with(player,class,|_|Ok(false))
    }

    /// Allow a player-owned text-input session to consume an event before native
    /// dispatch. The event stays local to this call and drops on error or unwind.
    pub fn pump_with(&mut self, player:*mut c_void, class:*mut c_void,
        mut consume:impl FnMut(*mut c_void)->Result<bool,()>)->Result<bool,()> {
        let env = self.env;
        for _ in 0..64 {
            let event = env.call_static_object(self.source, self.poll, &[]);
            let _event = crate::jni_env::LocalRef {env,object:event};
            if env.exception_check() {
                return Err(());
            }
            if event.is_null() {
                break;
            }
            if consume(event)? { continue; }
            let inject = *self.inject.get_or_insert_with(|| {
                env.method(class, c"nativeInjectEvent", c"(Landroid/view/InputEvent;)Z")
            });
            let Some(inject) = inject else {
                return Err(());
            };
            let accepted = u8::from(env.call_boolean(player, inject, &[Arg::object(event)]));
            if self.trace {
                eprintln!("[UnityInput] nativeInjectEvent accepted={accepted}");
            }
            if env.exception_check() {
                return Err(());
            }
        }
        let quit = env.call_static_boolean(self.source, self.quit, &[]);
        if env.exception_check() {
            Err(())
        } else {
            Ok(quit)
        }
    }
}

impl Drop for PlayerInput {
    fn drop(&mut self) {
        self.env.delete_local_ref(self.source);
    }
}

/// Quiesce independent services, let nativeDone own terminal producer stop,
/// then join final services/core workers. Preserve the first exception.
/// Runs on the player thread. `initialized` means initJni was attempted;
/// otherwise player/class may be null and only services are closed.
/// Independent services join in shutdown; native-owned producers must honor
/// their owner's stop-before-free callback and register final joins in finish.
/// Do not inject a suspend before terminal destruction: it can stop the mixer
/// still needed for streamed native releases. Suspend/resume remain separate.
pub fn finish_player(
    env: JniEnv,
    player: *mut c_void,
    class: *mut c_void,
    initialized: bool,
) -> bool {
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
    if initialized {
        eprintln!("[UnityLifecycle] native owner controls terminal stop order");
    }
    let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
    capture(&mut first);
    eprintln!("[UnityLifecycle] quiescing service producers");
    if let Some(stop) = env.static_method(lifecycle, c"shutdown", c"()V") {
        env.call_static_void(lifecycle, stop, &[]);
    }
    capture(&mut first);
    if initialized {
        eprintln!("[UnityLifecycle] destroying native player");
        if let Some(done) = env.method(class, c"nativeDone", c"()Z") {
            env.call_boolean(player, done, &[]);
        }
        capture(&mut first);
    }
    eprintln!("[UnityLifecycle] joining core workers");
    if let Some(finish) = env.static_method(lifecycle, c"finish", c"()V") {
        env.call_static_void(lifecycle, finish, &[]);
    }
    capture(&mut first);
    env.delete_local_ref(lifecycle);
    eprintln!("[UnityLifecycle] cleanup completed");
    if first.is_null() {
        true
    } else {
        env.throw(first);
        env.delete_local_ref(first);
        false
    }
}
