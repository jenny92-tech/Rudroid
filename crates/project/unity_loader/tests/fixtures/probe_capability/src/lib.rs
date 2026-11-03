//! INPUT:  Host API config/env getters and optional expected game working directory
//! OUTPUT:  fixture android_base descriptor and native phase diagnostics
//! POS:    proves installed config, bounded C strings and native-constructor cwd
//! Loader CLI test fixture (ticket 44): an `android_base`-kind capability
//! whose init reads `device.displayWidth` through the Host API v0 config
//! getter and logs the observed value. Deployed as `libandroid_base.so` it
//! proves end-to-end that the CLI installed the Host Config before the
//! plugin phases ran. Kept outside the workspace so it never ships in a kit.
//!
//! Ticket 46: init also reads the synthetic `RUDROID_REVIEW_*` environment
//! variables through the Host API getenv entry the way a C consumer would
//! (walking bytes until NUL), so a memory checker running the CLI observes
//! any over-read directly. This is a test fixture, not a JNI runtime: it
//! registers no classes and serves no guest calls.

use std::ffi::{c_char, c_int};

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::HostStr;

rudroid_common::declare_descriptor!("android_base", rudroid_common::Kind::AndroidBase);

/// C-consumer strlen: walks until the NUL terminator. Used so the valgrind
/// run sees exactly the reads a real C capability would perform.
unsafe fn c_strlen(mut ptr: *const c_char) -> usize {
    let mut len = 0usize;
    // SAFETY: the host API contract returns a valid NUL-terminated string.
    while unsafe { *ptr } != 0 {
        len += 1;
        // SAFETY: still inside the same string.
        ptr = unsafe { ptr.add(1) };
    }
    len
}

/// Announces the early phase so loader tests can assert that the logging
/// capability's early init runs before this android_base's (reference
/// `load_jni` order).
#[no_mangle]
extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    api.log_line(c"probe", "phase=early");
    0
}

#[no_mangle]
extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    let width = api.config_i64(c"device.displayWidth", -1);
    if let Some(expected) = std::env::var_os("RUDROID_EXPECT_GAME_CWD") {
        let cwd = std::env::current_dir().unwrap();
        if cwd != std::path::PathBuf::from(expected)
            || api.config_string(c"paths.game_files", 4096).as_deref() != cwd.to_str() {
            api.log_line(c"probe", "game cwd/config root mismatch");
            return -9;
        }
        api.log_line(c"probe", "game cwd/config root verified by native init");
    }

    // Ticket 46 probe: walk the returned strings like C code would.
    let env_value = (api.getenv)(HostStr::new(c"RUDROID_REVIEW_VALUE"));
    let env_len = if env_value.is_null() {
        "missing".to_owned()
    } else {
        // SAFETY: host API contract.
        format!("len={}", unsafe { c_strlen(env_value) })
    };
    let env_empty = (api.getenv)(HostStr::new(c"RUDROID_REVIEW_EMPTY"));
    let empty_state = if env_empty.is_null() {
        "missing"
    } else {
        // SAFETY: host API contract.
        if unsafe { c_strlen(env_empty) } == 0 {
            "empty-string"
        } else {
            "nonempty"
        }
    };

    api.log_line(
        c"probe",
        &format!("displayWidth={width}; env RUDROID_REVIEW_VALUE {env_len}; RUDROID_REVIEW_EMPTY={empty_state}"),
    );
    0
}
