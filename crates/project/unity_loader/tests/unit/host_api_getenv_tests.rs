//! INPUT:  private items of `src/host_api.rs` via `super`
//! OUTPUT:  `getenv_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/host_api.rs`; declared there with `#[path]`
use super::*;
use std::ffi::CStr;

/// Ticket 46: getenv must return NUL-terminated strings, stable pointers
/// for repeated reads, and null for missing variables. Synthetic
/// RUDROID_REVIEW_* names only — the rest of the user environment is never
/// touched by the cache.
#[test]
fn getenv_returns_nul_terminated_stable_strings() {
    std::env::set_var("RUDROID_REVIEW_VALUE", "hello-kit");
    std::env::set_var("RUDROID_REVIEW_EMPTY", "");

    unsafe {
        let value = api_getenv(HostStr::new(c"RUDROID_REVIEW_VALUE"));
        assert!(!value.is_null());
        assert_eq!(CStr::from_ptr(value).to_bytes(), b"hello-kit");

        // An empty value is a valid, NUL-terminated empty string — not null.
        let empty = api_getenv(HostStr::new(c"RUDROID_REVIEW_EMPTY"));
        assert!(!empty.is_null());
        assert_eq!(CStr::from_ptr(empty).to_bytes(), b"");

        let missing = api_getenv(HostStr::new(c"RUDROID_REVIEW_MISSING"));
        assert!(missing.is_null());

        // Duplicate reads return the same pointer (documented policy).
        let again = api_getenv(HostStr::new(c"RUDROID_REVIEW_VALUE"));
        assert_eq!(again, value);
        assert_eq!(CStr::from_ptr(again).to_bytes(), b"hello-kit");

        // A null name is rejected without panicking.
        assert!(api_getenv(HostStr::null()).is_null());
    }
}
