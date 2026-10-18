//! Loader test fixture: exports the kit descriptor plus a minimal init that
//! exercises the Host API v0 log entry (ticket 11).
//! Kept outside the workspace so it never ships in a real kit.

use std::ffi::c_int;

use rudroid_common::host_api::HostApiV0;

rudroid_common::declare_descriptor!("dummy", rudroid_common::Kind::Logging);

/// Phases announce themselves through the Host API log so loader tests can
/// assert the invocation order.
fn log_phase(api: &HostApiV0, phase: &str) {
    api.log_line(c"dummy", &format!("phase={phase}"));
}

#[no_mangle]
extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else { return -1 };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    log_phase(api, "early");
    0
}

#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else { return -1 };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    log_phase(api, "register_jni");
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
    log_phase(api, "init");
    0
}
