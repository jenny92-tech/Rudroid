//! INPUT:  private items of `src/jni_abi.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/jni_abi.rs`; declared there with `#[path]`
use super::*;
use std::ffi::c_char;
use crate::jni_env::{slot, JObject};
use std::cell::Cell;

unsafe extern "C" fn answer() -> i32 {
    42
}

#[test]
fn reads_the_slot_through_the_handle_indirection() {
    let table: [usize; 3] = [0, 0, answer as *const c_void as usize];
    let handle: *const usize = table.as_ptr();
    // The handle points at the table pointer, exactly like JNIEnv*.
    let handle_ptr = &handle as *const *const usize as *const c_void;
    let f = unsafe { function_slot::<unsafe extern "C" fn() -> i32>(handle_ptr, 2) };
    assert_eq!(unsafe { f() }, 42);
}

#[test]
#[should_panic(expected = "JNI handle is null")]
fn null_handle_panics() {
    let _ = unsafe { function_slot::<unsafe extern "C" fn()>(std::ptr::null(), 0) };
}

#[test]
#[should_panic(expected = "JNI slot 1 is null")]
fn null_slot_panics() {
    let table: [usize; 2] = [answer as *const c_void as usize, 0];
    let handle: *const usize = table.as_ptr();
    let handle_ptr = &handle as *const *const usize as *const c_void;
    let _ = unsafe { function_slot::<unsafe extern "C" fn() -> i32>(handle_ptr, 1) };
}

thread_local! {
    static LIFE_PENDING: Cell<usize> = const { Cell::new(0) };
    static LIFE_THROWN: Cell<usize> = const { Cell::new(0) };
    static LIFE_SHUTDOWN: Cell<u8> = const { Cell::new(0) };
    static LIFE_FINISH: Cell<u8> = const { Cell::new(0) };
}

unsafe extern "C" fn life_find_class(_env: *mut c_void, name: *const c_char) -> JObject {
    if unsafe { CStr::from_ptr(name) }.to_bytes() == b"org/rudroid/runtime/Lifecycle" {
        0x10 as JObject
    } else {
        std::ptr::null_mut()
    }
}

unsafe extern "C" fn life_get_static_method_id(
    _env: *mut c_void,
    _class: JObject,
    name: *const c_char,
    _signature: *const c_char,
) -> *mut c_void {
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"shutdown" => 0x20 as *mut c_void,
        b"finish" => 0x21 as *mut c_void,
        _ => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn life_call_static_void(
    _env: *mut c_void,
    _class: JObject,
    method: *mut c_void,
    _args: *const c_void,
) {
    match method as usize {
        0x20 => {
            LIFE_SHUTDOWN.with(|v| v.set(1));
            LIFE_PENDING.with(|v| v.set(0x99));
        }
        0x21 => {
            LIFE_FINISH.with(|v| v.set(1));
        }
        _ => {}
    }
}

unsafe extern "C" fn life_exception_check(_env: *mut c_void) -> u8 {
    u8::from(LIFE_PENDING.with(Cell::get) != 0)
}

unsafe extern "C" fn life_exception_occurred(_env: *mut c_void) -> JObject {
    LIFE_PENDING.with(Cell::get) as JObject
}

unsafe extern "C" fn life_exception_clear(_env: *mut c_void) {
    LIFE_PENDING.with(|v| v.set(0));
}

unsafe extern "C" fn life_throw(_env: *mut c_void, throwable: JObject) -> i32 {
    LIFE_THROWN.with(|v| v.set(throwable as usize));
    LIFE_PENDING.with(|v| v.set(throwable as usize));
    0
}

unsafe extern "C" fn life_delete_local_ref(_env: *mut c_void, _object: JObject) {}

#[test]
fn shutdown_runtime_lifecycle_preserves_the_first_exception() {
    LIFE_PENDING.with(|v| v.set(0));
    LIFE_THROWN.with(|v| v.set(0));
    LIFE_SHUTDOWN.with(|v| v.set(0));
    LIFE_FINISH.with(|v| v.set(0));
    let mut table = vec![0usize; 233];
    table[slot::env::FIND_CLASS] = life_find_class as usize;
    table[slot::env::GET_STATIC_METHOD_ID] = life_get_static_method_id as usize;
    table[slot::env::CALL_STATIC_VOID_METHOD_A] = life_call_static_void as usize;
    table[slot::env::EXCEPTION_CHECK] = life_exception_check as usize;
    table[slot::env::EXCEPTION_OCCURRED] = life_exception_occurred as usize;
    table[slot::env::EXCEPTION_CLEAR] = life_exception_clear as usize;
    table[slot::env::THROW] = life_throw as usize;
    table[slot::env::DELETE_LOCAL_REF] = life_delete_local_ref as usize;
    let table = Box::leak(table.into_boxed_slice());
    let handle = Box::leak(Box::new(table.as_ptr()));
    let env = unsafe { JniEnv::from_raw(handle as *mut *const usize as *mut c_void) };
    assert!(!shutdown_runtime_lifecycle(env));
    assert_eq!(LIFE_SHUTDOWN.with(Cell::get), 1);
    assert_eq!(LIFE_FINISH.with(Cell::get), 1);
    assert_eq!(LIFE_THROWN.with(Cell::get), 0x99);
}
