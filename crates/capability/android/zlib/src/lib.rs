//! INPUT:  borrowed ELF symbol queries and the gated ARM64 native zlib C ABI
//! OUTPUT:  android_zlib descriptor and explicit bare-name compression source
//! POS:    shares native zlib state/buffers without Rust wrappers or another allocator
use std::ffi::{c_char, c_void};

rudroid_common::declare_descriptor!(
    "android_zlib", rudroid_common::Kind::Runtime,
    package = None::<&str>, requires = Some("android_elf_load")
);

/// # Safety
/// Non-null name is a borrowed valid C string. Returned addresses require the
/// exact LP64 zlib prototypes and live 112 B stream objects. Caller retains
/// buffers and callbacks; callbacks must remain callable through matching
/// End/Copy cleanup, before guest unmap. Never share a mutable stream across
/// concurrent calls or C-free its internal state; native zlib owns that state.
#[no_mangle]
pub unsafe extern "C" fn rudroid_zlib_symbol(
    name: *const c_char, library: *const c_char, version: *const c_char, _user: *mut c_void,
) -> usize {
    if name.is_null() || !library.is_null() || !version.is_null() { return 0; }
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    {
        unsafe extern "C" { fn rudroid_native_zlib_symbol(name: *const c_char) -> usize; }
        rudroid_native_zlib_symbol(name)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
    { 0 }
}

#[cfg(all(test, target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/lib.rs"]
mod tests;
