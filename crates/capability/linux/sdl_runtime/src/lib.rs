//! INPUT:  HostConfig, dynamic Host API, system SDL2 and the shared JNI service contracts
//! OUTPUT:  platform_sdl_runtime descriptor, window/native-buffer/session, audio and guest graphics C APIs
//! POS:    owns the SDL display/audio/input backend behind independently selected compatibility services
//! Host window/GLES bring-up (ticket 55) and the audio policy pump, driven by
//! the Host Config `[device]` display block. Linux is the acceptance platform
//! (EGL/GLES-flavored); macOS can build this crate but the guest expects
//! EGL/GLES semantics, so macOS stays a development host — never the Silksong
//! acceptance target.
//!
//! Ticket 55 replaces the review findings on the old tickets 24/25:
//! - The GL context is requested as `GLProfile::GLES` (the baseline wrongly
//!   asked for Core desktop GL) with the reference version fallback ladder ES
//!   3.2 -> 3.1 -> 3.0 -> 2.0 (`platform_egl_sdl` policy). The granted
//!   context is verified functionally: the SDL profile attribute must report
//!   ES, `glGetString(GL_VERSION)` must yield an `OpenGL ES <maj>.<min>`
//!   string, the GLSL ES shader dialect must compile (a desktop
//!   context rejects it), and a drawn triangle must survive a `glReadPixels`
//!   readback with a non-background pixel before one real `SDL_GL_SwapWindow`.
//!   Dummy/offscreen SDL video drivers are rejected explicitly — they must
//!   never stand in for a display.
//! - The loader-facing surface is a C API ([`c_api`]): the loader's dynamic
//!   phase dlsyms `rudroid_sdl_session_create` on the deployed capability,
//!   exactly like the elf_load C API. Built without the `sdl` feature the
//!   session entries still exist and return an explicit last-error diagnostic
//!   — "SDL not compiled in" is a reported condition, never a missing symbol.
//! - The audio side pumps synthetic PCM under the dynamically registered
//!   `RudroidAudioPolicy` (ticket 45 registry, first-write-wins across deployed
//!   libraries — the old process-local `AudioPolicyRegistry` is gone) into a
//!   dummy (in-memory) or file sink. Sample vs frame semantics are explicit:
//!   buffers are sample counts, frames = samples / channels, an empty buffer
//!   never increases frames or sink calls, and a partial frame is rejected.
//!   Real SDL audio-device open stays out of this experiment — audibility is
//!   explicitly NOT claimed.
//!
//! Policy hooks (texture caps, GLES compat beyond the version ladder) stay
//! stubbed here on purpose: they land with the graphics policy tickets after
//! bring-up.

use rudroid_host_config::HostConfig;

rudroid_common::declare_descriptor!("platform_sdl_runtime", rudroid_common::Kind::Platform);

/// Display geometry the reference derives from the Host Config `[device]`
/// block (defaults 640x480, rotation 0 — reference parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowConfig {
    pub width: i64,
    pub height: i64,
    pub rotation: i64,
}

impl WindowConfig {
    pub fn from_config(config: &HostConfig) -> WindowConfig {
        let device = config.device();
        WindowConfig {
            width: device.display_width,
            height: device.display_height,
            rotation: device.display_rotation,
        }
    }

    /// Requested GLES context version for the guest: version override comes
    /// from the device config (`gpu.glMajorVersionOverride`) when present.
    pub fn gles_version(config: &HostConfig) -> (u8, u8) {
        let major = config.get_i64("gpu.glMajorVersionOverride", 3).clamp(1, 3) as u8;
        let minor = config.get_i64("gpu.glMinorVersionOverride", 2).clamp(0, 9) as u8;
        (major, minor)
    }

    /// The full context request ladder for a requested version: the request
    /// first, then the reference fallback steps (`platform_egl_sdl`: ES 3.2
    /// -> 3.1 -> 3.0 -> 2.0), deduplicated and never stepping above the
    /// request.
    pub fn gles_ladder(requested: (u8, u8)) -> Vec<(u8, u8)> {
        let mut ladder = vec![requested];
        for fallback in [(3u8, 1u8), (3, 0), (2, 0)] {
            if fallback.0 < requested.0 || (fallback.0 == requested.0 && fallback.1 < requested.1) {
                ladder.push(fallback);
            }
        }
        ladder
    }

    /// Config convenience wrapper over [`Self::gles_ladder`].
    pub fn gles_version_ladder(config: &HostConfig) -> Vec<(u8, u8)> {
        Self::gles_ladder(Self::gles_version(config))
    }
}

// Production kit audits reject images compiled with private probes.
#[cfg(feature = "diag-probes")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_diag_probes_marker() {}

