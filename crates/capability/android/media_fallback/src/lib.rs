//! INPUT:  same-kit HostApi logger and the private typed NDK media boundary
//! OUTPUT:  android_media_fallback descriptor and bare media symbol provider
//! POS:    optional unavailable-media policy; provides neither decoding nor a second platform service
use std::ffi::{c_char, c_void};

rudroid_common::declare_descriptor!(
    "android_media_fallback",
    rudroid_common::Kind::Runtime,
    package = None::<&str>,
    requires = Some("android_elf_load")
);

/// # Safety
/// Non-null names are valid C strings during the query. Returned data globals
/// contain a `const char*`, so callers must dereference once. Function pointers
/// require their exact NDK prototypes. Only this provider's live factory
/// handles may be deleted, exactly once and without concurrent use; no Rust
/// allocation is transferred. Caller owns fd/userdata/callbacks throughout:
/// failed setDataSource does not adopt, read, close or retain any of them.
#[no_mangle]
pub unsafe extern "C" fn rudroid_media_fallback_symbol(
    name: *const c_char,
    library: *const c_char,
    version: *const c_char,
    _user: *mut c_void,
) -> usize {
    if name.is_null() || !library.is_null() || !version.is_null() {
        return 0;
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        unsafe extern "C" { fn rudroid_media_symbol(name: *const c_char) -> usize; }
        rudroid_media_symbol(name)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
    { 0 }
}

#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static rudroid_common::host_api::HostApiV0>) -> i32 {
    let Some(api) = api.filter(|api| api.abi_version == rudroid_common::host_api::HOST_API_VERSION) else {
        return -1;
    };
    api.log_line(c"android_media_fallback",
        "NDK media decoder unavailable: source setup returns AMEDIA_ERROR_UNSUPPORTED; no video playback");
    0
}

#[cfg(all(test, target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/lib.rs"]
mod tests;
