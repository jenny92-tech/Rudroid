//! INPUT: validated paths from the Unity loader
//! OUTPUT: same-kit argument layout and named Mono service ABI
//! POS: the Mono owner pins the borrowed paths' contents before starting Unity
#![deny(unsafe_op_in_unsafe_fn)]

use rudroid_common::host_api::{HostApiV0, ServiceContract};
use rudroid_common::registry::{HostStr, RudroidGuestPlanBuilder};
use std::ffi::{c_int, CStr};

pub const FRAME_PACING_ANDROID: u32 = 0;
pub const FRAME_PACING_OBSERVE: u32 = 1;
pub const FRAME_PACING_DESKTOP: u32 = 2;
pub const MIN_DISPLAY_REFRESH_HZ: u32 = 24;
pub const MAX_DISPLAY_REFRESH_HZ: u32 = 360;

pub const MONO_SERVICE_NAME: &CStr = c"rudroid.unity.mono.v1";
pub const MONO_SERVICE_ABI: u32 = 1;

#[repr(C)]
pub struct MonoStartup<'a> {
    pub bridge: HostStr<'a>,
    pub runtime: HostStr<'a>,
    pub managed_dir: HostStr<'a>,
    pub guest_lib_dir: HostStr<'a>,
    pub unity_version: HostStr<'a>,
    pub plugins_map: HostStr<'a>,
    pub frame_pacing: u32,
    pub display_refresh_hz: u32,
}

/// Named Mono service published by `unity_mono` at Init.
#[repr(C)]
pub struct MonoServiceV1 {
    pub abi_version: u32,
    pub validate_config: Option<extern "C" fn(Option<&'static HostApiV0>) -> c_int>,
    pub add_reserved_paths:
        Option<extern "C" fn(Option<&'static HostApiV0>, Option<&RudroidGuestPlanBuilder>) -> c_int>,
    pub start_bridge: Option<extern "C" fn(Option<&'static HostApiV0>) -> c_int>,
}

// SAFETY: MonoServiceV1 is a stable POD registered only by unity_mono under
// MONO_SERVICE_NAME at MONO_SERVICE_ABI for process life.
unsafe impl ServiceContract for MonoServiceV1 {
    const NAME: &'static CStr = MONO_SERVICE_NAME;
    const ABI_VERSION: u32 = MONO_SERVICE_ABI;
}
