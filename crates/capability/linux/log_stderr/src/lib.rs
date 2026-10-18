//! Host log capability (ticket 23): writes `[tag] message` lines to stderr.
//!
//! The loader's Host API v0 `log` entry defaults to stderr already; this
//! capability is the deployable counterpart from the reference kit
//! (`platform_log_stderr`, kind `logging`). It exports:
//!
//! - `rudroid_plugin_descriptor` (via `declare_descriptor!`)
//! - `rudroid_log_stderr(tag, message)` — direct stderr write
//! - `rudroid_plugin_init` — announces the sink through the Host API
//!
//! No SDL, no Android assumptions.

use std::ffi::{c_char, c_int, CStr};

rudroid_common::declare_descriptor!("platform_log_stderr", rudroid_common::Kind::Logging);

/// # Safety
/// Both arguments must be NUL-terminated C strings (null is tolerated and
/// rendered as `<null>` so a logging bug cannot crash the host).
#[no_mangle]
pub unsafe extern "C" fn rudroid_log_stderr(tag: *const c_char, message: *const c_char) {
    let render = |ptr: *const c_char| -> &str {
        if ptr.is_null() {
            "<null>"
        } else {
            // SAFETY: non-null arguments are NUL-terminated C strings (contract above).
            unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("<utf8>")
        }
    };
    eprintln!("[{}] {}", render(tag), render(message));
}

#[no_mangle]
extern "C" fn rudroid_plugin_init(
    api: Option<&'static rudroid_common::host_api::HostApiV0>,
) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    api.log_line(c"log_stderr", "stderr log sink active");
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
