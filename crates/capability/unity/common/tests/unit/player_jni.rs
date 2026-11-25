//! INPUT:  `player_jni` public dispatch helpers via the crate root
//! OUTPUT:  PlayerInputDispatch probe regressions
//! POS:    wired from `lib.rs` so `player_jni`'s `forbid(unsafe_code)` does not cover probes
use crate::player_jni::*;
use rudroid_common::jni_env::{slot, JObject, JniEnv};
use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_void, CStr};

thread_local! {
    static PROBE_SCENARIO: Cell<u8> = const { Cell::new(0) };
    static PROBE_LOOKUPS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static PROBE_PENDING: Cell<usize> = const { Cell::new(0) };
    static PROBE_THROWN: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn probe_find_class(_env: *mut c_void, name: *const c_char) -> JObject {
    if unsafe { CStr::from_ptr(name) }.to_bytes() == b"java/lang/NoSuchMethodError" {
        0x200 as JObject
    } else {
        0x100 as JObject
    }
}

unsafe extern "C" fn probe_get_method_id(
    _env: *mut c_void,
    _class: JObject,
    _name: *const c_char,
    signature: *const c_char,
) -> *mut c_void {
    let signature = unsafe { CStr::from_ptr(signature) };
    let scenario = PROBE_SCENARIO.with(Cell::get);
    match signature.to_str().unwrap() {
        "(Landroid/view/InputEvent;I)Z" => {
            PROBE_LOOKUPS.with(|lookups| lookups.borrow_mut().push("new"));
            let pending = match scenario {
                1 => 0x900,
                2 => 0x901,
                3 => 0x902,
                _ => 0,
            };
            PROBE_PENDING.with(|value| value.set(pending));
            if pending == 0 {
                0x300 as JObject
            } else {
                std::ptr::null_mut()
            }
        }
        "(Landroid/view/InputEvent;)Z" => {
            PROBE_LOOKUPS.with(|lookups| lookups.borrow_mut().push("legacy"));
            if scenario == 3 {
                PROBE_PENDING.with(|value| value.set(0x903));
                std::ptr::null_mut()
            } else {
                0x300 as JObject
            }
        }
        other => panic!("unexpected input signature {other}"),
    }
}

unsafe extern "C" fn probe_exception_check(_env: *mut c_void) -> u8 {
    u8::from(PROBE_PENDING.with(Cell::get) != 0)
}

unsafe extern "C" fn probe_exception_occurred(_env: *mut c_void) -> JObject {
    PROBE_PENDING.with(Cell::get) as JObject
}

unsafe extern "C" fn probe_exception_clear(_env: *mut c_void) {
    PROBE_PENDING.with(|value| value.set(0));
}

unsafe extern "C" fn probe_is_instance_of(
    _env: *mut c_void,
    object: JObject,
    class: JObject,
) -> u8 {
    assert_eq!(class as usize, 0x200);
    u8::from(object as usize != 0x900)
}

unsafe extern "C" fn probe_throw(_env: *mut c_void, throwable: JObject) -> i32 {
    PROBE_THROWN.with(|value| value.set(throwable as usize));
    PROBE_PENDING.with(|value| value.set(throwable as usize));
    0
}

unsafe extern "C" fn probe_delete_local_ref(_env: *mut c_void, _object: JObject) {}

fn probe_env(scenario: u8) -> JniEnv {
    PROBE_SCENARIO.with(|value| value.set(scenario));
    PROBE_LOOKUPS.with(|lookups| lookups.borrow_mut().clear());
    PROBE_PENDING.with(|value| value.set(0));
    PROBE_THROWN.with(|value| value.set(0));
    let mut table = vec![0usize; 233];
    table[slot::env::FIND_CLASS] = probe_find_class as usize;
    table[slot::env::GET_METHOD_ID] = probe_get_method_id as usize;
    table[slot::env::THROW] = probe_throw as usize;
    table[slot::env::EXCEPTION_OCCURRED] = probe_exception_occurred as usize;
    table[slot::env::EXCEPTION_CLEAR] = probe_exception_clear as usize;
    table[slot::env::DELETE_LOCAL_REF] = probe_delete_local_ref as usize;
    table[slot::env::EXCEPTION_CHECK] = probe_exception_check as usize;
    table[slot::env::IS_INSTANCE_OF] = probe_is_instance_of as usize;
    let table = Box::leak(table.into_boxed_slice());
    let handle = Box::leak(Box::new(table.as_ptr()));
    unsafe { JniEnv::from_raw(handle as *mut *const usize as *mut c_void) }
}

#[test]
fn input_probe_does_not_fall_back_after_an_unexpected_lookup_exception() {
    let env = probe_env(1);
    assert!(PlayerInputDispatch::resolve(env, 0x100 as JObject).is_err());
    PROBE_LOOKUPS.with(|lookups| assert_eq!(*lookups.borrow(), ["new"]));
    assert_eq!(PROBE_PENDING.with(Cell::get), 0x900);
    assert_eq!(PROBE_THROWN.with(Cell::get), 0x900);
}

#[test]
fn input_probe_falls_back_only_for_no_such_method_error() {
    let env = probe_env(2);
    match PlayerInputDispatch::resolve(env, 0x100 as JObject) {
        Ok(PlayerInputDispatch::Legacy(_)) => {}
        _ => panic!("NoSuchMethodError must select the legacy shape"),
    }
    PROBE_LOOKUPS.with(|lookups| assert_eq!(*lookups.borrow(), ["new", "legacy"]));
    assert_eq!(PROBE_PENDING.with(Cell::get), 0);
}

#[test]
fn both_input_lookup_misses_restore_the_first_exception_identity() {
    let env = probe_env(3);
    assert!(PlayerInputDispatch::resolve(env, 0x100 as JObject).is_err());
    PROBE_LOOKUPS.with(|lookups| assert_eq!(*lookups.borrow(), ["new", "legacy"]));
    assert_eq!(PROBE_PENDING.with(Cell::get), 0x902);
    assert_eq!(PROBE_THROWN.with(Cell::get), 0x902);
}
