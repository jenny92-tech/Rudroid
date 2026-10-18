//! INPUT:  system SDL2, startup_brand, window/native-buffer selection and optional graphics backends
//! OUTPUT:  `SdlSession` window with GLES context or Vulkan preflight, report and typed errors
//! POS:    owns the handheld fullscreen drawable and reports its real size to guest geometry resolution
//!
//! Real SDL window + GLES context with an observable report. Only built
//! when the `sdl` feature is on (system SDL2 via sdl2-config); every
//! failure mode returns an explicit [`SdlError`], never a panic.

use std::ffi::{c_void, CStr, CString};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use rudroid_host_config::HostConfig;

use super::WindowConfig;
use super::startup_brand;

/// Raw GL enum values used by the minimal verification pipeline; the GL
/// functions are resolved through SDL's proc address, mirroring the
/// reference `getProc`.
mod gl {
    pub const VERSION: u32 = 0x1F02;
    pub const RENDERER: u32 = 0x1F01;
    pub const VENDOR: u32 = 0x1F00;
    pub const COLOR_BUFFER_BIT: u32 = 0x4000;
    pub const ARRAY_BUFFER: u32 = 0x8892;
    pub const STATIC_DRAW: u32 = 0x88E4;
    pub const FLOAT: u32 = 0x1406;
    pub const TRIANGLES: u32 = 0x0004;
    pub const RGBA: u32 = 0x1908;
    pub const UNSIGNED_BYTE: u32 = 0x1401;
    pub const VERTEX_SHADER: u32 = 0x8B31;
    pub const FRAGMENT_SHADER: u32 = 0x8B30;
    pub const COMPILE_STATUS: u32 = 0x8B81;
    pub const LINK_STATUS: u32 = 0x8B82;
    pub const TEXTURE_2D: u32 = 0x0DE1;
    pub const TEXTURE0: u32 = 0x84C0;
    pub const TEXTURE_MIN_FILTER: u32 = 0x2801;
    pub const TEXTURE_MAG_FILTER: u32 = 0x2800;
    pub const TEXTURE_WRAP_S: u32 = 0x2802;
    pub const TEXTURE_WRAP_T: u32 = 0x2803;
    pub const LINEAR: i32 = 0x2601;
    pub const CLAMP_TO_EDGE: i32 = 0x812F;
    /// SDL_GL_CONTEXT_PROFILE_ES value.
    pub const PROFILE_ES: i32 = 0x0004;
}

/// Internal clear color and the neutral loading-frame background. The
/// diagnostic clear is never presented; corner readbacks remain outside
/// the mascot and wordmark so artwork changes cannot affect verification.
const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FOREGROUND: [f32; 4] = [0.035, 0.047, 0.063, 1.0];

fn to_byte(value: f32) -> u8 {
    (value * 255.0).round() as u8
}

/// Keep the removed RGBA8/depth24/stencil8 FBO's format contract on the native buffer.
/// This session API owns SDL's default request state; it accepts geometry/version,
/// not external pixel-format overrides. Reset requests on every exit, not live contexts.
struct NativeBackbufferFormat<'a> {
    _video: &'a sdl2::VideoSubsystem,
}
impl<'a> NativeBackbufferFormat<'a> {
    fn select(video: &'a sdl2::VideoSubsystem) -> Self {
        let attr = video.gl_attr();
        attr.set_red_size(8); attr.set_green_size(8); attr.set_blue_size(8); attr.set_alpha_size(8);
        attr.set_depth_size(24); attr.set_stencil_size(8);
        Self { _video: video }
    }
}
impl Drop for NativeBackbufferFormat<'_> {
    fn drop(&mut self) {
        // SAFETY: creator-owned startup, borrowed video subsystem still live;
        // resets SDL request values only, without destroying or accessing a GL context.
        unsafe { sdl2::sys::SDL_GL_ResetAttributes() };
    }
}

/// The SDL2 0.37 wrapper takes a raw pointer to `mode.to_ll()` in a match arm.
/// Keep the actual C layout named across the call instead of lending that temporary.
fn set_native_display_mode(
    window: &sdl2::video::Window, desktop: sdl2::video::DisplayMode, width: i32, height: i32,
) -> Result<(), SdlError> {
    let mode = sdl2::sys::SDL_DisplayMode {
        format: desktop.format as u32, w: width, h: height,
        refresh_rate: desktop.refresh_rate, driverdata: std::ptr::null_mut(),
    };
    // SAFETY: creator-owned live window; this named initialized C struct remains
    // alive until SDL finishes copying it. No pointer or temporary survives the call.
    let result = unsafe { sdl2::sys::SDL_SetWindowDisplayMode(window.raw(), &mode) };
    if result < 0 { Err(SdlError::Window(sdl2::get_error())) } else { Ok(()) }
}

/// Creator-owned SDL window and graphics state, pinned for process life by
/// successful loader runs. Vulkan preflight does not draw a verification frame.
pub struct SdlSession {
    _context: Option<sdl2::video::GLContext>,
    #[cfg(all(feature = "vulkan", target_os = "linux"))]
    vulkan_window: Option<super::vulkan::NativeWindow>,
    #[cfg(all(feature = "vulkan", target_os = "linux"))]
    guest_vulkan: Option<Arc<super::vulkan::Graphics>>,
    #[cfg(all(feature = "vulkan", target_os = "linux"))]
    vulkan_render_attempted: bool,
    /// Pinned so the GL context stays valid; never read after creation.
    _window: sdl2::video::Window,
    _sdl: sdl2::Sdl,
    report: SdlSessionReport,
    #[cfg(target_os = "linux")]
    render: Option<rudroid_common::display_dimensions::RenderDimensions>,
    #[cfg(target_os = "linux")]
    guest_graphics: Option<Arc<super::guest_graphics::Graphics>>,
    #[cfg(target_os = "linux")]
    creator: std::thread::ThreadId,
}

