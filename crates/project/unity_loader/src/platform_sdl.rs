//! INPUT:  pinned platform_sdl_runtime/ELF capabilities, HostConfig presentation and audio policy
//! OUTPUT:  DisplayBackend adapter, native SDL session ownership and legacy symbol names
//! POS:    confines SDL C ABI, native-buffer fallback and EGL/ELF wiring outside generic boot and runtime registry code
#![deny(unsafe_op_in_unsafe_fn)]
use crate::{
    host_platform::{DisplayBackend, DisplaySession, HostSessionReport},
    runtime, LoadedCapability,
};
use rudroid_common::{display_dimensions::RenderDimensions, registry::RudroidAudioPolicy};
use rudroid_host_config::{HostConfig, PresentationBackend, GraphicsApi};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CStr};

// ---------------------------------------------------------------- host SDL session (ticket 55)

/// Loader-side mirror of the deployed `platform_sdl_runtime` C API names.
/// The capability owns the authoritative constants (`rudroid_linux_sdl_
/// runtime::c_api::symbols`); a dev-side parity test pins the two copies so
/// a silent drift can never happen. The loader deliberately does not link
/// the capability crate (ticket 43: no static capability links).
pub mod api {
    pub const SESSION_CREATE: &str = "rudroid_sdl_session_create";
    pub const SESSION_CREATE_NATIVE_BUFFER: &str = "rudroid_sdl_session_create_native_buffer";
    pub const SESSION_CREATE_VULKAN: &str = "rudroid_sdl_session_create_vulkan";
    pub const GRAPHICS_FEATURES: &str = "rudroid_sdl_graphics_features";
    pub const SESSION_REPORT: &str = "rudroid_sdl_session_report";
    pub const SESSION_DESTROY: &str = "rudroid_sdl_session_destroy";
    pub const LAST_ERROR: &str = "rudroid_sdl_last_error";
    pub const GUEST_ACTIVATE: &str = "rudroid_sdl_guest_graphics_activate";
    pub const RENDER_CONFIGURE: &str = "rudroid_sdl_render_configure";
    pub const AUDIO_POLICY_CONFIGURE: &str = "rudroid_sdl_audio_policy_configure";
    pub const GUEST_SYMBOL: &str = "rudroid_sdl_guest_symbol";
}

/// C mirror of the capability's `CRudroidSdlReport` (field order must not
/// drift; the real-session test verifies the loader read through this mirror
/// against the capability's own view).
#[repr(C)]
struct CSdlReport {
    window_w: u32,
    window_h: u32,
    major: u32,
    minor: u32,
    profile_mask: u32,
    swap_count: u32,
    readback_non_background: u32,
    video_driver: *const c_char,
    gl_vendor: *const c_char,
    gl_renderer: *const c_char,
    gl_version: *const c_char,
}

type CreateSession = unsafe extern "C" fn(i64, i64, i32, i32) -> *mut c_void;

struct SdlSessionApi {
    graphics_features: extern "C" fn() -> u64,
    audio_policy: extern "C" fn(Option<&RudroidAudioPolicy>) -> c_int,
    create: CreateSession,
    create_native_buffer: Option<CreateSession>,
    create_vulkan: Option<CreateSession>,
    report: unsafe extern "C" fn(*const c_void, *mut CSdlReport) -> c_int,
    last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
    destroy: unsafe extern "C" fn(*mut c_void),
}

/// The adapter retains the session until activation succeeds. A successful
/// session and its callbacks stay process-pinned; failures release it once.
pub(crate) struct Session<'a> {
    handle: *mut c_void,
    api: SdlSessionApi,
    capabilities: &'a [LoadedCapability],
    library: &'a Library,
    graphics_api: GraphicsApi,
}
impl Drop for Session<'_> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: this adapter owns a live SDL session; a failed bootstrap
            // has not started guest threads. pin clears the handle on success.
            unsafe { (self.api.destroy)(self.handle) };
        }
    }
}

