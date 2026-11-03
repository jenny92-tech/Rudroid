//! INPUT:  private items of `src/jni_abi.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/jni_abi.rs`; declared there with `#[path]`
use super::*;

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
