//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use std::ffi::CString;

#[test]
fn direct_stderr_call_is_safe_for_null_and_bad_utf8() {
    let tag = CString::new("PLUGIN").unwrap();
    let message = CString::new("hello from log_stderr test").unwrap();
    // This writes the line to the captured test stderr output; the
    // end-to-end run in crates/docs/E2E-DEMO.md shows it in a real terminal.
    unsafe {
        rudroid_log_stderr(tag.as_ptr(), message.as_ptr());
        // Null / invalid inputs must not crash the host.
        rudroid_log_stderr(std::ptr::null(), std::ptr::null());
        let invalid: *const c_char = [0xff as u8, 0].as_ptr() as *const c_char;
        rudroid_log_stderr(tag.as_ptr(), invalid);
    }
}
