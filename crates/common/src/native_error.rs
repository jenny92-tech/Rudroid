//! INPUT: a pinned provider's thread-local C error channel
//! OUTPUT: owned UTF-8 diagnostic
//! POS: shared two-call diagnostic channel for SDL and ELF providers
#![deny(unsafe_op_in_unsafe_fn)]
use std::ffi::c_char;

/// # Safety
/// The callback must accept (null, 0) for the required length including NUL,
/// write at most the supplied capacity, and stay callable through both calls.
pub unsafe fn read(channel: unsafe extern "C" fn(*mut c_char, usize) -> usize) -> Option<String> {
    // SAFETY: null output queries the length without writing.
    let needed = unsafe { channel(std::ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(needed).ok()?;
    buffer.resize(needed, 0u8);
    // SAFETY: this buffer owns exactly the reported writable span.
    let copied = unsafe { channel(buffer.as_mut_ptr().cast(), needed) };
    if copied != needed || buffer.pop() != Some(0) {
        return None;
    }
    String::from_utf8(buffer).ok()
}
