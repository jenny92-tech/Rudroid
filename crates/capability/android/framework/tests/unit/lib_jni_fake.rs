//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `jni_fake` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use std::ffi::{c_char, c_void, CStr};
use std::sync::{Arc, Mutex, OnceLock};

use rudroid_common::jni_env::{slot, JNI_VERSION_1_6};

static VOLUME: Mutex<i32> = Mutex::new(100);
static FAIL_FIND: Mutex<bool> = Mutex::new(false);
static CALLS: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
/// Every JNINativeInterface slot (indices 0..=232).
const ENV_TABLE_SLOTS: usize = 233;
static ENV_TABLE: OnceLock<Box<[usize; ENV_TABLE_SLOTS]>> = OnceLock::new();
static VM_TABLE: OnceLock<Box<[usize; 8]>> = OnceLock::new();

fn calls() -> Arc<Mutex<Vec<String>>> {
    CALLS
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

/// Reset the knobs and hand back the call log.
pub fn reset(volume: i32) -> Arc<Mutex<Vec<String>>> {
    *VOLUME.lock().unwrap() = volume;
    *FAIL_FIND.lock().unwrap() = false;
    let calls = calls();
    calls.lock().unwrap().clear();
    calls
}

pub fn set_fail_find(fail: bool) {
    *FAIL_FIND.lock().unwrap() = fail;
}

/// The registered-VM pointer the join resolves through `shared_vm`.
pub fn vm_ptr() -> *mut c_void {
    let vm_table = VM_TABLE.get_or_init(|| {
        let mut table = Box::new([0usize; 8]);
        table[slot::vm::GET_ENV] = fake_get_env as usize;
        table
    });
    static VM: OnceLock<usize> = OnceLock::new();
    (VM.get_or_init(|| vm_table.as_ref().as_ptr() as usize) as *const usize)
        .cast_mut()
        .cast()
}

fn record(message: &str) {
    calls().lock().unwrap().push(message.to_owned());
}

unsafe extern "C" fn fake_get_env(
    _vm: *const c_void,
    env: *mut *mut c_void,
    version: i32,
) -> i32 {
    if version != JNI_VERSION_1_6 {
        return -3;
    }
    let env_table = ENV_TABLE.get_or_init(|| {
        let mut table = Box::new([0usize; ENV_TABLE_SLOTS]);
        table[slot::env::FIND_CLASS] = fake_find_class as usize;
        table[slot::env::NEW_GLOBAL_REF] = fake_new_global_ref as usize;
        table[slot::env::DELETE_LOCAL_REF] = fake_delete_local_ref as usize;
        table[22] = fake_delete_global_ref as usize;
        table[25] = fake_new_local_ref as usize;
        table[slot::env::GET_STATIC_METHOD_ID] = fake_get_static_method_id as usize;
        table[slot::env::CALL_STATIC_INT_METHOD_A] = fake_call_static_int as usize;
        table[215] = fake_register_natives as usize;
        table[143] = fake_call_static_void as usize;
        table[228] = fake_exception_check as usize;
        table[167] = fake_new_string as usize;
        table[163] = fake_new_utf16_string as usize;
        table[144] = fake_get_static_method_id as usize;
        table[94] = fake_get_static_method_id as usize;
        table[154] = fake_set_static_object as usize;
        table[159] = fake_set_static_int as usize;
        table
    });
    // SAFETY: non-null test out-param.
    static ENV: OnceLock<usize> = OnceLock::new();
    unsafe {
        *env = (ENV.get_or_init(|| env_table.as_ref().as_ptr() as usize) as *const usize)
            .cast_mut()
            .cast()
    };
    0
}

unsafe extern "C" fn fake_find_class(_env: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: test passes a NUL-terminated literal.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    record(&format!("find_class {name}"));
    if *FAIL_FIND.lock().unwrap() {
        return std::ptr::null_mut();
    }
    0x521 as *mut c_void
}

unsafe extern "C" fn fake_new_global_ref(_env: *mut c_void, obj: *mut c_void) -> *mut c_void {
    record(&format!("new_global_ref {obj:p}"));
    0x522 as *mut c_void
}

unsafe extern "C" fn fake_delete_local_ref(_env: *mut c_void, obj: *mut c_void) {
    record(&format!("delete_local_ref {obj:p}"));
}
unsafe extern "C" fn fake_delete_global_ref(_env: *mut c_void, obj: *mut c_void) {
    record(&format!("delete_global_ref {obj:p}"));
}
unsafe extern "C" fn fake_new_local_ref(_env: *mut c_void, obj: *mut c_void) -> *mut c_void {
    record(&format!("new_local_ref {obj:p}"));
    obj
}

unsafe extern "C" fn fake_get_static_method_id(
    _env: *mut c_void,
    _class: *mut c_void,
    name: *const c_char,
    sig: *const c_char,
) -> *mut c_void {
    // SAFETY: test passes NUL-terminated literals.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    let sig = unsafe { CStr::from_ptr(sig) }.to_string_lossy();
    record(&format!("get_static_method_id {name}{sig}"));
    0x523 as *mut c_void
}

unsafe extern "C" fn fake_register_natives(
    _env: *mut c_void,
    _class: *mut c_void,
    methods: *const rudroid_common::jni_abi::NativeMethod,
    count: i32,
) -> i32 {
    assert!(!methods.is_null() && count > 0);
    record(&format!("register_natives {count}"));
    0
}
unsafe extern "C" fn fake_call_static_void(
    _env: *mut c_void,
    _class: *mut c_void,
    _method: *mut c_void,
    _args: *const rudroid_common::jni_abi::JValue,
) {
    record("call_static_void");
}
unsafe extern "C" fn fake_new_utf16_string(
    _env: *mut c_void,
    _value: *const u16,
    _len: i32,
) -> *mut c_void {
    0x524 as *mut c_void
}
unsafe extern "C" fn fake_new_string(_env: *mut c_void, _value: *const c_char) -> *mut c_void {
    0x524 as *mut c_void
}
unsafe extern "C" fn fake_set_static_object(
    _env: *mut c_void,
    _class: *mut c_void,
    _field: *mut c_void,
    _value: *mut c_void,
) {
}
unsafe extern "C" fn fake_set_static_int(
    _env: *mut c_void,
    _class: *mut c_void,
    _field: *mut c_void,
    _value: i32,
) {
}
unsafe extern "C" fn fake_exception_check(_env: *mut c_void) -> u8 {
    0
}

unsafe extern "C" fn fake_call_static_int(
    _env: *mut c_void,
    _class: *mut c_void,
    _method: *mut c_void,
    _args: *mut c_void,
) -> i32 {
    record("call_static_int");
    *VOLUME.lock().unwrap()
}