/// Wire the existing window into ELF imports before any guest mapping. No
/// capability is statically linked; both libraries and the session stay pinned.
fn install_guest_graphics(
    capabilities: &[super::LoadedCapability],
    sdl: &Library,
    handle: *mut c_void,
    last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
    geometry: rudroid_common::display_dimensions::RenderDimensions,
) -> Result<(), String> {
    let Some(elf) = capabilities
        .iter()
        .find(|c| c.descriptor.id == "android_elf_load")
    else {
        return Ok(());
    };
    type Source = unsafe extern "C" fn(*const c_char, *mut c_void) -> usize;
    // SAFETY: both handles are validated, process-pinned same-kit members.
    // The session is owned by this adapter on its creating thread, before
    // guest execution. Exact C prototypes match the providers; registration
    // is revoked if activation fails, before the session can be destroyed.
    unsafe {
        let configure: libloading::Symbol<
            unsafe extern "C" fn(*mut c_void, i32, i32, i32) -> c_int,
        > = sdl
            .get(api::RENDER_CONFIGURE.as_bytes())
            .map_err(|e| e.to_string())?;
        if configure(
            handle,
            geometry.width,
            geometry.height,
            i32::from(geometry.sharp),
        ) != 0
        {
            return Err(fetch_sdl_last_error(last_error)
                .unwrap_or_else(|| "guest render configuration failed".into()));
        }
        let source: libloading::Symbol<Source> = sdl
            .get(api::GUEST_SYMBOL.as_bytes())
            .map_err(|e| e.to_string())?;
        let activate: libloading::Symbol<
            unsafe extern "C" fn(*mut c_void, Option<extern "C" fn()>) -> c_int,
        > = sdl
            .get(api::GUEST_ACTIVATE.as_bytes())
            .map_err(|e| e.to_string())?;
        let register: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, Source, *mut c_void) -> c_int,
        > = elf
            .library()
            .get(rudroid_common::symbols::elf_exec::SOURCE_REGISTER.as_bytes())
            .map_err(|e| e.to_string())?;
        let unregister: libloading::Symbol<unsafe extern "C" fn(c_int) -> c_int> = elf
            .library()
            .get(rudroid_common::symbols::elf_exec::SOURCE_UNREGISTER.as_bytes())
            .map_err(|e| e.to_string())?;
        let id = register(
            c"sdl-guest-graphics".as_ptr(),
            *source,
            std::ptr::null_mut(),
        );
        if id <= 0 {
            return Err("ELF rejected the SDL guest graphics source".into());
        }
        let presented = runtime::registered_android_hooks().and_then(|hooks| hooks.signal_vsync);
        if activate(handle, presented) != 0 {
            unregister(id);
            return Err(fetch_sdl_last_error(last_error)
                .unwrap_or_else(|| "could not activate the SDL guest EGL/GLES service".into()));
        }
    }
    eprintln!("[SDL] guest EGL/GLES source registered before guest mapping");
    Ok(())
}

/// # Safety
/// `library` must be the pinned handle of the platform_sdl_runtime member.
unsafe fn resolve_sdl_api(library: &Library) -> Result<SdlSessionApi, String> {
    // SAFETY: dlsym on the pinned capability handle; call shapes mirror the
    // capability's documented C API.
    unsafe {
        let create: libloading::Symbol<CreateSession> =
            library
                .get(api::SESSION_CREATE.as_bytes())
                .map_err(|e| format!("misses `{}`: {e}", api::SESSION_CREATE))?;
        let report: libloading::Symbol<
            unsafe extern "C" fn(*const c_void, *mut CSdlReport) -> c_int,
        > = library
            .get(api::SESSION_REPORT.as_bytes())
            .map_err(|e| format!("misses `{}`: {e}", api::SESSION_REPORT))?;
        let last_error: libloading::Symbol<unsafe extern "C" fn(*mut c_char, usize) -> usize> =
            library
                .get(api::LAST_ERROR.as_bytes())
                .map_err(|e| format!("misses `{}`: {e}", api::LAST_ERROR))?;
        let destroy: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> = library
            .get(api::SESSION_DESTROY.as_bytes())
            .map_err(|e| format!("misses `{}`: {e}", api::SESSION_DESTROY))?;
        Ok(SdlSessionApi {
            graphics_features: *library.get::<extern "C" fn() -> u64>(api::GRAPHICS_FEATURES.as_bytes())
                .map_err(|e|e.to_string())?,
            audio_policy: *library
                .get::<extern "C" fn(Option<&RudroidAudioPolicy>) -> c_int>(
                    api::AUDIO_POLICY_CONFIGURE.as_bytes(),
                )
                .map_err(|e| e.to_string())?,
            create: *create,
            create_native_buffer: library
                .get::<CreateSession>(api::SESSION_CREATE_NATIVE_BUFFER.as_bytes())
                .ok().map(|entry| *entry),
            create_vulkan: library.get::<CreateSession>(api::SESSION_CREATE_VULKAN.as_bytes())
                .ok().map(|entry| *entry),
            report: *report,
            last_error: *last_error,
            destroy: *destroy,
        })
    }
}