impl SdlSession {
    #[cfg(target_os = "linux")]
    pub(crate) fn configure_render(&mut self,width:i32,height:i32,sharp:i32)->Result<(),String> {
        #[cfg(feature = "vulkan")]
        if self.guest_vulkan.is_some() { return Err("render configuration must precede Vulkan activation".into()); }
        #[cfg(feature = "vulkan")]
        if self.vulkan_window.is_some() && self.vulkan_render_attempted {
            return Err("Vulkan render configuration is immutable; recreate the session to retry or change modes".into());
        }
        if self.creator!=std::thread::current().id() || self.guest_graphics.is_some() {
            return Err("render configuration requires creator-thread access before guest activation".into());
        }
        if !matches!(sharp,0|1) {return Err("render sharp flag must be 0 or 1".into());}
        let dimensions = rudroid_common::display_dimensions::RenderDimensions::resolve(
            i64::from(self.report.window_w),i64::from(self.report.window_h),i64::from(width),i64::from(height),100,sharp!=0)
            .map_err(str::to_owned)?;
        #[cfg(feature = "vulkan")]
        if self.vulkan_window.is_some() {
            // A failed reconfiguration may have changed the native window.
            // Never retain geometry that no longer describes its live buffer.
            self.render=None;
            self.vulkan_render_attempted=true;
            self.configure_vulkan_render(dimensions)?;
        }
        self.render=Some(dimensions);
        Ok(())
    }

