//! INPUT: shared explicit diagnostics policy and borrowed C log records
//! OUTPUT: ERROR/FATAL stderr records and opt-in ordinary diagnostics
//! POS: keeps the deployable log capability quiet in normal production runs
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

#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_char, c_int, CStr};

use rudroid_common::{
    host_api::{HostApiV0, HOST_API_VERSION},
    registry::HostStr,
};

rudroid_common::declare_descriptor!("platform_log_stderr", rudroid_common::Kind::Logging);

fn write_record(tag: &str, message: &str) {
    if rudroid_common::logging::is_error_tag(tag) || rudroid_common::logging::diagnostics_enabled() {
        rudroid_common::logging::stderr(format_args!("[{tag}] {message}"));
    }
}

extern "C" fn log_sink_entry(tag: HostStr<'_>, message: HostStr<'_>) {
    let tag = tag
        .to_cstr()
        .map_or("<null>", |value| value.to_str().unwrap_or("<utf8>"));
    let message = message
        .to_cstr()
        .map_or("<null>", |value| value.to_str().unwrap_or("<utf8>"));
    write_record(tag, message);
}

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
    write_record(render(tag), render(message));
}

#[no_mangle]
extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    if !api.register_log(log_sink_entry) {
        return -1;
    }
    0
}

#[no_mangle]
extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    api.log_line(c"log_stderr", "stderr log sink active");
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