/// Two-phase fetch of the capability's thread-local diagnostic (same
/// contract as the elf_load channel).
///
/// # Safety
/// `last_error` must point into the pinned capability library.
unsafe fn fetch_sdl_last_error(
    last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
) -> Option<String> {
    // SAFETY: the two-phase contract; the buffer is sized by the first call.
    unsafe {
        let need = last_error(std::ptr::null_mut(), 0);
        if need == 0 {
            return None;
        }
        let mut buffer = vec![0u8; need];
        last_error(buffer.as_mut_ptr() as *mut c_char, need);
        buffer.truncate(need.saturating_sub(1));
        String::from_utf8(buffer).ok()
    }
}

/// # Safety
/// `ptr` must be a NUL-terminated string owned by the pinned registrant.
unsafe fn c_string_lossy(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return "(null)".to_owned();
    }
    // SAFETY: caller guarantees validity for the read.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) struct Backend<'a> {
    capability: &'a LoadedCapability,
    capabilities: &'a [LoadedCapability],
}
impl<'a> Backend<'a> {
    fn api(&self) -> Result<SdlSessionApi, String> {
        // SAFETY: validated, process-pinned same-kit capability; this adapter
        // alone resolves its documented native prototypes.
        unsafe { resolve_sdl_api(self.capability.library()) }
    }
    pub(crate) fn find(capabilities: &'a [LoadedCapability]) -> Option<Self> {
        capabilities
            .iter()
            .find(|c| c.descriptor.id == "platform_sdl_runtime")
            .map(|capability| Self {
                capability,
                capabilities,
            })
    }
}
impl<'a> DisplayBackend for Backend<'a> {
    type Session = Session<'a>;
    fn id(&self) -> &str {
        &self.capability.descriptor.id
    }
    fn create(&self, config: &HostConfig) -> Result<Self::Session, String> {
        let api = self.api()?;
        // AudioOutput is a separate JNI service. This bundled SDL backend
        // applies its policy here; display adapters need no audio methods.
        if (api.audio_policy)(runtime::registered_audio_policy().as_ref()) != 0 {
            return Err("SDL rejected the registered audio output policy".into());
        }
        let device = config.device();
        let major = config.get_i64("gpu.glMajorVersionOverride", 3).clamp(1, 3) as i32;
        let minor = config.get_i64("gpu.glMinorVersionOverride", 2).clamp(0, 9) as i32;
        let presentation = config.presentation_backend().map_err(str::to_owned)?;
        let graphics_api = config.graphics_api().map_err(str::to_owned)?;
        if graphics_api == GraphicsApi::Vulkan && presentation != PresentationBackend::Internal {
            return Err("Vulkan uses its own native Wayland presentation; gpu.presentationBackend must be internal".into());
        }
        let requested = config.render_dimensions().map_err(str::to_owned)?;
        // SAFETY: pinned exact native ABI; a non-null result is adopted below.
        let handle = unsafe {
            if graphics_api == GraphicsApi::Vulkan {
                api.create_vulkan.ok_or("deployed SDL backend has no Vulkan creation API")?
                    (device.display_width, device.display_height, 0, 0)
            } else if presentation == PresentationBackend::WaylandNative {
                let candidate = api.create_native_buffer.map_or(std::ptr::null_mut(), |create| {
                    create(i64::from(requested.width), i64::from(requested.height), major, minor)
                });
                if candidate.is_null() {
                    let reason = if api.create_native_buffer.is_some() {
                        fetch_sdl_last_error(api.last_error).unwrap_or_else(|| "no backend diagnostic".into())
                    } else { "backend has no native-buffer entry".into() };
                    eprintln!("[SDL] wayland_native unavailable ({reason}); retaining internal presentation at the same guest size");
                    (api.create)(device.display_width, device.display_height, major, minor)
                } else { candidate }
            } else {
                (api.create)(device.display_width, device.display_height, major, minor)
            }
        };
        if handle.is_null() {
            // SAFETY: the error function belongs to the same pinned backend.
            return Err(unsafe { fetch_sdl_last_error(api.last_error) }
                .unwrap_or_else(|| "session creation failed without a diagnostic".into()));
        }
        Ok(Session {
            handle,
            api,
            capabilities: self.capabilities,
            library: self.capability.library(),
            graphics_api,
        })
    }
}