    #[cfg(all(feature = "vulkan", target_os = "linux"))]
    fn configure_vulkan_render(&mut self, dimensions: rudroid_common::display_dimensions::RenderDimensions) -> Result<(), String> {
        if dimensions.scaled() {
            if dimensions.sharp { return Err("Vulkan render scaling requires its explicit linear filter".into()); }
            if !(1..=16384).contains(&dimensions.width) || !(1..=16384).contains(&dimensions.height) {
                return Err("Vulkan native render dimensions must be in 1..=16384".into());
            }
            if sdl2::hint::get("SDL_VIDEO_WAYLAND_MODE_EMULATION").as_deref() != Some("1") {
                return Err("Vulkan scaling requires window-local Wayland mode emulation".into());
            }
            let video = self._window.subsystem();
            let desktop = video.desktop_display_mode(self._window.display_index()?)?;
            set_native_display_mode(&self._window, desktop, dimensions.width, dimensions.height).map_err(|e|e.to_string())?;
            self._window.set_fullscreen(sdl2::video::FullscreenType::True)?;
        }
        let mut events = self._sdl.event_pump()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        loop {
            events.pump_events();
            let pixels = super::vulkan::pixel_extent(&self._window)?;
            // Before the first GPU buffer commit SDL may still report the
            // closest logical desktop mode. Its physical pixel extent is the
            // backing-buffer contract; Wayland already transforms input.
            if pixels == (dimensions.width, dimensions.height) { break; }
            if std::time::Instant::now() >= deadline {
                return Err(format!("Vulkan native render size did not settle: requested={}x{}, pixels={pixels:?}, window={:?}", dimensions.width, dimensions.height, self._window.size()));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if Some(super::vulkan::NativeWindow::from_window(&self._window)?) != self.vulkan_window {
            return Err("SDL replaced the Vulkan native surface during scaling".into());
        }
        eprintln!("[SDL] Vulkan render: output={}x{} buffer={}x{} filter=linear compositor_scale={} initial_sdl_window={:?}",
            dimensions.output_width, dimensions.output_height, dimensions.width, dimensions.height, dimensions.scaled(), self._window.size());
        Ok(())
    }
    /// Immutable creation report. GLES includes a verification frame; Vulkan
    /// reports queried hardware/surface support with zero game-frame evidence.
    pub fn report(&self) -> &SdlSessionReport {
        &self.report
    }

    /// # Safety
    /// The optional callback must remain valid and have no unmet call
    /// preconditions until guest callers stop. The caller must stop all
    /// guest calls and release their EGL bindings before dropping us.
    #[cfg(target_os = "linux")]
    pub(crate) unsafe fn activate_guest_graphics(
        &mut self,
        presented: Option<extern "C" fn()>,
    ) -> Result<(), String> {
        if self.creator != std::thread::current().id() {
            return Err("guest graphics activation must run on the SDL creating thread".into());
        }
        #[cfg(feature = "vulkan")]
        if let Some(native) = self.vulkan_window {
            if self.guest_vulkan.is_none() {
                let dimensions = self.render.ok_or("Vulkan requires configured render dimensions")?;
                // SAFETY: creator-owned live SDL resources and pinned callback;
                // the activation caller guarantees Vulkan object/guest quiescence.
                let graphics = unsafe { super::vulkan::Graphics::activate(native, dimensions, presented) }?;
                // SDL's Wayland viewport already maps pointer coordinates to
                // the emulated native mode. Mapping output→render again would
                // shrink input coordinates a second time.
                let input_dimensions = vulkan_input_dimensions(dimensions);
                if let Err(error) = super::jni_input::activate(&self._sdl, input_dimensions) {
                    graphics.shutdown();
                    return Err(error);
                }
                self.guest_vulkan = Some(graphics);
            }
            return Ok(());
        }
        if self.guest_graphics.is_none() {
            self._window
                .gl_make_current(self._context.as_ref().ok_or("GLES session has no context")?)
                .map_err(|e| e.to_string())?;
            // SAFETY: creating thread with the primary context just made
            // current; session lifetime and callback follow our contract.
            let graphics = unsafe { super::guest_graphics::Graphics::activate(presented,self.render) }?;
            if let Err(error) = super::jni_input::activate(
                &self._sdl,
                graphics.dimensions(),
            ) {
                // SAFETY: creating thread, session live, and no guest has
                // seen the service yet (activation is still failing).
                unsafe { graphics.shutdown() };
                return Err(error);
            }
            self.guest_graphics = Some(graphics);
        }
        Ok(())
    }
}

#[cfg(all(feature = "vulkan", target_os = "linux"))]
fn vulkan_input_dimensions(dimensions: rudroid_common::display_dimensions::RenderDimensions)
    -> rudroid_common::display_dimensions::RenderDimensions {
    rudroid_common::display_dimensions::RenderDimensions {
        output_width: dimensions.width, output_height: dimensions.height, ..dimensions
    }
}

#[cfg(all(test, feature = "vulkan", target_os = "linux"))]
#[path = "../tests/unit/vulkan_render.rs"]
mod vulkan_render_tests;

impl Drop for SdlSession {
    fn drop(&mut self) {
        #[cfg(all(feature = "vulkan", target_os = "linux"))]
        if let Some(graphics) = self.guest_vulkan.take() {
            super::jni_input::deactivate();
            graphics.shutdown();
        }
        #[cfg(target_os = "linux")]
        if let Some(graphics) = self.guest_graphics.take() {
            super::jni_input::deactivate();
            // SAFETY: this !Send session drops on its creating thread,
            // before its native fields drop. Activation/C ABI callers
            // guarantee all guest users stopped and released bindings.
            unsafe { graphics.shutdown() };
        }
    }
}

/// Observed GLES verification or Vulkan hardware/surface preflight facts.
/// Legacy GL field names preserve the C report ABI. Each immutable string has one
/// shared allocation: Rust snapshots own it, and C reports borrow it
/// while the session is live. The loader pins its session for process life.
#[derive(Debug, Clone)]
pub struct SdlSessionReport {
    pub window_w: u32,
    pub window_h: u32,
    /// Active SDL video driver ("x11" under Xvfb; never "dummy").
    pub video_driver: Arc<CStr>,
    /// Granted GLES context version or queried Vulkan physical-device API.
    pub major: u32,
    pub minor: u32,
    /// Granted GLES profile mask; zero for Vulkan.
    pub profile_mask: u32,
    /// Native graphics API version text.
    pub gl_version: Arc<CStr>,
    /// Vendor string returned by GL_VENDOR.
    pub gl_vendor: Arc<CStr>,
    /// Renderer string returned by GL_RENDERER (llvmpipe on the
    /// software stack).
    pub gl_renderer: Arc<CStr>,
    /// glGetString(GL_VERSION / GL_RENDERER / GL_VENDOR), in that order.
    pub gl_strings: [Arc<CStr>; 3],
    /// GLES startup verification swaps; zero for Vulkan preflight.
    pub swap_count: u32,
    /// GLES verification pixel proof; false for Vulkan preflight.
    pub readback_non_background: bool,
}

#[derive(Debug)]
pub enum SdlError {
    /// SDL could not initialize (no display service, driver missing).
    Init(String),
    /// The active SDL video driver cannot stand in for a display
    /// (`dummy`/`offscreen` must never pass as GLES verification).
    VideoDriver(String),
    Window(String),
    /// No ladder step produced a context (last SDL error attached).
    Context(String),
    /// A context was granted but it is not an ES context.
    NonEsContext(String),
    /// Shader/program failure or a GL error inside the pipeline.
    Gl(String),
    Vulkan(String),
}

impl std::fmt::Display for SdlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SdlError::Init(message) => {
                write!(f, "SDL init failed (no display service?): {message}")
            }
            SdlError::VideoDriver(name) => write!(
                f,
                "SDL video driver `{name}` cannot stand in for a display; \
                     GLES verification requires a real window (Xvfb is fine)"
            ),
            SdlError::Window(message) => {
                write!(f, "SDL window creation failed: {message}")
            }
            SdlError::Context(message) => write!(
                f,
                "no GLES context granted on any ladder step (ES 3.2/3.1/3.0/2.0): \
                     {message}"
            ),
            SdlError::NonEsContext(detail) => write!(
                f,
                "context granted but glGetString never produced an `OpenGL ES` \
                     version string ({detail}) — refusing to present a non-ES \
                     context as the guest surface"
            ),
            SdlError::Gl(stage) => write!(f, "GLES pipeline failed: {stage}"),
            SdlError::Vulkan(stage) => write!(f, "Vulkan initialization failed: {stage}"),
        }
    }
}

impl std::error::Error for SdlError {}