/// Whether this build compiled the real SDL backend (the `sdl` feature).
pub const SDL_ENABLED: bool = cfg!(feature = "sdl");

/// Host audio output policy pump (ticket 55). The policy values come from the
/// ticket-45 dynamic registry (`RudroidAudioPolicy`, first-write-wins across
/// deployed libraries); the pump drives a dummy (in-memory) or file sink.
/// The pump is exercisable without any audio device: the OpenSL guest side
/// stays in the guest ELF world, and real SDL audio-device output remains
/// host bring-up follow-up work — audibility is explicitly not claimed.
pub mod audio;

#[cfg(feature = "sdl")]
pub mod window;
#[cfg(feature = "sdl")]
mod startup_brand;
#[cfg(all(feature = "sdl", target_os = "linux"))]
mod render_scale;

#[cfg(test)]
#[path = "../tests/unit/lib_window_config_tests.rs"]
mod window_config_tests;

/// Loader-facing C API of the capability (ticket 55). The loader's dynamic
/// phase dlsyms these on the deployed `platform_sdl_runtime` cdylib, exactly
/// like the elf_load C API. Session entries exist in EVERY build: without the
/// `sdl` feature they return null plus the explicit last-error diagnostic —
/// "SDL not compiled in" is a reported condition, never a missing symbol.
///
/// The last-error channel follows the elf_load two-phase contract:
/// `rudroid_sdl_last_error(buffer, capacity)` returns the needed length
/// including the NUL (0 when there is no message) and copies a truncated
/// NUL-terminated prefix when the buffer allows.
/// Ticket 92: real audio-device bridge (SDL callback device, bounded SPSC
/// ring, chain observability). Gated on the `sdl` feature like the window
/// session; the C API entries exist in both builds with explicit
/// diagnostics.
#[cfg(feature = "sdl")]
pub mod real_audio;

#[cfg(all(feature = "sdl", target_os = "linux"))]
mod guest_graphics;
#[cfg(all(feature = "sdl", target_os = "linux"))]
mod native_window;
#[cfg(all(feature = "sdl", target_os = "linux"))]
#[cfg(feature = "diag-probes")]
mod wayland_interval;
#[cfg(all(feature = "vulkan", target_os = "linux"))]
mod vulkan;

#[cfg(all(feature = "sdl", target_os = "linux"))]
mod invalidate_policy;

#[cfg(all(feature = "sdl", target_os = "linux"))]
mod gles_identity;
#[cfg(all(feature = "sdl", target_os = "linux"))]
mod gles_policy;
#[cfg(all(feature="sdl",target_os="linux"))]
mod shader_policy;
#[cfg(all(feature="sdl",target_os="linux"))]
mod shader_state;
#[cfg(all(feature="sdl",target_os="linux"))]
mod texture_policy;
#[cfg(all(feature="sdl",target_os="linux"))]
mod texture_state;

#[cfg(all(feature = "sdl", target_os = "linux"))]
mod jni_audio;
#[cfg(all(feature = "sdl", target_os = "linux"))]
mod jni_input;
#[cfg(all(feature="sdl",target_os="linux"))]
mod input_policy;
#[cfg(all(feature = "sdl", target_os = "linux"))]
mod render_thread;
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(
    api: Option<&'static rudroid_common::host_api::HostApiV0>,
) -> i32 {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    #[cfg(all(feature = "sdl", target_os = "linux"))]
    {
        let is_vulkan = api
            .config_string(c"gpu.graphicsApi", 32)
            .is_some_and(|v| v == "vulkan");
        if !is_vulkan {
            #[cfg(feature = "diag-probes")]
            if let Err(error) = wayland_interval::register(api) {
                rudroid_common::error!("[SDL] {error}");
                return -1;
            }
            if !invalidate_policy::configure(api) { return -1; }
            #[cfg(feature = "diag-probes")]
            if !guest_graphics::geometry_map::configure(api) { return -1; }
            #[cfg(feature = "diag-probes")]
            if let Err(error) = guest_graphics::geometry_trace::register(api) {
                rudroid_common::error!("[SDL] {error}");
                return -1;
            }
            if let Err(error)=gles_identity::configure(api) {rudroid_common::error!("[SDL] {error}");return -1;}
            if let Err(error)=gles_policy::configure(api) {rudroid_common::error!("[SDL] {error}");return -1;}
            shader_policy::configure(api);
            if let Err(error)=texture_policy::configure(api) {rudroid_common::error!("[SDL] {error}");return -1;}
        }
        render_thread::configure(api);
        if jni_audio::register(api) != 0 {
            return -1;
        }
        jni_input::register(api)
    }
    #[cfg(not(all(feature = "sdl", target_os = "linux")))]
    {
        0
    }
}

pub mod c_api;
