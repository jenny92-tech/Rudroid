//! C FFI surface of the Reply Catalog cdylib.
//!
//! POD in/out-params only; handles are opaque pointers. Symbol names mirror
//! `rudroid_common::symbols::reply_catalog` (ticket 01/04 contract):
//!
//! - `rudroid_reply_catalog_from_bytes(const uint8_t*, uintptr_t)` → handle
//!   (null on malformed input)
//! - `rudroid_reply_catalog_from_path(const char*)` → handle (null on
//!   missing/unreadable file or malformed blob)
//! - `rudroid_reply_catalog_query_int(handle, class, method, sig, int64_t*)`
//!   → `1` on hit (out-param written), `0` on miss (out-param untouched)
//! - `rudroid_reply_catalog_query_bool(handle, class, method, sig, int32_t*)`
//!   → same contract
//! - `rudroid_reply_catalog_destroy(handle)` — frees a handle; null is a
//!   no-op
//!
//! The exported C symbols are the `ffi`-prefixed ones below; the constants in
//! `common::symbols` carry the same names without decoration.

use std::ffi::{c_char, c_int, CStr};
use std::ptr;

use crate::Catalog;

/// Opaque handle for C consumers.
pub struct RudroidReplyCatalog {
    catalog: Catalog,
}

/// # Safety
/// `bytes` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn rudroid_reply_catalog_from_bytes(
    bytes: *const u8,
    len: usize,
) -> *mut RudroidReplyCatalog {
    if bytes.is_null() {
        return ptr::null_mut();
    }
    let slice = std::slice::from_raw_parts(bytes, len);
    match Catalog::from_blob(slice) {
        Ok(catalog) => Box::into_raw(Box::new(RudroidReplyCatalog { catalog })),
        Err(_) => ptr::null_mut(),
    }
}

/// # Safety
/// `path` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn rudroid_reply_catalog_from_path(
    path: *const c_char,
) -> *mut RudroidReplyCatalog {
    if path.is_null() {
        return ptr::null_mut();
    }
    let path = match CStr::from_ptr(path).to_str() {
        Ok(p) => p,
        Err(_) => return ptr::null_mut(),
    };
    match Catalog::from_path(path) {
        Ok(catalog) => Box::into_raw(Box::new(RudroidReplyCatalog { catalog })),
        Err(_) => ptr::null_mut(),
    }
}

/// Borrow keys for one ABI call. The caller must keep each non-null C
/// string valid for the returned references' lifetime; no key is retained.
unsafe fn key_parts<'a>(
    class: *const c_char,
    method: *const c_char,
    sig: *const c_char,
) -> Option<(&'a str, &'a str, &'a str)> {
    let read = |ptr: *const c_char| -> Option<&'a str> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: the caller keeps each non-null key a valid C string for `'a`.
        unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .ok()
    };
    Some((read(class)?, read(method)?, read(sig)?))
}

/// # Safety
/// Pointers must be valid; `out` must be writable when non-null.
#[no_mangle]
pub unsafe extern "C" fn rudroid_reply_catalog_query_int(
    handle: *const RudroidReplyCatalog,
    class: *const c_char,
    method: *const c_char,
    sig: *const c_char,
    out: *mut i64,
) -> c_int {
    let handle = match handle.as_ref() {
        Some(h) => h,
        None => return 0,
    };
    // SAFETY: the caller passes valid C strings (or null) for the call.
    let Some((class, method, sig)) = (unsafe { key_parts(class, method, sig) }) else {
        return 0;
    };
    match handle.catalog.query(class, method, sig) {
        Some(crate::ReplyValue::Int(value)) => {
            if !out.is_null() {
                *out = *value;
            }
            1
        }
        _ => 0,
    }
}

/// # Safety
/// Pointers must be valid; `out` must be writable when non-null.
#[no_mangle]
pub unsafe extern "C" fn rudroid_reply_catalog_query_bool(
    handle: *const RudroidReplyCatalog,
    class: *const c_char,
    method: *const c_char,
    sig: *const c_char,
    out: *mut c_int,
) -> c_int {
    let handle = match handle.as_ref() {
        Some(h) => h,
        None => return 0,
    };
    // SAFETY: the caller passes valid C strings (or null) for the call.
    let Some((class, method, sig)) = (unsafe { key_parts(class, method, sig) }) else {
        return 0;
    };
    match handle.catalog.query(class, method, sig) {
        Some(crate::ReplyValue::Bool(value)) => {
            if !out.is_null() {
                *out = *value as c_int;
            }
            1
        }
        _ => 0,
    }
}

/// # Safety
/// `handle` must have come from `from_bytes`/`from_path` and not been freed
/// before; null is accepted as a no-op.
#[no_mangle]
pub unsafe extern "C" fn rudroid_reply_catalog_destroy(handle: *mut RudroidReplyCatalog) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

#[cfg(test)]
#[path = "../tests/unit/ffi.rs"]
mod tests;
