//! INPUT:  Android abort/assert prototypes, borrowed C text and native stderr/abort
//! OUTPUT:  android_set_abort_message, abort, __assert2 and __stack_chk_fail addresses
//! POS:    retains the first fatal message in one owned buffer and reports native failures

use std::ffi::{c_char, c_int, CStr};
use std::sync::OnceLock;

static MESSAGE: OnceLock<Vec<u8>> = OnceLock::new();

unsafe extern "C" fn set_message(message: *const c_char) {
    if MESSAGE.get().is_some() {
        return;
    }
    let text = if message.is_null() {
        c"(null)"
    } else {
        CStr::from_ptr(message)
    };
    // Android keeps the first message and silently tolerates allocation failure.
    // Copy once: a guest buffer/DSO can be released before the process aborts.
    let mut owned = Vec::new();
    if owned.try_reserve_exact(text.to_bytes().len()).is_err() {
        return;
    }
    owned.extend_from_slice(text.to_bytes());
    let _ = MESSAGE.set(owned); // A concurrent loser drops its sole owned copy.
}

unsafe fn report(mut text: &[u8]) {
    while !text.is_empty() {
        let result = libc::write(libc::STDERR_FILENO, text.as_ptr().cast(), text.len());
        if result > 0 {
            text = &text[result as usize..];
        } else if result < 0 && *libc::__errno_location() == libc::EINTR {
            continue;
        } else {
            break;
        }
    }
}
unsafe extern "C" fn abort() -> ! {
    if let Some(message) = MESSAGE.get() {
        report(b"[BIONIC] abort: ");
        report(message);
        report(b"\n");
    }
    libc::abort()
}
unsafe extern "C" fn assert2(
    file: *const c_char,
    line: c_int,
    function: *const c_char,
    expression: *const c_char,
) -> ! {
    report(b"[BIONIC] assertion failed: ");
    for text in [file, function, expression] {
        if !text.is_null() {
            report(CStr::from_ptr(text).to_bytes());
        }
        report(b" ");
    }
    let mut buffer = [0u8; 32];
    let length = libc::snprintf(
        buffer.as_mut_ptr().cast(),
        buffer.len(),
        c"(line %d)\n".as_ptr(),
        line,
    );
    if length > 0 {
        report(&buffer[..(length as usize).min(buffer.len() - 1)]);
    }
    set_message(expression);
    abort()
}
unsafe extern "C" fn stack_fail() -> ! {
    set_message(c"stack corruption detected (-fstack-protector)".as_ptr());
    abort()
}
symbol_provider! {
    b"android_set_abort_message" => set_message as *const () as usize,
    b"abort" => abort as *const () as usize,
    b"__assert2" => assert2 as *const () as usize,
    b"__stack_chk_fail" => stack_fail as *const () as usize,
}
