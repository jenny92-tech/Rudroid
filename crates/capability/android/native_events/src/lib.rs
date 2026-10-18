//! INPUT:  borrowed ELF symbol queries and Rust-owned Linux epoll/eventfd/pthread event module
//! OUTPUT:  android_native_events descriptor and strict bare-name NDK source
//! POS:    supplies real fd polling and explicit empty sensor availability without another worker
use std::ffi::{c_char, c_void};

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod events;

#[cfg(test)]
static TEST_FDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

rudroid_common::declare_descriptor!(
    "android_native_events", rudroid_common::Kind::Runtime,
    package = None::<&str>, requires = Some("android_elf_load")
);

/// Prepare the host/UI thread's one shared Looper before Unity JNI_OnLoad.
#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static rudroid_common::host_api::HostApiV0>) -> i32 {
    let Some(api)=api else { return -1; };
    if api.abi_version!=rudroid_common::host_api::HOST_API_VERSION { return -1; }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        // SAFETY: ALooper_prepare takes a plain flag and only touches this thread's TLS.
        if unsafe { events::prepare(1) }.is_null() { -1 } else { 0 }
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
    { -1 }
}

/// # Safety
/// Name is a borrowed valid C string. Use exact NDK prototypes. Prepare/forThread
/// borrow the thread-held Looper reference; acquire before sharing across threads
/// and release only acquired references. Caller owns monitored fds and callbacks.
/// Remove registrations and quiesce in-flight polls/callbacks before guest unmap;
/// join all threads before unloading the process-pinned native event capability.
#[no_mangle]
pub unsafe extern "C" fn rudroid_native_events_symbol(
    name: *const c_char, library: *const c_char, version: *const c_char, _user: *mut c_void,
) -> usize {
    if name.is_null() || !library.is_null() || !version.is_null() { return 0; }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        events::symbol(std::ffi::CStr::from_ptr(name).to_bytes())
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
    { 0 }
}

#[cfg(all(test, target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/lib.rs"]
mod tests;
