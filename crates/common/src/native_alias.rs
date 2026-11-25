//! INPUT: exact native aliases and a resolver from a launcher-selected capability
//! OUTPUT: same-kit C ABI for the existing guest libdl owner
//! POS: references are borrowed during registration, resolver and API live for the process
#![deny(unsafe_op_in_unsafe_fn)]

use crate::host_api::ServiceContract;
use crate::registry::HostStr;
use std::ffi::{c_int, c_void, CStr};

#[repr(C)]
pub struct NativeAlias<'a> {
    /// Basename, relative guest pathname, absolute guest pathname and the
    /// canonical native pathname. These spellings are matched byte-for-byte.
    pub names: [HostStr<'a>; 4],
    pub symbol: extern "C" fn(HostStr<'_>) -> usize,
}

pub const GUEST_LIBDL_SERVICE_NAME: &CStr = c"rudroid.elf.guest_libdl.v1";
pub const GUEST_LIBDL_SERVICE_ABI: u32 = 1;

#[repr(C)]
pub struct GuestLibdl {
    pub register_alias: extern "C" fn(&NativeAlias<'_>) -> c_int,
    pub open: extern "C" fn(HostStr<'_>, c_int) -> *mut c_void,
    pub lookup: extern "C" fn(*mut c_void, HostStr<'_>) -> *mut c_void,
}

// SAFETY: GuestLibdl is a stable POD registered only by android_elf_load under
// GUEST_LIBDL_SERVICE_NAME at GUEST_LIBDL_SERVICE_ABI for process life.
unsafe impl ServiceContract for GuestLibdl {
    const NAME: &'static CStr = GUEST_LIBDL_SERVICE_NAME;
    const ABI_VERSION: u32 = GUEST_LIBDL_SERVICE_ABI;
}
