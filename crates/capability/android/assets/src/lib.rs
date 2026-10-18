//! INPUT:  same-kit host resource service and private NDK asset ABI adapter
//! OUTPUT:  android_assets descriptor, initialization and native resource symbols
//! POS:    bridges extracted assets to Android callers without copying whole resource packs
#![deny(unsafe_code)]
#[allow(unsafe_code)]
mod ffi;

rudroid_common::declare_descriptor!(
    "android_assets",
    rudroid_common::Kind::Runtime,
    package = None::<&str>,
    requires = Some("android_elf_load")
);

#[allow(unsafe_code)] // Standardized plugin export; pointer operations stay in ffi.rs.
#[no_mangle]
pub extern "C" fn rudroid_plugin_init(
    api: Option<&'static rudroid_common::host_api::HostApiV0>,
) -> i32 {
    let Some(api) = api.filter(|a| a.abi_version == rudroid_common::host_api::HOST_API_VERSION)
    else {
        return -1;
    };
    ffi::initialize(api);
    api.log_line(
        c"Assets",
        "NDK assets ready: owned file handles, shared resolver, lazy read-only file mapping",
    );
    0
}
