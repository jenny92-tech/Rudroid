//! Loader test fixture (ticket 45): registers a module-loaded subscription
//! through the Host API in `early_init`, then FAILS `init` with code 5.
//! Loader tests prove that a phase failure aborts bring-up with a structured
//! error and shuts the runtime registry down, so this subscription can never
//! fire afterwards. The callback counts events into an exported atomic so
//! tests can dlsym the count. Kept outside the workspace so it never ships.

use std::ffi::c_int;
use std::sync::atomic::{AtomicU32, Ordering};

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::RudroidModuleLoadedCallback;

rudroid_common::declare_descriptor!("failing", rudroid_common::Kind::Logging);

static DANGLING_EVENTS: AtomicU32 = AtomicU32::new(0);

/// Exported for tests: how many module events the subscription received.
/// Must stay 0 after the init failure and registry shutdown.
#[no_mangle]
pub extern "C" fn rudroid_failing_dangling_events() -> u32 {
    DANGLING_EVENTS.load(Ordering::SeqCst)
}

extern "C" fn on_module_loaded(
    _event: Option<&rudroid_common::registry::RudroidModuleEvent>,
    _user: *mut std::ffi::c_void,
) {
    DANGLING_EVENTS.fetch_add(1, Ordering::SeqCst);
}

#[no_mangle]
extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    let callback: RudroidModuleLoadedCallback = on_module_loaded;
    (api.register_module_loaded)(
        rudroid_common::ModuleKind::Main as u32,
        Some(callback),
        std::ptr::null_mut(),
    )
}

#[no_mangle]
extern "C" fn rudroid_plugin_init(_api: Option<&'static HostApiV0>) -> c_int {
    // The documented deterministic failure used by loader tests.
    5
}