/// Vulkan preflight verifies the real Wayland surface and hardware query path.
/// It reports zero swaps/readback: only the guest's successful present proves
/// a game frame. It never claims the GLES startup verification was performed.
#[cfg(all(feature = "vulkan", target_os = "linux"))]
pub(crate) fn create_session_vulkan(width: i64, height: i64) -> Result<(SdlSession, SdlSessionReport), SdlError> {
    if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
        return Err(SdlError::Window(format!("invalid Vulkan window size {width}x{height}")));
    }
    let sdl = sdl2::init().map_err(SdlError::Init)?;
    // Window-local mode emulation uses SDL's existing Wayland viewport. The
    // 100% path remains fullscreen desktop; lower modes are selected only by
    // configure_render before any guest Vulkan object or input service exists.
    sdl2::hint::set("SDL_VIDEO_WAYLAND_MODE_EMULATION", "1");
    let video = sdl.video().map_err(SdlError::Window)?;
    let driver = video.current_video_driver();
    if driver != "wayland" { return Err(SdlError::Vulkan(format!("candidate supports Wayland, active SDL driver is {driver}"))); }
    let window = video.window("RUDROID", width as u32, height as u32).position_centered()
        .fullscreen_desktop().build().map_err(|e| SdlError::Window(e.to_string()))?;
    let (native, observed) = super::vulkan::NativeWindow::inspect(&window).map_err(SdlError::Vulkan)?;
    let (pixel_w, pixel_h) = super::vulkan::pixel_extent(&window).map_err(SdlError::Vulkan)?;
    if !(1..=16384).contains(&pixel_w) || !(1..=16384).contains(&pixel_h) {
        return Err(SdlError::Window("SDL returned no Vulkan window pixel extent".into()));
    }
    let text = |s: String| CString::new(s).map(Arc::<CStr>::from).map_err(|_| SdlError::Vulkan("driver text contains NUL".into()));
    let strings = [text(observed.version)?, text(observed.renderer)?, text(observed.vendor)?];
    let report = SdlSessionReport { window_w: pixel_w as u32, window_h: pixel_h as u32,
        video_driver: text(driver.into())?, major: observed.major, minor: observed.minor, profile_mask: 0,
        gl_version: strings[0].clone(), gl_renderer: strings[1].clone(), gl_vendor: strings[2].clone(),
        gl_strings: strings, swap_count: 0, readback_non_background: false };
    eprintln!("[SDL] Vulkan Wayland preflight: output={pixel_w}x{pixel_h}; guest presentation pending");
    let session = SdlSession { _context: None, _window: window, _sdl: sdl, report: report.clone(),
        render: None, guest_graphics: None, creator: std::thread::current().id(),
        vulkan_window: Some(native), guest_vulkan: None, vulkan_render_attempted: false };
    Ok((session, report))
}

/// Declare each GL prototype once, including its checked SDL resolver.
macro_rules! gl_table {
    ($($field:ident = $name:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// Minimal GL function table resolved through SDL (reference `getProc`).
        #[derive(Clone, Copy)]
        struct GlFns {
            $($field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        /// # Safety
        /// Requires a current GL context (SDL resolves names through it).
        unsafe fn load_gl_fns(video: &sdl2::VideoSubsystem) -> Result<GlFns, SdlError> {
            Ok(GlFns {
                $($field: {
                    let pointer = video.gl_get_proc_address($name);
                    if pointer.is_null() {
                        return Err(SdlError::Gl(concat!("SDL could not resolve ", $name).into()));
                    }
                    // SAFETY: the checked address has the GL prototype declared in this table.
                    unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn($($arg),*) $(-> $ret)?>(pointer) }
                },)*
            })
        }
    };
}

gl_table! {
    get_string = "glGetString": fn(u32) -> *const u8;
    get_error = "glGetError": fn() -> u32;
    viewport = "glViewport": fn(i32, i32, i32, i32);
    clear_color = "glClearColor": fn(f32, f32, f32, f32);
    clear = "glClear": fn(u32);
    create_shader = "glCreateShader": fn(u32) -> u32;
    shader_source = "glShaderSource": fn(u32, i32, *const *const u8, *const i32);
    compile_shader = "glCompileShader": fn(u32);
    get_shader_iv = "glGetShaderiv": fn(u32, u32, *mut i32);
    delete_shader = "glDeleteShader": fn(u32);
    create_program = "glCreateProgram": fn() -> u32;
    attach_shader = "glAttachShader": fn(u32, u32);
    link_program = "glLinkProgram": fn(u32);
    get_program_iv = "glGetProgramiv": fn(u32, u32, *mut i32);
    use_program = "glUseProgram": fn(u32);
    delete_program = "glDeleteProgram": fn(u32);
    gen_buffers = "glGenBuffers": fn(i32, *mut u32);
    bind_buffer = "glBindBuffer": fn(u32, u32);
    delete_buffers = "glDeleteBuffers": fn(i32, *const u32);
    buffer_data = "glBufferData": fn(u32, isize, *const c_void, u32);
    get_attrib_location = "glGetAttribLocation": fn(u32, *const u8) -> i32;
    vertex_attrib_pointer = "glVertexAttribPointer": fn(u32, i32, u32, u8, i32, *const c_void);
    enable_vertex_attrib_array = "glEnableVertexAttribArray": fn(u32);
    disable_vertex_attrib_array = "glDisableVertexAttribArray": fn(u32);
    draw_arrays = "glDrawArrays": fn(u32, i32, i32);
    read_pixels = "glReadPixels": fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    gen_textures = "glGenTextures": fn(i32, *mut u32);
    active_texture = "glActiveTexture": fn(u32);
    bind_texture = "glBindTexture": fn(u32, u32);
    tex_parameter_i = "glTexParameteri": fn(u32, u32, i32);
    tex_image_2d = "glTexImage2D": fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    delete_textures = "glDeleteTextures": fn(i32, *const u32);
}

/// Capture one GL string with its NUL terminator. All report views share
/// this owned snapshot; destroying its context cannot invalidate Rust copies.
fn gl_string(fns: &GlFns, name: u32) -> Arc<CStr> {
    // SAFETY: current context; GL strings are valid for its lifetime.
    let ptr = unsafe { (fns.get_string)(name) };
    if ptr.is_null() {
        Arc::from(c"(null)")
    } else {
        // SAFETY: GL returns a NUL-terminated string.
        let text = unsafe { CStr::from_ptr(ptr.cast()) };
        match text.to_str() {
            Ok(_) => Arc::from(text),
            Err(_) => Arc::from(c"(invalid utf8)"),
        }
    }
}

/// Compile one shader; explicit failure when the ES dialect is rejected
/// (a desktop context would reject it — that refusal is evidence).
// SAFETY: requires a current context; `fns` loaded from it.
unsafe fn compile_shader(fns: &GlFns, kind: u32, source: &str) -> Result<u32, SdlError> {
    let source_ptr = source.as_ptr();
    // The private verification sources are short fixed templates. GL
    // accepts an explicit byte length, so no NUL-terminated copy is needed.
    let source_len = source.len() as i32;
    // SAFETY: current context; the source pointer outlives the call.
    unsafe {
        let shader = (fns.create_shader)(kind);
        if shader == 0 {
            return Err(SdlError::Gl("glCreateShader returned 0".into()));
        }
        (fns.shader_source)(shader, 1, &source_ptr, &source_len);
        (fns.compile_shader)(shader);
        let mut compiled = 0;
        (fns.get_shader_iv)(shader, gl::COMPILE_STATUS, &mut compiled);
        if compiled == 0 {
            (fns.delete_shader)(shader);
            return Err(SdlError::Gl(format!(
                "{} shader compile failed: GLSL ES dialect rejected — the \
                     granted context is probably not an ES context",
                if kind == gl::VERTEX_SHADER {
                    "vertex"
                } else {
                    "fragment"
                }
            )));
        }
        Ok(shader)
    }
}

/// GLSL ES dialect matching the granted context version.
fn es_shader_directive(major: u32, minor: u32) -> &'static str {
    match (major, minor) {
        (3, 2) => "#version 320 es",
        (3, _) => "#version 300 es",
        _ => "#version 100",
    }
}

/// Own only the verification frame's temporary objects. This guard stays
/// inside draw_and_verify_frame, while its fresh context is current. It
/// does not cache or alter any guest-owned program, shader, buffer or texture.
struct VerificationObjects<'a> {
    fns: &'a GlFns,
    program: u32,
    vertex: u32,
    fragment: u32,
    buffer: u32,
    texture: u32,
    attribute: Option<u32>,
    program_in_use: bool,
}

