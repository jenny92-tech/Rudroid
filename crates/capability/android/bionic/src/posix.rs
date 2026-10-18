//! INPUT:  native GNU descriptor services, safe asset paths and guest syslog identities
//! OUTPUT:  native fsync/openlog and one C-owned asset-path adapter
//! POS:    shares native descriptor I/O and bounds retained/path string ownership

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

// Native openlog retains the pointer. A guest stack string or unloaded DSO
// cannot remain its backing storage. Keep one cached identity across closelog
// (some host versions may retain it), until a successful native replacement.
static IDENT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// # Safety
/// A non-null path is a live NUL-terminated C string for this call. The caller
/// frees a non-null result with native free after its I/O operation; no Rust
/// allocation or guest pointer is retained or handed to native free.
#[no_mangle]
pub(super) unsafe extern "C" fn rudroid_bionic_asset_path(path: *const c_char) -> *mut c_char {
    if path.is_null() { return std::ptr::null_mut(); }
    // SAFETY: the ABI caller lends a live C string; strdup creates an owned
    // native-heap copy before the temporary Rust CString is dropped.
    let mapped = super::paths::extracted_asset(CStr::from_ptr(path));
    mapped.map_or(std::ptr::null_mut(), |value| libc::strdup(value.as_ptr()))
}

unsafe extern "C" fn openlog(ident: *const c_char, options: c_int, facility: c_int) {
    let mut current = IDENT.lock().unwrap_or_else(|e| e.into_inner());
    if ident.is_null() {
        libc::openlog(ident, options, facility);
        return;
    }
    let text = CStr::from_ptr(ident).to_bytes_with_nul();
    if current.as_slice() == text {
        libc::openlog(current.as_ptr().cast(), options, facility);
        return;
    }
    let mut next = Vec::new();
    if next.try_reserve_exact(text.len()).is_err() {
        return;
    }
    next.extend_from_slice(text);
    // Native syslog/openlog serialize their use of LogTag. Publish the new
    // pointer through native openlog before freeing the previous owned copy.
    libc::openlog(next.as_ptr().cast(), options, facility);
    *current = next;
}

// The C POSIX/locale/jump/signal/network/atfork tables are separate
// providers in registry.rs; this module owns the retained openlog tag and
// scalar descriptor sync. Android and GNU fsync both take a native int fd
// and return int; the existing I/O providers already share host descriptors
// and thread-local errno. No facade or extra buffer is needed here.
symbol_provider! {
    b"openlog" => openlog as *const () as usize,
    b"fsync" => libc::fsync as *const () as usize,
}
