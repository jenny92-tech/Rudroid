//! INPUT:  version-aware ELF queries and Linux ARM64 GNU host libc services
//! OUTPUT:  android_bionic descriptor, attested native ABI sources and shared log installation
//! POS:    shares native heap, errno, I/O and synchronization across ABI facades

use std::ffi::{c_char, c_void};
#[cfg(test)]
static LIFECYCLE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// First: defines `symbol_provider!` for the provider modules below.
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[macro_use]
mod registry;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod basic;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod diagnostics;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod posix;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod paths;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod multibyte;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod destructors;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
pub use destructors::rudroid_bionic_finalize_range;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod stdio;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod sync;
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod threads;

rudroid_common::declare_descriptor!(
    "android_bionic",
    rudroid_common::Kind::Runtime,
    package = None::<&str>,
    requires = Some("android_elf_load")
);

/// Explicit C pointer/scalar APIs, stdio facades and guest destructors are
/// attested. Guest jump buffers use native assembly only within guest C
/// frames; never jump across a Rust ownership frame. Private TCB/ELF TLS
/// remain unsupported, and there is no generic native dlsym fallback. Signal handlers must be restored and signal
/// delivery quiescent before their guest image is unloaded.
/// Allocations use the process C heap; FILE data/buffers remain host-owned.
///
/// # Safety
/// Non-null arguments must be valid NUL-terminated strings during this call.
/// For bare queries both library and version must be null. Callers of a
/// returned pointer must use its matching C prototype and allocation rules;
/// free/realloc/usable_size accept only pointers from this native family.
/// FILE callers must use live facades from this provider, not host FILE*,
/// and exclude fclose/free/cookie mutation during I/O. Bionic private buffer
/// internals are unsupported. Android asset URIs resolve to extracted files;
/// ordinary paths keep host libc semantics.
#[no_mangle]
pub unsafe extern "C" fn rudroid_bionic_symbol(
    name: *const c_char,
    library: *const c_char,
    version: *const c_char,
    _user: *mut c_void,
) -> usize {
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        use std::ffi::CStr;
        if name.is_null() {
            return 0;
        }
        if library.is_null() != version.is_null() {
            return 0;
        }
        let symbol = CStr::from_ptr(name);
        let name = symbol.to_bytes();
        if library.is_null() {
            let log = (registry::LOGGING.lookup)(symbol);
            if log != 0 { return log; }
        }
        // libm has a separate SONAME/version contract. No libc struct API is
        // allowed through this branch, and numeric code uses its C prototype.
        if !library.is_null() && CStr::from_ptr(library) == c"libm.so" {
            return if CStr::from_ptr(version) == c"LIBC" {
                (registry::MATH.lookup)(symbol)
            } else { 0 };
        }
        // Android's libc.map.txt version nodes live in registry::VERSIONED.
        if !library.is_null()
            && (CStr::from_ptr(library) != c"libc.so"
                || CStr::from_ptr(version) != registry::required_version(name))
        {
            return 0;
        }
        if library.is_null() || name == b"ldexp" {
            let address = (registry::MATH.lookup)(symbol);
            if address != 0 { return address; }
        }
        registry::libc_symbol(symbol)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
    {
        let _ = (name, library, version);
        0
    }
}

/// Install the same process-pinned core log callback used by other facades.
#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static rudroid_common::host_api::HostApiV0>) -> std::ffi::c_int {
    let Some(api) = api.filter(|api| api.abi_version == rudroid_common::host_api::HOST_API_VERSION) else { return -1; };
    #[cfg(all(target_os="linux",target_arch="aarch64",target_env="gnu"))]
    paths::install(api);
    #[cfg(all(target_os="linux",target_arch="aarch64",target_env="gnu"))]
    {
        unsafe extern "C" { fn rudroid_bionic_install_logger(log: Option<rudroid_common::host_api::LogFn>); }
        // SAFETY: the loader's log entry stays callable for the process and
        // consumes (never retains) the strings liblog hands it.
        unsafe { rudroid_bionic_install_logger(Some(api.log)) };
    }
    #[cfg(not(all(target_os="linux",target_arch="aarch64",target_env="gnu")))]
    let _ = api;
    0
}

#[cfg(all(test, target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/lib.rs"]
mod tests;