impl Drop for VerificationObjects<'_> {
    fn drop(&mut self) {
        // SAFETY: the caller keeps the originating context current until
        // this guard drops. Unbind the program before deletion so it and
        // its attachments are released immediately, not just flagged.
        unsafe {
            if let Some(attribute) = self.attribute {
                (self.fns.disable_vertex_attrib_array)(attribute);
            }
            if self.buffer != 0 {
                (self.fns.bind_buffer)(gl::ARRAY_BUFFER, 0);
                // Deletion also detaches this buffer from the current
                // vertex-array state, releasing the attribute reference.
                (self.fns.delete_buffers)(1, &self.buffer);
            }
            if self.texture != 0 {
                (self.fns.bind_texture)(gl::TEXTURE_2D, 0);
                (self.fns.delete_textures)(1, &self.texture);
            }
            if self.program_in_use {
                (self.fns.use_program)(0);
            }
            if self.program != 0 {
                (self.fns.delete_program)(self.program);
            }
            if self.vertex != 0 {
                (self.fns.delete_shader)(self.vertex);
            }
            if self.fragment != 0 {
                (self.fns.delete_shader)(self.fragment);
            }
        }
    }
}

/// Create the window + GLES session from the Host Config and verify the
/// pipeline (ES identity, clear/draw/readback, one swap). Display
/// required; call only from the host bring-up path.
pub fn create_session(config: &HostConfig) -> Result<(SdlSession, SdlSessionReport), SdlError> {
    let window_config = WindowConfig::from_config(config);
    let (major, minor) = WindowConfig::gles_version(config);
    create_session_sized(window_config.width, window_config.height, major, minor)
}

/// Like [`create_session`], with explicit geometry/version (the loader
/// passes the Host Config values through the C API).
pub fn create_session_sized(
    width: i64,
    height: i64,
    major: u8,
    minor: u8,
) -> Result<(SdlSession, SdlSessionReport), SdlError> {
    create_session_impl(width, height, major, minor, false)
}

/// Explicit Wayland fullscreen mode emulation; the compositor scales this buffer.
/// Rejected backends/geometry return before any guest context becomes visible.
pub fn create_session_native_buffer(
    width: i64,
    height: i64,
    major: u8,
    minor: u8,
) -> Result<(SdlSession, SdlSessionReport), SdlError> {
    create_session_impl(width, height, major, minor, true)
}

