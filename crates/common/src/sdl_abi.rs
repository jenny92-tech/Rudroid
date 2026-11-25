//! INPUT: SDL provider and loader C ABI contract
//! OUTPUT: dynamic symbol names and session report layout
//! POS: shared ABI for independently linked kit images
#![forbid(unsafe_code)]

use std::ffi::c_char;

pub mod symbols {
    pub const SESSION_CREATE: &str = "rudroid_sdl_session_create";
    pub const SESSION_CREATE_NATIVE_BUFFER: &str = "rudroid_sdl_session_create_native_buffer";
    pub const SESSION_CREATE_VULKAN: &str = "rudroid_sdl_session_create_vulkan";
    pub const GRAPHICS_FEATURES: &str = "rudroid_sdl_graphics_features";
    pub const SESSION_REPORT: &str = "rudroid_sdl_session_report";
    pub const SESSION_DESTROY: &str = "rudroid_sdl_session_destroy";
    pub const GUEST_ACTIVATE: &str = "rudroid_sdl_guest_graphics_activate";
    pub const RENDER_CONFIGURE: &str = "rudroid_sdl_render_configure";
    pub const AUDIO_POLICY_CONFIGURE: &str = "rudroid_sdl_audio_policy_configure";
    pub const GUEST_SYMBOL: &str = "rudroid_sdl_guest_symbol";
    // Ticket-55 audio POLICY pump (dummy/file sinks).
    pub const AUDIO_OPEN: &str = "rudroid_sdl_audio_open";
    pub const AUDIO_WRITE: &str = "rudroid_sdl_audio_write";
    pub const AUDIO_FRAMES: &str = "rudroid_sdl_audio_frames";
    pub const AUDIO_CALLS: &str = "rudroid_sdl_audio_calls";
    pub const AUDIO_DESTROY: &str = "rudroid_sdl_audio_destroy";
    pub const LAST_ERROR: &str = "rudroid_sdl_last_error";
    // Ticket-92 real audio DEVICE bridge (the deploy-kit dynamic
    // interface a guest-side producer binds).
    pub const AUDIO_DEVICE_OPEN: &str = "rudroid_sdl_audio_device_open";
    pub const AUDIO_DEVICE_SUBMIT: &str = "rudroid_sdl_audio_device_submit";
    pub const AUDIO_DEVICE_PAUSE_RESUME: &str = "rudroid_sdl_audio_device_pause_resume";
    pub const AUDIO_DEVICE_CLOSE: &str = "rudroid_sdl_audio_device_close";
    pub const AUDIO_DEVICE_RELEASE: &str = "rudroid_sdl_audio_device_release";
    pub const AUDIO_DEVICE_STATS: &str = "rudroid_sdl_audio_device_stats";
    pub const AUDIO_DEVICE_SPEC: &str = "rudroid_sdl_audio_device_spec";
}

/// Reported graphics backend identity in [`CRudroidSdlReport::graphics_api`].
pub const GRAPHICS_API_GLES: u32 = 1;
pub const GRAPHICS_API_VULKAN: u32 = 2;

/// C view of the session verification report. Strings borrow the
/// session's immutable cache and stay valid until session_destroy. The
/// loader pins its session, making these borrows process-lifetime there.
#[repr(C)]
pub struct CRudroidSdlReport {
    pub graphics_api: u32,
    pub window_w: u32,
    pub window_h: u32,
    pub major: u32,
    pub minor: u32,
    pub profile_mask: u32,
    pub swap_count: u32,
    pub readback_non_background: u32,
    pub video_driver: *const c_char,
    pub gl_vendor: *const c_char,
    pub gl_renderer: *const c_char,
    pub gl_version: *const c_char,
}