/// Compiled support from the deployed runtime, before SDL/guest initialization.
/// A positive result is not proof of a compatible driver or game shaders.
pub fn graphics_feature_tokens(capabilities: &[LoadedCapability]) -> Result<String, String> {
    let backend = Backend::find(capabilities).ok_or("no supported display backend deployed")?;
    rudroid_common::graphics_features::tokens((backend.api()?.graphics_features)()).map_err(str::to_owned)
}

/// Cold, disposable Vulkan environment check. No phases, guest, activation or
/// presentation: the creating thread drops the session before returning.
pub fn probe_vulkan(capabilities: &[LoadedCapability]) -> Result<HostSessionReport, String> {
    let backend = Backend::find(capabilities).ok_or("no supported display backend deployed")?;
    let config = HostConfig::parse("[gpu]\ngraphicsApi = 'vulkan'\n").map_err(|e| e.to_string())?;
    let session = backend.create(&config)?;
    let report = session.report()?;
    if report.graphics_api != GraphicsApi::Vulkan || report.window_w == 0 || report.window_h == 0
        || report.major < 1 || (report.major == 1 && report.minor < 1)
        || report.profile_mask != 0 || report.swap_count != 0 || report.readback_non_background {
        return Err("invalid Vulkan environment preflight report".into());
    }
    Ok(report)
}
impl DisplaySession for Session<'_> {
    fn report(&self) -> Result<HostSessionReport, String> {
        let mut raw = CSdlReport {
            window_w: 0,
            window_h: 0,
            major: 0,
            minor: 0,
            profile_mask: 0,
            swap_count: 0,
            readback_non_background: 0,
            video_driver: std::ptr::null(),
            gl_vendor: std::ptr::null(),
            gl_renderer: std::ptr::null(),
            gl_version: std::ptr::null(),
        };
        // SAFETY: this adapter owns the live session and pinned report/error
        // functions. raw is writable, and every returned string borrows the
        // session cache; all four are copied before that borrow can end. No
        // native pointer escapes into the backend-neutral report.
        unsafe {
            if (self.api.report)(self.handle, &mut raw) != 0 {
                return Err(fetch_sdl_last_error(self.api.last_error)
                    .unwrap_or_else(|| "session report failed without a diagnostic".into()));
            }
            Ok(HostSessionReport {
                graphics_api: self.graphics_api,
                window_w: raw.window_w,
                window_h: raw.window_h,
                major: raw.major,
                minor: raw.minor,
                profile_mask: raw.profile_mask,
                swap_count: raw.swap_count,
                readback_non_background: raw.readback_non_background != 0,
                video_driver: c_string_lossy(raw.video_driver),
                gl_vendor: c_string_lossy(raw.gl_vendor),
                gl_renderer: c_string_lossy(raw.gl_renderer),
                gl_version: c_string_lossy(raw.gl_version),
            })
        }
    }
    fn activate(&mut self, geometry: RenderDimensions) -> Result<(), String> {
        install_guest_graphics(
            self.capabilities,
            self.library,
            self.handle,
            self.api.last_error,
            geometry,
        )
    }
    fn pin(mut self) {
        self.handle = std::ptr::null_mut();
    }
}
