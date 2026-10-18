//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
#[test]
fn explicit_names_only_without_foreign_version_fallback() {
    unsafe {
        assert_ne!(rudroid_zlib_symbol(c"inflateInit2_".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut()),0);
        assert_eq!(rudroid_zlib_symbol(c"inflateInit2_".as_ptr(),c"libc.so".as_ptr(),c"LIBC".as_ptr(),std::ptr::null_mut()),0);
        assert_eq!(rudroid_zlib_symbol(c"unknown".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut()),0);
    }
}
