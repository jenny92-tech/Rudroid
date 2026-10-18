//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
#[test]
fn explicit_names_only_without_foreign_version_fallback() {
    let _fds=TEST_FDS.lock().unwrap_or_else(|error| error.into_inner());
    unsafe {
        assert_ne!(rudroid_native_events_symbol(c"ALooper_pollOnce".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut()),0);
        assert_eq!(rudroid_native_events_symbol(c"ALooper_pollOnce".as_ptr(),c"libc.so".as_ptr(),c"LIBC".as_ptr(),std::ptr::null_mut()),0);
        assert_eq!(rudroid_native_events_symbol(c"unknown".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut()),0);
    }
    // Exhaust fd availability on a fresh native thread, restore the process
    // limit before asserting, then retry to prove partial creation rolls back.
    std::thread::spawn(|| unsafe {
        #[repr(C)]
        struct Limit { current: u64, maximum: u64 }
        unsafe extern "C" {
            fn getrlimit(resource: i32, limit: *mut Limit) -> i32;
            fn setrlimit(resource: i32, limit: *const Limit) -> i32;
        }
        let address=rudroid_native_events_symbol(c"ALooper_prepare".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut());
        let prepare: unsafe extern "C" fn(i32)->*mut c_void=std::mem::transmute(address);
        let mut limit=Limit { current:0,maximum:0 };
        assert_eq!(getrlimit(7,&mut limit),0);
        let exhausted=Limit { current:0,maximum:limit.maximum };
        assert_eq!(setrlimit(7,&exhausted),0);
        let failed=prepare(1);
        let restored=setrlimit(7,&limit);
        assert_eq!(restored,0);
        assert!(failed.is_null());
        // Allow exactly one free descriptor: epoll succeeds, eventfd fails.
        // Existing open descriptors above the lowered limit are untouched.
        let mut first_free=0;
        while libc::fcntl(first_free,libc::F_GETFD)>=0 { first_free+=1; }
        let partial=Limit { current:(first_free+1) as u64,maximum:limit.maximum };
        assert_eq!(setrlimit(7,&partial),0);
        let failed=prepare(1);
        let restored=setrlimit(7,&limit);
        assert_eq!(restored,0);
        assert!(failed.is_null());
        assert_eq!(libc::fcntl(first_free,libc::F_GETFD),-1,"failed eventfd creation must close epoll");
        assert!(!prepare(1).is_null());
    }).join().unwrap();
}
