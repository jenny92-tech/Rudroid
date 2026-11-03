//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
#[test]
fn data_globals_have_pointer_indirection_and_foreign_versions_are_rejected() {
    unsafe {
        let address = rudroid_media_fallback_symbol(c"AMEDIAFORMAT_KEY_DURATION".as_ptr(),
            std::ptr::null(), std::ptr::null(), std::ptr::null_mut());
        assert_ne!(address, 0);
        let text = *(address as *const *const c_char);
        assert_eq!(std::ffi::CStr::from_ptr(text), c"durationUs");
        assert_eq!(rudroid_media_fallback_symbol(c"AMEDIAFORMAT_KEY_DURATION".as_ptr(),
            c"libc.so".as_ptr(), c"LIBC".as_ptr(), std::ptr::null_mut()), 0);
        assert_eq!(rudroid_media_fallback_symbol(c"unknown".as_ptr(),
            std::ptr::null(), std::ptr::null(), std::ptr::null_mut()), 0);
    }
}
