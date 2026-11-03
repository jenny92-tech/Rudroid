//! INPUT:  shared `after_unity_module` chain and the loader Host API table
//! OUTPUT:  `sdk_unity_helpers` descriptor, its `GuestModule` data and `rudroid_plugin_init`
//! POS:    Unity Helpers Android Addon: maps `lib/arm64-v8a/libUnityHelpers_Android.so` after the Unity module loads
//!
//! Rust counterpart of the read-only `sdk_unity_helpers` plugin (ticket 21; real
//! load chain ticket 56). The behavior lives in
//! `rudroid_android_addon_after_unity_module`; this crate only states which
//! library, log tag and module event it owns.

use std::ffi::c_int;

use rudroid_android_addon_after_unity_module::{self as after_unity, GuestModule};
use rudroid_common::host_api::HostApiV0;
use rudroid_common::ModuleKind;

pub use after_unity::{last_outcome, reset_state_for_tests, MappingOutcome};

rudroid_common::declare_descriptor!(
    "sdk_unity_helpers",
    rudroid_common::Kind::AndroidAddon,
    package = None::<&str>,
    requires = Some("android_base,android_elf_load")
);

/// The guest library this addon maps after the Unity module loads.
pub static MODULE: GuestModule = GuestModule {
    library: "lib/arm64-v8a/libUnityHelpers_Android.so",
    log_tag: "UNITYHELP",
    kind: ModuleKind::UnityHelpers,
    label: "unity_helpers",
};

#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    after_unity::plugin_init_raw(api, &MODULE)
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
