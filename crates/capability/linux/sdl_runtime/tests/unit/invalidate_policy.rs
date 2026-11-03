//! INPUT:  pointer-free framebuffer hint policy
//! OUTPUT:  disabled fallback, exact symbol matching and ignored-pointer checks
//! POS:    guards opt-in scope and prevents accidental guest attachment access
use super::*;

#[test]
fn disabled_policy_preserves_native_resolution_for_all_hints() {
    for name in [b"glInvalidateFramebuffer".as_slice(), b"glInvalidateSubFramebuffer", b"glDiscardFramebufferEXT"] {
        assert_eq!(symbol(false, name), 0);
        assert_ne!(symbol(true, name), 0);
    }
    for name in [b"glDeleteFramebuffers".as_slice(), b"glBindFramebuffer", b"glInvalidateFramebufferEXT", b"glInvalidateFramebufferExtra"] {
        assert_eq!(symbol(true, name), 0);
    }
}

#[test]
fn ignored_hints_never_access_or_retain_attachment_storage() {
    let invalid = usize::MAX as *const u32;
    discard(u32::MAX, i32::MAX, invalid);
    discard_sub(u32::MAX, i32::MAX, invalid, i32::MIN, i32::MAX, -1, -1);
    discard(0, -1, std::ptr::null());
}
