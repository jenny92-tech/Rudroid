//! INPUT:  the same-kit host resource service and existing extracted game layout
//! OUTPUT:  platform_android_asset_packs descriptor and early pack-policy registration
//! POS:    enables one host-owned read-only pack overlay before Java/NDK initialization
#![deny(unsafe_code)]
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};

rudroid_common::declare_descriptor!(
    "platform_android_asset_packs",
    rudroid_common::Kind::Platform
);

#[allow(unsafe_code)] // The standardized export is the ABI boundary; its body only calls the safe host method.
#[no_mangle]
pub extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> i32 {
    let Some(api) = api.filter(|api| api.abi_version == HOST_API_VERSION) else {
        return -1;
    };
    if api.enable_asset_packs() {
        0
    } else {
        -1
    }
}