fn create_session_impl(
    width: i64,
    height: i64,
    major: u8,
    minor: u8,
    native_buffer: bool,
) -> Result<(SdlSession, SdlSessionReport), SdlError> {
    if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
        return Err(SdlError::Window(format!(
            "invalid configured window geometry {width}x{height}"
        )));
    }
    #[cfg(target_os = "linux")]
    super::guest_graphics::pin_driver_libraries().map_err(SdlError::Gl)?;
    let sdl = sdl2::init().map_err(|e| SdlError::Init(e.to_string()))?;
    // A GLES profile alone may produce a GLX ES context on X11. Guest
    // EGL requires actual EGL handles, so request SDL's EGL backend.
    #[cfg(target_os = "linux")]
    sdl2::hint::set("SDL_VIDEO_X11_FORCE_EGL", "1");
    if native_buffer && !sdl2::hint::set("SDL_VIDEO_WAYLAND_MODE_EMULATION", "1") {
        return Err(SdlError::Window("Wayland mode emulation is disabled by a higher-priority SDL hint".into()));
    }
    let video = sdl.video().map_err(|e| SdlError::Window(e.to_string()))?;
    let driver = video.current_video_driver();
    if native_buffer && driver != "wayland" {
        return Err(SdlError::Window(format!("native buffer presentation requires Wayland; active SDL driver is {driver}")));
    }
    if matches!(driver, "dummy" | "offscreen") {
        return Err(SdlError::VideoDriver(driver.to_owned()));
    }
    let _native_format = native_buffer.then(|| NativeBackbufferFormat::select(&video));
    let gl_attr = video.gl_attr();
    gl_attr.set_context_profile(sdl2::video::GLProfile::GLES);

    // Reference ladder (platform_egl_sdl): try the configured version,
    // then fall back 3.2 -> 3.1 -> 3.0 -> 2.0, logging every rejection
    // with its SDL error exactly like the reference `bd_create_gles_
    // context` does.
    let ladder = WindowConfig::gles_ladder((major, minor));
    eprintln!(
        "[SDL] video driver: {driver}; context ladder: {}",
        ladder
            .iter()
            .map(|(a, b)| format!("ES {a}.{b}"))
            .collect::<Vec<_>>()
            .join(" -> ")
    );
    let mut builder = video.window("Rudroid", width as u32, height as u32);
    builder.opengl().position_centered();
    // Commit the emulated fullscreen mode before Wayland maps the surface.
    // Mapping a temporary windowed surface first can leave EGL without a drawable.
    if native_buffer { builder.hidden(); }
    // Match the reference's handheld fullscreen request. The compositor
    // owns rotation and may grant a different size from the requested mode.
    // X11 development sessions remain windowed for their display probes.
    if !native_buffer && matches!(driver, "wayland" | "kmsdrm" | "mali" | "rpi" | "vivante") {
        builder.fullscreen();
    }
    let mut window = builder
        .build()
        .map_err(|e| SdlError::Window(e.to_string()))?;
    if native_buffer {
        // Wayland emulates this mode for this window; never change a KMS mode or system driver.
        let desktop = video.desktop_display_mode(window.display_index().map_err(SdlError::Window)?)
            .map_err(SdlError::Window)?;
        set_native_display_mode(&window, desktop, width as i32, height as i32)?;
        window.set_fullscreen(sdl2::video::FullscreenType::True).map_err(SdlError::Window)?;
        window.show();
        eprintln!("[SDL] presentation=wayland_native desktop={}x{} native_request={width}x{height}; compositor owns scaling", desktop.w, desktop.h);
    }
    let mut last_error = String::new();
    let mut context = None;
    for &(step_major, step_minor) in &ladder {
        gl_attr.set_context_version(step_major, step_minor);
        match window.gl_create_context() {
            Ok(created) => {
                eprintln!("[SDL] context granted: ES {step_major}.{step_minor}");
                context = Some(created);
                break;
            }
            Err(err) => {
                eprintln!("[SDL] ES {step_major}.{step_minor} rejected: {err}");
                last_error = err.to_string();
            }
        }
    }
    let context = context.ok_or_else(|| SdlError::Context(last_error))?;
    window
        .gl_make_current(&context)
        .map_err(|e| SdlError::Context(e.to_string()))?;
    let (window_w, window_h) = window.size();
    let (drawable_w, drawable_h) = window.drawable_size();
    if !(1..=16384).contains(&drawable_w) || !(1..=16384).contains(&drawable_h) {
        return Err(SdlError::Window(format!(
            "invalid actual drawable geometry {drawable_w}x{drawable_h}"
        )));
    }
    eprintln!(
        "[SDL] geometry: requested={width}x{height} window={window_w}x{window_h} \
         drawable={drawable_w}x{drawable_h} fullscreen={:?}",
        window.fullscreen_state()
    );

    let granted = gl_attr.context_version();
    let profile = gl_attr.context_profile();
    let profile_mask = match profile {
        sdl2::video::GLProfile::GLES => gl::PROFILE_ES,
        _other => 0,
    };
    if profile_mask != gl::PROFILE_ES {
        return Err(SdlError::NonEsContext(format!(
            "SDL attributes granted profile mask {profile_mask:#x}, not ES"
        )));
    }
    if native_buffer && (gl_attr.red_size() < 8 || gl_attr.green_size() < 8
        || gl_attr.blue_size() < 8 || gl_attr.alpha_size() < 8
        || gl_attr.depth_size() < 24 || gl_attr.stencil_size() < 8) {
        return Err(SdlError::Gl("native buffer did not grant RGBA8/depth24/stencil8".into()));
    }

    // GL_VERSION alone establishes the ES version; vendor/renderer
    // remain independent diagnostics using their standard GL enums.
    // SAFETY: the context was made current above.
    let fns = unsafe { load_gl_fns(&video)? };
    let gl_strings = [
        gl_string(&fns, gl::VERSION),
        gl_string(&fns, gl::RENDERER),
        gl_string(&fns, gl::VENDOR),
    ];
    eprintln!(
        "[SDL] glGetString: VERSION={:?} RENDERER={:?} VENDOR={:?}",
        gl_strings[0], gl_strings[1], gl_strings[2]
    );
    let es_string = gl_strings[0]
        .to_str()
        .expect("GL snapshots contain valid UTF-8");
    if !es_string.starts_with("OpenGL ES ") {
        return Err(SdlError::NonEsContext(format!("GL_VERSION: {es_string}")));
    }
    if !es_string.starts_with(&format!("OpenGL ES {}.{} ", granted.0, granted.1)) {
        eprintln!(
            "[SDL] note: string {es_string:?} vs SDL attribute version {}.{}",
            granted.0, granted.1
        );
    }

    // One branded verification frame: the diagnostic clear stays internal;
    // only the dark background, mascot and RUDROID wordmark are presented.
    let swap_count = AtomicU32::new(0);
    // SAFETY: the context is still current and `fns` was loaded from it.
    let readback = unsafe { draw_and_verify_frame(&fns, &window, granted, &swap_count)? };

    if native_buffer {
        // Fullscreen configure events may follow the first buffer commit. Finish
        // their bounded startup transition before input sees the logical window.
        let mut events = sdl.event_pump().map_err(SdlError::Window)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        let expected = (width as u32, height as u32);
        loop {
            events.pump_events();
            if (drawable_w, drawable_h) == expected && window.size() == expected && window.drawable_size() == expected { break; }
            if std::time::Instant::now() >= deadline {
                return Err(SdlError::Window(format!(
                    "Wayland did not retain requested native buffer {width}x{height}: window={:?} drawable={:?}",
                    window.size(), window.drawable_size()
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        eprintln!("[SDL] native buffer settled: window={width}x{height} drawable={width}x{height}");
    }

    let report = SdlSessionReport {
        window_w: drawable_w,
        window_h: drawable_h,
        video_driver: Arc::from(CString::new(driver).expect("SDL driver has no embedded NUL")),
        major: granted.0 as u32,
        minor: granted.1 as u32,
        profile_mask: profile_mask as u32,
        swap_count: swap_count.load(Ordering::SeqCst),
        readback_non_background: readback,
        gl_version: Arc::clone(&gl_strings[0]),
        gl_vendor: Arc::clone(&gl_strings[2]),
        gl_renderer: Arc::clone(&gl_strings[1]),
        gl_strings,
    };
    eprintln!(
        "[SDL] session verified: window={}x{} driver={} GLES {}.{} profile=ES \
             swaps={} readback_non_background={}",
        report.window_w,
        report.window_h,
        report.video_driver.to_string_lossy(),
        report.major,
        report.minor,
        report.swap_count,
        report.readback_non_background
    );
    Ok((
        SdlSession {
            _context: Some(context),
            #[cfg(all(feature = "vulkan", target_os = "linux"))]
            vulkan_window: None,
            #[cfg(all(feature = "vulkan", target_os = "linux"))]
            guest_vulkan: None,
            #[cfg(all(feature = "vulkan", target_os = "linux"))]
            vulkan_render_attempted: false,
            _window: window,
            _sdl: sdl,
            report: report.clone(),
            #[cfg(target_os = "linux")]
            guest_graphics: None,
            #[cfg(target_os = "linux")]
            render: None,
            #[cfg(target_os = "linux")]
            creator: std::thread::current().id(),
        },
        report,
    ))
}

/// Clear, draw, read back a background-only pixel and swap once. Returns whether
/// the drawn pixel differs from the background. Explicit error when a
/// shader refuses to compile or GL reports an error state — a silent
/// "maybe it drew" is never a pass.
// SAFETY: requires a current context; `fns` loaded from it.
unsafe fn draw_and_verify_frame(
    fns: &GlFns,
    window: &sdl2::video::Window,
    granted: (u8, u8),
    swap_count: &AtomicU32,
) -> Result<bool, SdlError> {
    let directive = es_shader_directive(granted.0 as u32, granted.1 as u32);
    // One GLES2-compatible attribute and sampler; no font/image decoder,
    // animation, extra swap or delay is needed for the startup brand.
    let (input, vertex_output, fragment_input, output, declaration, sample) = if granted.0 >= 3 {
        ("in", "out", "in", "fragColor", "out vec4 fragColor;", "texture")
    } else {
        ("attribute", "varying", "varying", "gl_FragColor", "", "texture2D")
    };
    let vertex_source = format!("{directive}\n{input} vec4 a_pos;\n{vertex_output} mediump vec2 v_uv;\n\
        void main() {{ gl_Position = vec4(a_pos.xy, 0.0, 1.0); v_uv = a_pos.zw; }}\n");
    let fragment_source = format!("{directive}\nprecision mediump float;\n\
        {fragment_input} mediump vec2 v_uv;\nuniform sampler2D u_mascot;\n{declaration}\n\
        void main() {{\n\
            vec4 bg = vec4({:?}, {:?}, {:?}, {:?});\n\
            if (v_uv.x < -2.5) {{ {output} = vec4(0.56, 0.60, 0.68, 1.0); }}\n\
            else if (v_uv.x < -1.5) {{ {output} = vec4(0.92, 0.90, 0.98, 1.0); }}\n\
            else if (v_uv.x < -0.5) {{ {output} = bg; }}\n\
            else {{ vec4 cat = {sample}(u_mascot, v_uv); {output} = vec4(mix(bg.rgb, cat.rgb, cat.a), 1.0); }}\n\
        }}\n", FOREGROUND[0], FOREGROUND[1], FOREGROUND[2], FOREGROUND[3]);
    let (w, h) = window.drawable_size();
    let sample = ((w / 8) as i32, (h / 8) as i32);
    // SAFETY: current context; every call below goes through the table
    // resolved for it, with buffer/readback sizes derived from the window.
    unsafe {
        (fns.viewport)(0, 0, w as i32, h as i32);
        (fns.clear_color)(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], BACKGROUND[3]);
        (fns.clear)(gl::COLOR_BUFFER_BIT);

        let mut objects = VerificationObjects {
            fns,
            program: (fns.create_program)(),
            vertex: 0,
            fragment: 0,
            buffer: 0,
            texture: 0,
            attribute: None,
            program_in_use: false,
        };
        let program = objects.program;
        if program == 0 {
            return Err(SdlError::Gl("glCreateProgram returned 0".into()));
        }
        objects.vertex = compile_shader(fns, gl::VERTEX_SHADER, &vertex_source)?;
        objects.fragment = compile_shader(fns, gl::FRAGMENT_SHADER, &fragment_source)?;
        (fns.attach_shader)(program, objects.vertex);
        (fns.attach_shader)(program, objects.fragment);
        (fns.link_program)(program);
        let mut linked = 0;
        (fns.get_program_iv)(program, gl::LINK_STATUS, &mut linked);
        if linked == 0 {
            return Err(SdlError::Gl("program link failed".into()));
        }

        // Background sanity: before any draw the sampled pixel must equal
        // the clear color; a mismatch means the readback path lies.
        let mut background = [0u8; 4];
        (fns.read_pixels)(
            sample.0,
            sample.1,
            1,
            1,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            background.as_mut_ptr().cast(),
        );
        let expected = [
            to_byte(BACKGROUND[0]),
            to_byte(BACKGROUND[1]),
            to_byte(BACKGROUND[2]),
            to_byte(BACKGROUND[3]),
        ];
        if background != expected {
            return Err(SdlError::Gl(format!(
                "background readback {background:?} does not match the clear \
                     color {expected:?} — the readback path cannot be trusted"
            )));
        }

        // The immutable 300x300 RGBA8 asset has exactly the upload span,
        // and rows are four-byte aligned. This is a fresh context: no
        // guest PBO, unpack state or texture binding exists yet. The guard
        // releases the temporary texture even if upload/draw fails.
        (fns.active_texture)(gl::TEXTURE0);
        (fns.gen_textures)(1, &mut objects.texture);
        if objects.texture == 0 {
            return Err(SdlError::Gl("glGenTextures returned 0".into()));
        }
        (fns.bind_texture)(gl::TEXTURE_2D, objects.texture);
        (fns.tex_parameter_i)(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR);
        (fns.tex_parameter_i)(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR);
        (fns.tex_parameter_i)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
        (fns.tex_parameter_i)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
        (fns.tex_image_2d)(gl::TEXTURE_2D, 0, gl::RGBA as i32,
            startup_brand::MASCOT_SIDE, startup_brand::MASCOT_SIDE, 0,
            gl::RGBA, gl::UNSIGNED_BYTE, startup_brand::MASCOT.as_ptr().cast());

        // One interleaved batch, including the full-screen background.
        // Works across ES 2.0-3.2 without gl_VertexID or a VAO.
        let vertices = startup_brand::vertices(w, h);
        (fns.gen_buffers)(1, &mut objects.buffer);
        if objects.buffer == 0 {
            return Err(SdlError::Gl("glGenBuffers returned 0".into()));
        }
        (fns.bind_buffer)(gl::ARRAY_BUFFER, objects.buffer);
        (fns.buffer_data)(
            gl::ARRAY_BUFFER,
            std::mem::size_of_val(vertices.as_slice()) as isize,
            vertices.as_ptr().cast(),
            gl::STATIC_DRAW,
        );
        let location = (fns.get_attrib_location)(program, b"a_pos\0".as_ptr());
        if location < 0 {
            return Err(SdlError::Gl("attribute a_pos not found".into()));
        }
        (fns.vertex_attrib_pointer)(location as u32, 4, gl::FLOAT, 0, 0, std::ptr::null());
        (fns.enable_vertex_attrib_array)(location as u32);
        objects.attribute = Some(location as u32);

        (fns.use_program)(program);
        objects.program_in_use = true;
        (fns.draw_arrays)(gl::TRIANGLES, 0, vertices.len() as i32);

        let mut pixel = [0u8; 4];
        (fns.read_pixels)(
            sample.0,
            sample.1,
            1,
            1,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            pixel.as_mut_ptr().cast(),
        );
        let non_background = pixel.iter().zip(FOREGROUND).all(|(actual, expected)| {
            actual.abs_diff(to_byte(expected)) <= 2
        });
        eprintln!(
            "[SDL] readback sample={sample:?} rgba=({},{},{},{}) background={expected:?} \
                 non_background={non_background}",
            pixel[0], pixel[1], pixel[2], pixel[3]
        );

        let error = (fns.get_error)();
        if error != 0 {
            return Err(SdlError::Gl(format!("glGetError reported {error:#x}")));
        }

        window.gl_swap_window();
        swap_count.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "[SDL] SDL_GL_SwapWindow done ({} call)",
            swap_count.load(Ordering::SeqCst)
        );
        eprintln!("[SDL] startup brand={} (one frame, no animation or delay)", startup_brand::TITLE);
        Ok(non_background)
    }
}

#[cfg(all(test, feature = "sdl"))]
#[path = "../tests/unit/window_es_session_tests.rs"]
mod es_session_tests;
