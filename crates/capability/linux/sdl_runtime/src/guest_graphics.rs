//! INPUT:  one live SDL GLES session and the native EGL driver
//! OUTPUT:  shared EGL/GLES/ANativeWindow source, SDL window presentation and worker EGL lifecycle
//! POS:    Android graphics entry points backed by the existing Linux window; no second window or driver state
//!
//! Linux ARM64 guest/host EGL and GLES use the same C ABI. GL calls return
//! driver pointers directly; static imports and eglGetProcAddress share a
//! bounded address cache. EGL owns only guest worker contexts/pbuffers;
//! scaled default-buffer wrappers stay on the owning primary context; the
//! physical window/context remain owned by SdlSession. Activation and
//! shutdown run on its creating thread, with all guest threads quiescent
//! at shutdown. Driver current bindings and EGL errors remain per-thread.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use rudroid_common::display_dimensions::RenderDimensions;
use super::render_scale::RenderScale;
#[path = "scaled_gl.rs"]
mod scaled_gl;
#[path="shader_gl.rs"]
mod shader_gl;
#[path="texture_gl.rs"]
mod texture_gl;
#[cfg(feature = "diag-probes")]
#[path = "geometry_map.rs"]
pub(super) mod geometry_map;
#[cfg(feature = "diag-probes")]
#[path = "geometry_trace.rs"]
pub(super) mod geometry_trace;
#[cfg(feature = "diag-probes")]
#[path = "geometry_census.rs"]
mod geometry_census;
#[cfg(feature = "diag-probes")]
#[path = "geometry_alias.rs"]
pub(super) mod geometry_alias;

type Handle = *mut c_void;
type Bool = u32;
const TRUE: Bool = 1;
const FALSE: Bool = 0;
const SUCCESS: i32 = 0x3000;
const NOT_INITIALIZED: i32 = 0x3001;
const BAD_CONFIG: i32 = 0x3005;
const BAD_CURRENT_SURFACE: i32 = 0x3007;
const BAD_DISPLAY: i32 = 0x3008;
const BAD_MATCH: i32 = 0x3009;
const BAD_PARAMETER: i32 = 0x300c;
const CONFIG_ID: i32 = 0x3028;
const NONE: i32 = 0x3038;
const DRAW: i32 = 0x3059;
const EXTENSIONS: i32 = 0x3055;
const WIDTH: i32 = 0x3057;
const HEIGHT: i32 = 0x3056;
const CACHE_LIMIT: usize = 2048;

static ACTIVE: Mutex<Weak<Graphics>> = Mutex::new(Weak::new());
static GENERATION: AtomicU64 = AtomicU64::new(1);
// Driver entry points and native TLS destructors can outlive SDL video.
// Pin one EGL and one GLES library for the process; window/GPU objects still drop.
static EGL_LIBRARY: OnceLock<Result<libloading::Library, String>> = OnceLock::new();
fn egl_library() -> Result<&'static libloading::Library, String> {
    EGL_LIBRARY
        .get_or_init(|| {
            // SAFETY: system EGL initializers are allowed; the library stays pinned for the process.
            unsafe {
                libloading::Library::new("libEGL.so.1")
                    .or_else(|_| libloading::Library::new("libEGL.so"))
            }
            .map_err(|e| format!("native EGL driver: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(crate) fn pin_driver_libraries() -> Result<(), String> {
    static GLES_LIBRARY: OnceLock<Result<libloading::Library, String>> = OnceLock::new();
    egl_library()?;
    GLES_LIBRARY
        .get_or_init(|| {
            // SAFETY: system GLES initializers are allowed; the library stays pinned for the process.
            unsafe {
                libloading::Library::new("libGLESv2.so.2")
                    .or_else(|_| libloading::Library::new("libGLESv2.so"))
            }
            .map_err(|e| format!("native GLES driver: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(())
}
thread_local! {
    static CACHED: RefCell<(u64, Weak<Graphics>)> = const { RefCell::new((0, Weak::new())) };
    static ERROR: Cell<i32> = const { Cell::new(SUCCESS) };
}

fn active() -> Option<Arc<Graphics>> {
    let generation = GENERATION.load(Ordering::Acquire);
    CACHED
        .try_with(|slot| {
            let mut cached = slot.borrow_mut();
            if cached.0 != generation {
                *cached = (
                    generation,
                    ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).clone(),
                );
            }
            cached.1.upgrade()
        })
        // A later guest TLS destructor may still call EGL after this cache
        // was destroyed. Read the same shared service without resurrecting
        // TLS or panicking across the native boundary. Normal hits stay local.
        .unwrap_or_else(|_| ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).upgrade())
        .filter(|graphics| graphics.live.load(Ordering::Acquire))
}
fn fail<T>(code: i32, result: T) -> T {
    if let Some(g) = active() {
        // SAFETY: eglGetError has no preconditions; it clears the native error.
        unsafe { (g.driver.error)() };
    }
    let _ = ERROR.try_with(|error| error.set(code));
    result
}

macro_rules! driver_table {
    ($($field:ident: $ty:ty => $name:literal),* $(,)?) => {
        struct Driver { $($field: $ty,)* }
        impl Driver {
            fn load() -> Result<Self, String> {
                let library = egl_library()?;
                Ok(Self { $( $field: {
                    let name = concat!($name, "\0");
                    // SAFETY: each field type is the EGL 1.5 prototype of its entry point.
                    *unsafe { library.get::<$ty>(name.as_bytes()) }.map_err(|e| format!("native EGL entry {}: {e}", $name))?
                }, )* })
            }
        }
    };
}
driver_table! {
    current_display: unsafe extern "C" fn() -> Handle => "eglGetCurrentDisplay",
    current_context: unsafe extern "C" fn() -> Handle => "eglGetCurrentContext",
    current_surface: unsafe extern "C" fn(i32) -> Handle => "eglGetCurrentSurface",
    query_context: unsafe extern "C" fn(Handle, Handle, i32, *mut i32) -> Bool => "eglQueryContext",
    choose_config: unsafe extern "C" fn(Handle, *const i32, *mut Handle, i32, *mut i32) -> Bool => "eglChooseConfig",
    config_attrib: unsafe extern "C" fn(Handle, Handle, i32, *mut i32) -> Bool => "eglGetConfigAttrib",
    initialize: unsafe extern "C" fn(Handle, *mut i32, *mut i32) -> Bool => "eglInitialize",
    create_context: unsafe extern "C" fn(Handle, Handle, Handle, *const i32) -> Handle => "eglCreateContext",
    destroy_context: unsafe extern "C" fn(Handle, Handle) -> Bool => "eglDestroyContext",
    create_pbuffer: unsafe extern "C" fn(Handle, Handle, *const i32) -> Handle => "eglCreatePbufferSurface",
    destroy_surface: unsafe extern "C" fn(Handle, Handle) -> Bool => "eglDestroySurface",
    make_current: unsafe extern "C" fn(Handle, Handle, Handle, Handle) -> Bool => "eglMakeCurrent",
    query_surface: unsafe extern "C" fn(Handle, Handle, i32, *mut i32) -> Bool => "eglQuerySurface",
    query_string: unsafe extern "C" fn(Handle, i32) -> *const c_char => "eglQueryString",
    surface_attrib: unsafe extern "C" fn(Handle, Handle, i32, i32) -> Bool => "eglSurfaceAttrib",
    swap: unsafe extern "C" fn(Handle, Handle) -> Bool => "eglSwapBuffers",
    interval: unsafe extern "C" fn(Handle, i32) -> Bool => "eglSwapInterval",
    error: unsafe extern "C" fn() -> i32 => "eglGetError",
    release_thread: unsafe extern "C" fn() -> Bool => "eglReleaseThread",
}

#[derive(Default)]
struct Resources {
    contexts: HashSet<usize>,
    retired_contexts: HashSet<usize>,
    surfaces: HashSet<usize>,
}

pub(crate) struct Graphics {
    driver: Driver,
    // Borrowed from SdlSession; the window outlives all guest calls. SDL owns
    // its native surface and, on KMSDRM, the GBM/DRM page-flip bookkeeping.
    window: usize,
    display: usize,
    context: usize,
    surface: usize,
    config: usize,
    // This SDL window is fixed-size. EGL and NDK facades read the same
    // authoritative size queried once from the existing native surface.
    width: i32,
    height: i32,
    dimensions: RenderDimensions,
    scale: Option<RenderScale>,
    #[cfg(feature = "diag-probes")]
    geometry_maps: Option<geometry_map::GeometryMaps>,
    #[cfg(feature = "diag-probes")]
    geometry_trace: Option<geometry_trace::Trace>,
    disable_invalidation: bool,
    primary_claimed: AtomicBool,
    live: AtomicBool,
    get_integer: usize,
    use_program: usize,
    addresses: Mutex<HashMap<Vec<u8>, usize>>,
    resources: Mutex<Resources>,
    extensions: CString,
    presented: Option<extern "C" fn()>,
    #[cfg(feature = "diag-probes")]
    interval_trace: Option<super::wayland_interval::Trace>,
    #[cfg(feature = "diag-probes")]
    capture: Option<FrameCapture>,
    capture_errors: Mutex<HashMap<usize,VecDeque<u32>>>,
    swaps: AtomicU64,
    diagnostic_records: AtomicU8,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Route {
    ElfSource,
    EglProc,
}

#[cfg(feature = "diag-probes")]
#[path = "frame_capture.rs"]
mod frame_capture;
#[cfg(feature = "diag-probes")]
use frame_capture::FrameCapture;

impl Graphics {
    fn capture_enabled(&self) -> bool {
        #[cfg(feature = "diag-probes")]
        { self.capture.is_some() }
        #[cfg(not(feature = "diag-probes"))]
        { false }
    }

    // Only entry/result/ownership metadata; never query/consume an EGL error,
    // inspect guest memory or hold a resources lock across the log callback.
    // Separate from first-present evidence: an EGL entry is not a game frame.
    fn trace_egl(&self, message: impl FnOnce() -> String) {
        if rudroid_common::logging::diagnostics_enabled()
            && self.diagnostic_records.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
                |n| (n < 96).then_some(n + 1)).is_ok()
        {
            rudroid_common::diagnostic!("[SDL] guest EGL phase: {} thread={:?}", message(), std::thread::current().id());
        }
    }
    /// # Safety
    /// Run on the SDL creating thread with its primary context current. The
    /// SDL session must outlive this service and all resolved-function uses.
    /// A callback must remain valid and be callable with no extra preconditions
    /// after a successful present, until every guest caller has stopped.
    pub(crate) unsafe fn activate(
        presented: Option<extern "C" fn()>,
        render: Option<RenderDimensions>,
        _native_wayland_buffer: bool,
    ) -> Result<Arc<Self>, String> {
        if ACTIVE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .upgrade()
            .is_some()
        {
            return Err("a guest graphics session is already active".into());
        }
        let driver = Driver::load()?;
        // SAFETY: SDL/EGL current-state queries have no preconditions; our
        // activation contract keeps the creator-owned window and context live.
        let (window, display, context, surface) = unsafe {
            (
                sdl2::sys::SDL_GL_GetCurrentWindow(),
                (driver.current_display)(),
                (driver.current_context)(),
                (driver.current_surface)(DRAW),
            )
        };
        if window.is_null() || display.is_null() || context.is_null() || surface.is_null() {
            return Err(
                "SDL GLES session has no current native EGL display/context/window surface".into(),
            );
        }
        let mut id = 0;
        // EGL_KHR_no_config_context permits a context config id of zero.
        // The existing window surface is the authoritative pixel config.
        // SAFETY: current display/surface checked non-null; `id` is a valid out slot.
        if unsafe { (driver.query_surface)(display, surface, CONFIG_ID, &mut id) } == FALSE {
            return Err("cannot query the primary EGL window config".into());
        }
        let (mut width, mut height) = (0, 0);
        // SAFETY: current display/surface; `width`/`height` are valid out slots.
        if unsafe { (driver.query_surface)(display, surface, WIDTH, &mut width) } == FALSE
            // SAFETY: as above, for the height slot.
            || unsafe { (driver.query_surface)(display, surface, HEIGHT, &mut height) } == FALSE
            || width <= 0 || height <= 0
        {
            return Err("cannot query the primary EGL window size".into());
        }
        let mut config = std::ptr::null_mut();
        let mut count = 0;
        // SAFETY: NONE-terminated attribute list; out slots sized for one config.
        if unsafe {
            (driver.choose_config)(
                display,
                [CONFIG_ID, id, NONE].as_ptr(),
                &mut config,
                1,
                &mut count,
            )
        } == FALSE
            || count != 1
        {
            return Err(format!(
                "cannot find the primary EGL config: id={id}, count={count}, error={:#x}",
                // SAFETY: eglGetError has no preconditions.
                unsafe { (driver.error)() }
            ));
        }
        // Advertise only extensions whose guest entry paths are available.
        // SAFETY: live display; EGL returns null or a NUL-terminated string it owns.
        let raw = unsafe { (driver.query_string)(display, EXTENSIONS) };
        let native = if raw.is_null() {
            ""
        } else {
            // SAFETY: non-null, NUL-terminated and owned by the live display.
            unsafe { CStr::from_ptr(raw) }.to_str().unwrap_or("")
        };
        let supported = [
            "EGL_KHR_create_context",
            "EGL_KHR_surfaceless_context",
            "EGL_KHR_fence_sync",
            "EGL_KHR_wait_sync",
            "EGL_KHR_reusable_sync",
            "EGL_EXT_buffer_age",
            "EGL_KHR_swap_buffers_with_damage",
            "EGL_EXT_swap_buffers_with_damage",
            "EGL_KHR_get_all_proc_addresses",
        ];
        let mut extensions = native
            .split_ascii_whitespace()
            .filter(|ext| supported.contains(ext))
            .collect::<Vec<_>>()
            .join(" ");
        extensions.push_str(" EGL_ANDROID_presentation_time");
        let dimensions=render.unwrap_or(RenderDimensions::resolve(i64::from(width),i64::from(height),0,0,100,true)?);
        if dimensions.output_width!=width || dimensions.output_height!=height {
            return Err("native EGL surface size differs from the configured SDL output".into());
        }
        // SAFETY: creator thread with its owning primary context current, before guest execution.
        let scale=if dimensions.scaled() {Some(unsafe {RenderScale::create(dimensions)}?)}else{None};
        rudroid_common::diagnostic!("[SDL] render geometry: output={}x{} guest={}x{} scaled={} sharp={}",width,height,dimensions.width,dimensions.height,dimensions.scaled(),dimensions.sharp);
        let disable_invalidation = super::invalidate_policy::configured();
        rudroid_common::diagnostic!("[SDL] framebuffer invalidation policy: disabled={disable_invalidation}");
        #[cfg(feature = "diag-probes")]
        let native_gl = |name: &CStr| {
            // SAFETY: the verified primary GLES context is current at activation;
            // names are NUL-terminated and only driver functions are resolved.
            unsafe { sdl2::sys::SDL_GL_GetProcAddress(name.as_ptr()) as usize }
        };
        #[cfg(feature = "diag-probes")]
        let geometry_maps = geometry_map::GeometryMaps::configured(native_gl);
        let graphics = Arc::new(Self {
            driver,
            window: window as usize,
            display: display as usize,
            context: context as usize,
            surface: surface as usize,
            config: config as usize,
            width: dimensions.width,
            height: dimensions.height,
            dimensions,
            scale,
            #[cfg(feature = "diag-probes")]
            geometry_maps,
            #[cfg(feature = "diag-probes")]
            geometry_trace: geometry_trace::Trace::configured(native_gl),
            disable_invalidation,
            primary_claimed: AtomicBool::new(false),
            live: AtomicBool::new(true),
            get_integer: driver_address(c"glGetIntegerv"),
            use_program: driver_address(c"glUseProgram"),
            addresses: Mutex::new(HashMap::new()),
            resources: Mutex::new(Resources::default()),
            extensions: CString::new(extensions).unwrap(),
            presented,
            #[cfg(feature = "diag-probes")]
            interval_trace: super::wayland_interval::Trace::for_native_wayland(_native_wayland_buffer),
            #[cfg(feature = "diag-probes")]
            capture: FrameCapture::from_environment(),
            capture_errors: Mutex::new(HashMap::new()),
            swaps: AtomicU64::new(0),
            diagnostic_records: AtomicU8::new(0),
        });
        // SAFETY: creating thread, primary context current, driver and window live.
        unsafe { capture_gles_extensions(&graphics) }?;
        // Hand the primary context to whichever guest render thread binds it.
        // SAFETY: releasing the creating thread's current context is always valid.
        if unsafe { sdl2::sys::SDL_GL_MakeCurrent(std::ptr::null_mut(), std::ptr::null_mut()) } != 0
        {
            return Err(
                "cannot release the primary EGL context for the guest render thread".into(),
            );
        }
        let mut slot = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        if slot.upgrade().is_some() {
            return Err("a guest graphics session is already active".into());
        }
        *slot = Arc::downgrade(&graphics);
        GENERATION.fetch_add(1, Ordering::Release);
        Ok(graphics)
    }

    /// # Safety
    /// Run on the SDL creating thread while its display/context/window are
    /// still live. All guest calls (including cached native GL pointers) must
    /// have stopped and guest threads must have released their EGL bindings.
    /// Arc keeps this Rust state live, not the SDL-owned native resources.
    pub(crate) unsafe fn shutdown(&self) {
        if !self.live.swap(false, Ordering::AcqRel) {return;}
        let mut slot = ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
        if std::ptr::eq(slot.as_ptr(), self) {
            *slot = Weak::new();
            GENERATION.fetch_add(1, Ordering::Release);
        }
        drop(slot);
        let resources =
            std::mem::take(&mut *self.resources.lock().unwrap_or_else(|p| p.into_inner()));
        // SAFETY: per this fn's contract the display is live and no guest uses these objects.
        unsafe {
            if let Some(scale)=&self.scale {
                if (self.driver.make_current)(self.display as Handle,self.surface as Handle,self.surface as Handle,self.context as Handle)==TRUE {
                    scale.destroy();
                } else {
                    // Context destruction still owns/reclaims these GL names; never delete on an unrelated context.
                    rudroid_common::error!("[SDL] could not bind the owning context for scale cleanup");
                }
            }
            (self.driver.make_current)(
                self.display as Handle,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            for context in resources.contexts {
                (self.driver.destroy_context)(self.display as Handle, context as Handle);
                shader_gl::forget_context(context);
            }
            for context in resources.retired_contexts {shader_gl::forget_context(context);}
            shader_gl::forget_context(self.context);
            texture_gl::report();
            for surface in resources.surfaces {
                (self.driver.destroy_surface)(self.display as Handle, surface as Handle);
            }
        }
    }

    fn remember_gl_error(&self, context:usize, error:u32) {
        let mut pending=self.capture_errors.lock().unwrap_or_else(|p|p.into_inner());
        let errors=pending.entry(context).or_default();
        if errors.len()<16 && !errors.contains(&error) {errors.push_back(error);}
    }
    fn display_ok(&self, display: Handle) -> bool {
        if display as usize == self.display {
            true
        } else {
            fail(BAD_DISPLAY, false)
        }
    }

    pub(crate) fn dimensions(&self)->RenderDimensions {self.dimensions}

    fn primary_current(&self)->bool {
        // SAFETY: thread-local SDL/EGL binding queries have no preconditions;
        // the window and native library remain pinned until guest quiescence.
        unsafe {
            if sdl2::sys::SDL_GL_GetCurrentWindow() as usize!=self.window
                || (self.driver.current_context)() as usize!=self.context { return false; }
            let draw=(self.driver.current_surface)(DRAW);
            !draw.is_null() && draw==(self.driver.current_surface)(0x305a)
        }
    }

    fn primary_scale(&self)->Option<&RenderScale> {
        let scale=self.scale.as_ref()?;
        self.primary_current().then_some(scale)
    }

    #[cfg(test)]
    fn resolve(&self, name: &CStr) -> usize {
        self.resolve_from(name, Route::ElfSource)
    }

    fn resolve_from(&self, name: &CStr, route: Route) -> usize {
        let bytes = name.to_bytes();
        if bytes.starts_with(b"ANativeWindow_") {
            return super::native_window::resolve::<Self>(bytes);
        }
        let egl = egl_link_symbol(name);
        if egl != 0 { return egl; }
        let canonical = if bytes.starts_with(b"glad_gl") {
            CStr::from_bytes_with_nul(&name.to_bytes_with_nul()[5..]).unwrap()
        } else {
            name
        };
        let address = self.resolve_gl(canonical);
        #[cfg(feature = "diag-probes")]
        if let Some(trace) = &self.geometry_trace {
            return trace.resolve(canonical.to_bytes(), route, address);
        }
        let _ = route;
        address
    }

    fn resolve_gl(&self, canonical: &CStr) -> usize {
        let key = canonical.to_bytes();
        let hint = super::invalidate_policy::symbol(self.disable_invalidation, key);
        if hint != 0 { return hint; }
        {
            match key {
                b"glGetString"=>return identity_string as usize,
                b"glGetStringi"=>return identity_string_indexed as usize,
                b"glGetIntegerv"=>return identity_integer as usize,
                b"glGetInteger64v"=>return identity_integer64 as usize,
                b"glProgramBinary" | b"glProgramBinaryOES"=>return program_binary as usize,
                b"glGetProgramBinary" | b"glGetProgramBinaryOES"=>return get_program_binary as usize,
                _=>{},
            }
        }
        #[cfg(feature = "diag-probes")]
        if let Some(geometry_maps) = &self.geometry_maps {
            let wrapper = geometry_maps.symbol(key);
            if wrapper != 0 {
                return wrapper;
            }
        }
        let texture=texture_gl::symbol(key);if texture!=0{return texture;}
        let shader=shader_gl::symbol(key);if shader!=0{return shader;}
        if self.scale.is_some() {
            let wrapper=scaled_gl::symbol(key);
            if wrapper!=0 {return wrapper;}
        }
        if key==b"glGetError" && (self.capture_enabled() || super::texture_policy::enabled()){return captured_gl_error as usize;}
        self.native_address(canonical)
    }

    // Resolve the pinned native driver entry without reentering guest wrappers.
    fn native_address(&self, canonical:&CStr)->usize {
        let key=canonical.to_bytes();
        match key {
            b"glGetIntegerv" if self.get_integer != 0 => return self.get_integer,
            b"glUseProgram" if self.use_program != 0 => return self.use_program,
            _ => {}
        }
        let gl = key.starts_with(b"gl") && key.get(2).is_some_and(u8::is_ascii_uppercase);
        let egl = matches!(
            key,
            b"eglBindAPI"
                | b"eglQueryAPI"
                | b"eglWaitGL"
                | b"eglWaitNative"
                | b"eglWaitClient"
                | b"eglBindTexImage"
                | b"eglReleaseTexImage"
                | b"eglSignalSyncKHR"
        ) || key.starts_with(b"eglCreateSync")
            || key.starts_with(b"eglDestroySync")
            || key.starts_with(b"eglClientWaitSync")
            || key.starts_with(b"eglWaitSync")
            || key.starts_with(b"eglGetSyncAttrib");
        if (!gl && !egl)
            || key.len() > 256
            || !key.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            return 0;
        }
        if let Some(address) = self
            .addresses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .copied()
        {
            return address;
        }
        // No cache lock crosses a driver call. Misses are not retained.
        let address = driver_address(canonical);
        if address != 0 {
            let mut cache = self.addresses.lock().unwrap_or_else(|p| p.into_inner());
            if cache.len() < CACHE_LIMIT {
                cache.entry(key.to_vec()).or_insert(address);
            }
        }
        address
    }
}

/// Called during graphics activation or while resolving a live guest entry.
fn driver_address(name: &CStr) -> usize {
    // SAFETY: name is terminated and SDL video is initialized by the active session.
    unsafe { sdl2::sys::SDL_GL_GetProcAddress(name.as_ptr()) as usize }
}

/// # Safety
/// Caller obeys GLES current-context rules. Override strings remain process-pinned.
unsafe extern "C" fn identity_string(name:u32)->*const u8 {
    let Some(g)=active() else{return std::ptr::null();};
    // SAFETY: current-context query and pinned GLES entry have the declared
    // prototypes; returning a static string lends no temporary Rust storage.
    unsafe {
        if name==0x1f02 && !(g.driver.current_context)().is_null() {
            if let Some(version)=super::gles_identity::version() {return version.as_ptr().cast();}
        }
        if !(g.driver.current_context)().is_null() {
            if name==0x1f03 {
                if let Some(value)=super::gles_policy::extension_string() {return value.as_ptr().cast();}
            }
            if let Some(value)=super::gles_policy::cached_string(name) {return value.as_ptr().cast();}
        }
        let address=g.native_address(c"glGetString");if address==0{return std::ptr::null();}
        let query:unsafe extern "C" fn(u32)->*const u8=std::mem::transmute(address);
        let value=query(name);
        if !value.is_null() {
            if let Some(copy)=super::gles_policy::remember_string(name,CStr::from_ptr(value.cast())) {return copy.as_ptr().cast();}
        }
        value
    }
}
/// # Safety
/// Output holds the native GLES query's writable element count and is never retained.
unsafe extern "C" fn identity_integer(name:u32,out:*mut i32) {
    let Some(g)=active() else{return;};
    // SAFETY: version queries write exactly one caller-owned i32. All other
    // queries retain their native/scaled prototype and original output count.
    unsafe {
        if !out.is_null() && !(g.driver.current_context)().is_null() {
            if let Some(value)=super::gles_identity::integer(name) {*out=value;return;}
            match super::gles_policy::integer(name) {
                Some(super::gles_policy::IntegerOverride::Scalar(value)) => {*out=value;return;}
                Some(super::gles_policy::IntegerOverride::NoWrite) => return,
                None => {}
            }
        }
        let scaled=if g.scale.is_some(){scaled_gl::symbol(b"glGetIntegerv")}else{0};
        let address=if scaled!=0 {scaled}else{g.native_address(c"glGetIntegerv")};
        if address==0{return;}
        let query:unsafe extern "C" fn(u32,*mut i32)=std::mem::transmute(address);
        query(name,out);
    }
}

/// # Safety
/// The creating thread owns a current live GLES context for this graphics session.
unsafe fn capture_gles_extensions(g: &Graphics) -> Result<(), String> {
    if !super::gles_policy::extension_dirty() { return Ok(()); }
    // SAFETY: pinned driver prototypes; native strings are copied while the context is current.
    unsafe {
        let raw=g.native_address(c"glGetString");if raw==0{return Err("native GLES string query unavailable".into());}
        let string:unsafe extern "C" fn(u32)->*const u8=std::mem::transmute(raw);
        let version=string(0x1f02);if version.is_null(){return Err("native GLES version unavailable".into());}
        let es3=CStr::from_ptr(version.cast()).to_bytes().starts_with(b"OpenGL ES 3.");
        let addr=g.native_address(c"glGetIntegerv"); let indexed=g.native_address(c"glGetStringi");
        let mut native=Vec::new();
        if es3 && addr!=0 && indexed!=0 {
            let integer:unsafe extern "C" fn(u32,*mut i32)=std::mem::transmute(addr);
            let string:unsafe extern "C" fn(u32,u32)->*const u8=std::mem::transmute(indexed);
            let mut count=0;integer(0x821d,&mut count);
            if !(0..=4096).contains(&count) {return Err("invalid native GLES extension count".into());}
            for i in 0..count {let p=string(0x1f03,i as u32);if !p.is_null(){native.push(CStr::from_ptr(p.cast()).to_string_lossy().into_owned());}}
        } else {
            let p=string(0x1f03);
            if p.is_null(){return Err("native GLES extensions are unavailable".into());}
            native.extend(CStr::from_ptr(p.cast()).to_string_lossy().split_ascii_whitespace().map(str::to_owned));
        }
        super::gles_policy::initialize_extensions(&native)
    }
}
/// # Safety
/// Caller obeys GLES current-context rules; returned strings are process-pinned.
unsafe extern "C" fn identity_string_indexed(name:u32,index:u32)->*const u8 {
    let Some(g)=active() else{return std::ptr::null();};
    // SAFETY: current-context query, exact native prototype, stable filtered strings.
    unsafe {
        if name==0x1f03 && super::gles_policy::extension_dirty() && !(g.driver.current_context)().is_null() {
            return super::gles_policy::extension(index).map_or(std::ptr::null(),|s|s.as_ptr().cast());
        }
        let addr=g.native_address(c"glGetStringi");if addr==0{return std::ptr::null();}
        let query:unsafe extern "C" fn(u32,u32)->*const u8=std::mem::transmute(addr);query(name,index)
    }
}
/// # Safety
/// Output holds the requested native query's writable element count, with i64 alignment.
unsafe extern "C" fn identity_integer64(name:u32,out:*mut i64) {
    let Some(g)=active() else{return;};
    // SAFETY: overridden queries are scalar; vector queries preserve native/scaled element counts.
    unsafe {
        if !out.is_null() && !(g.driver.current_context)().is_null() {
            if let Some(value)=super::gles_identity::integer(name) {*out=i64::from(value);return;}
            match super::gles_policy::integer(name) {
                Some(super::gles_policy::IntegerOverride::Scalar(value)) => {*out=i64::from(value);return;}
                Some(super::gles_policy::IntegerOverride::NoWrite) => return,
                None => {}
            }
        }
        let scaled=if g.scale.is_some(){scaled_gl::symbol(b"glGetInteger64v")}else{0};
        let addr=if scaled!=0{scaled}else{g.native_address(c"glGetInteger64v")};if addr==0{return;}
        let query:unsafe extern "C" fn(u32,*mut i64)=std::mem::transmute(addr);query(name,out);
    }
}
/// # Safety
/// In native mode the binary holds length readable bytes and a compatible context is current.
unsafe extern "C" fn program_binary(program:u32,format:u32,binary:*const c_void,length:i32) {
    if super::gles_policy::binary_disabled(){return;}
    let Some(g)=active() else{return;};
    let addr=g.native_address(c"glProgramBinary");let addr=if addr!=0{addr}else{g.native_address(c"glProgramBinaryOES")};if addr==0{return;}
    // SAFETY: exact GLES core/OES ABI; caller supplies current context and readable binary.
    unsafe {
        let call: unsafe extern "C" fn(u32, u32, *const c_void, i32) = std::mem::transmute(addr);
        let track_skip = super::shader_policy::policy().enabled() && super::shader_policy::policy().skip;
        let change = track_skip.then(shader_gl::begin_change);
        call(program, format, binary, length);
        if track_skip {
            shader_gl::program_binary_loaded(&g, program);
        }
        drop(change);
    }
}
/// # Safety
/// Non-null length/format are writable scalars; native mode additionally requires buffer_size writable bytes.
unsafe extern "C" fn get_program_binary(program:u32,buffer_size:i32,length:*mut i32,format:*mut u32,binary:*mut c_void) {
    // SAFETY: the ABI lends these outputs for this call; disabled mode never accesses binary storage.
    unsafe {
        if super::gles_policy::binary_disabled(){if !length.is_null(){*length=0;}if !format.is_null(){*format=0;}return;}
        let Some(g)=active() else{return;};let addr=g.native_address(c"glGetProgramBinary");let addr=if addr!=0{addr}else{g.native_address(c"glGetProgramBinaryOES")};if addr==0{return;}
        let call:unsafe extern "C" fn(u32,i32,*mut i32,*mut u32,*mut c_void)=std::mem::transmute(addr);call(program,buffer_size,length,format,binary);
    }
}

/// EGL entry addresses remain available for Unity's unconditional relocations
/// even with Vulkan selected. Without an active GLES session these established
/// wrappers report EGL_NOT_INITIALIZED; Android window tokens never reach the
/// host driver's Wayland EGL entry points.
pub(crate) fn egl_link_symbol(name: &CStr) -> usize {
    match name.to_bytes() {
        b"eglGetDisplay" => get_display as usize,
        b"eglInitialize" => initialize as usize,
        b"eglChooseConfig" => choose_config as usize,
        b"eglGetConfigs" => get_configs as usize,
        b"eglGetConfigAttrib" => config_attrib as usize,
        b"eglCreateWindowSurface" => create_window_surface as usize,
        b"eglCreateContext" => create_context as usize,
        b"eglCreatePbufferSurface" => create_pbuffer as usize,
        b"eglDestroyContext" => destroy_context as usize,
        b"eglDestroySurface" => destroy_surface as usize,
        b"eglMakeCurrent" => make_current as usize,
        b"eglGetCurrentDisplay" => current_display as usize,
        b"eglGetCurrentContext" => current_context as usize,
        b"eglGetCurrentSurface" => current_surface as usize,
        b"eglQuerySurface" => query_surface as usize,
        b"eglQueryString" => query_string as usize,
        b"eglSwapBuffers" => swap as usize,
        b"eglSwapBuffersWithDamageKHR" | b"eglSwapBuffersWithDamageEXT" => {
            swap_damage as usize
        }
        b"eglSwapInterval" => interval as usize,
        b"eglGetError" => get_error as usize,
        b"eglGetProcAddress" => get_proc_address as usize,
        b"eglReleaseThread" => release_thread as usize,
        b"eglTerminate" => terminate as usize,
        b"eglPresentationTimeANDROID" => presentation_time as usize,
        b"eglSurfaceAttrib" => surface_attrib as usize,
        b"eglQueryContext" => query_context as usize,
        _ => 0,
    }
}
unsafe extern "C" fn query_context(display: Handle, context: Handle, attribute: i32, value: *mut i32) -> Bool {
    let Some(g) = active() else { return fail(NOT_INITIALIZED, FALSE); };
    if !g.display_ok(display) { return FALSE; }
    // SAFETY: exact EGL ABI, validated display; native EGL validates context/attribute/output.
    unsafe { (g.driver.query_context)(display, context, attribute, value) }
}
unsafe extern "C" fn surface_attrib(display: Handle, surface: Handle, attribute: i32, value: i32) -> Bool {
    let Some(g) = active() else { return fail(NOT_INITIALIZED, FALSE); };
    if !g.display_ok(display) { return FALSE; }
    // SAFETY: exact EGL ABI, validated display; native EGL validates the
    // surface and scalar attribute. No Android window token is forwarded.
    unsafe { (g.driver.surface_attrib)(display, surface, attribute, value) }
}

/// ELF symbol-source callback. The loader pins the capability and session.
/// # Safety
/// `name` is null or points to a NUL-terminated C string borrowed for this call;
/// `user` is unused. Guest threads must stop calling before session destruction.
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_guest_symbol(name: *const c_char, _user: Handle) -> usize {
    if name.is_null() {
        return 0;
    }
    // SAFETY: the source caller lends a readable NUL-terminated name.
    let name = unsafe { CStr::from_ptr(name) };
    resolve_guest_symbol(name, Route::ElfSource)
}

fn resolve_guest_symbol(name: &CStr, route: Route) -> usize {
    #[cfg(feature = "vulkan")]
    {
        let result = super::vulkan::resolve(name);
        if result != 0 { return result; }
    }
    active().map_or(0, |graphics| graphics.resolve_from(name, route))
}

unsafe extern "C" fn get_display(_native: Handle) -> Handle {
    active().map_or_else(
        || fail(NOT_INITIALIZED, std::ptr::null_mut()),
        |g| g.display as Handle,
    )
}
unsafe extern "C" fn initialize(display: Handle, major: *mut i32, minor: *mut i32) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    // SAFETY: validated display; out pointers forwarded under the EGL contract.
    unsafe { (g.driver.initialize)(display, major, minor) }
}
unsafe extern "C" fn choose_config(
    display: Handle,
    _attrs: *const i32,
    configs: *mut Handle,
    size: i32,
    count: *mut i32,
) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    if count.is_null() || size < 0 {
        return fail(BAD_PARAMETER, FALSE);
    }
    // SAFETY: `count` checked non-null; `configs` holds `size` entries per the EGL contract.
    unsafe {
        *count = if configs.is_null() { 1 } else { size.min(1) };
        if !configs.is_null() && size > 0 {
            *configs = g.config as Handle;
        }
    }
    TRUE
}
unsafe extern "C" fn get_configs(
    display: Handle,
    configs: *mut Handle,
    size: i32,
    count: *mut i32,
) -> Bool {
    // SAFETY: same EGL contract as eglChooseConfig; null attributes are allowed.
    unsafe { choose_config(display, std::ptr::null(), configs, size, count) }
}
unsafe extern "C" fn config_attrib(
    display: Handle,
    config: Handle,
    attribute: i32,
    out: *mut i32,
) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    if config as usize != g.config {
        return fail(BAD_CONFIG, FALSE);
    }
    // SAFETY: validated display/config; `out` forwarded under the EGL contract.
    unsafe { (g.driver.config_attrib)(display, config, attribute, out) }
}
unsafe extern "C" fn create_window_surface(
    display: Handle,
    config: Handle,
    _native: Handle,
    _attrs: *const i32,
) -> Handle {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, std::ptr::null_mut());
    };
    g.trace_egl(|| "create-window-surface begin".into());
    if !g.display_ok(display) {
        return std::ptr::null_mut();
    }
    if config as usize != g.config {
        return fail(BAD_CONFIG, std::ptr::null_mut());
    }
    g.surface as Handle
}

impl super::native_window::Backend for Graphics {
    const FORMAT: Option<i32> = None;
    const NULL_WINDOW: i32 = -22;
    const INVALID_GEOMETRY: i32 = -22;
    fn active() -> Option<Arc<Self>> { active() }
    fn size(&self) -> (i32, i32) { (self.width, self.height) }
    fn trace_surface(&self, present: bool) {
        self.trace_egl(|| format!("fromSurface surface_present={present}"));
    }
    fn trace_geometry(&self, width: i32, height: i32, format: i32, result: i32) {
        self.trace_egl(|| format!("native-window geometry width={width} height={height} format={format} returned={result}"));
    }
}

unsafe extern "C" fn create_context(
    display: Handle,
    config: Handle,
    share: Handle,
    attrs: *const i32,
) -> Handle {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, std::ptr::null_mut());
    };
    g.trace_egl(|| format!("create-context begin share_present={}", !share.is_null()));
    if !g.display_ok(display) {
        return std::ptr::null_mut();
    }
    if config as usize != g.config {
        return fail(BAD_CONFIG, std::ptr::null_mut());
    }
    if share.is_null() && !g.primary_claimed.swap(true, Ordering::AcqRel) {
        g.trace_egl(|| "create-context returned primary".into());
        return g.context as Handle;
    }
    #[cfg(feature = "diag-probes")]
    if !share.is_null() && g.geometry_trace.as_ref().is_some_and(|trace| !trace.before_shared_context(&g)) {
        return fail(BAD_MATCH, std::ptr::null_mut());
    }
    // SAFETY: validated display/config; `share`/`attrs` forwarded under the EGL contract.
    let context = unsafe { (g.driver.create_context)(display, config, share, attrs) };
    g.trace_egl(|| format!("create-context native returned success={}", !context.is_null()));
    if !context.is_null() {
        g.resources
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contexts
            .insert(context as usize);
    }
    if !context.is_null(){g.capture_errors.lock().unwrap().remove(&(context as usize));}
    if !context.is_null(){
        if g.resources.lock().unwrap().retired_contexts.remove(&(context as usize)){shader_gl::forget_context(context as usize);}
        shader_gl::add_context(context as usize,share as usize);
    }
    context
}
unsafe extern "C" fn create_pbuffer(display: Handle, config: Handle, attrs: *const i32) -> Handle {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, std::ptr::null_mut());
    };
    if !g.display_ok(display) {
        return std::ptr::null_mut();
    }
    // SAFETY: validated display; `config`/`attrs` forwarded under the EGL contract.
    let surface = unsafe { (g.driver.create_pbuffer)(display, config, attrs) };
    if !surface.is_null() {
        g.resources
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .surfaces
            .insert(surface as usize);
    }
    surface
}
unsafe extern "C" fn destroy_context(display: Handle, context: Handle) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    if context as usize == g.context {
        g.primary_claimed.store(false, Ordering::Release);
        return TRUE;
    }
    // SAFETY: validated display; EGL rejects unknown context handles.
    let result = unsafe { (g.driver.destroy_context)(display, context) };
    if result == TRUE {
        g.resources
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contexts
            .remove(&(context as usize));
    }
    if result==TRUE{g.resources.lock().unwrap().retired_contexts.insert(context as usize);}
    result
}
unsafe extern "C" fn destroy_surface(display: Handle, surface: Handle) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    if surface as usize == g.surface {
        return TRUE;
    }
    // SAFETY: validated display; EGL rejects unknown surface handles.
    let result = unsafe { (g.driver.destroy_surface)(display, surface) };
    if result == TRUE {
        g.resources
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .surfaces
            .remove(&(surface as usize));
    }
    result
}
unsafe extern "C" fn make_current(
    display: Handle,
    draw: Handle,
    read: Handle,
    context: Handle,
) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    g.trace_egl(|| format!("make-current begin primary_context={} primary_surface={} release={}",
        context as usize == g.context, draw as usize == g.surface && read as usize == g.surface, context.is_null()));
    if !g.display_ok(display) {
        return FALSE;
    }
    // SAFETY: EGL current-state query, no preconditions.
    let previous=unsafe{(g.driver.current_context)()} as usize;
    if g.scale.is_some() && context as usize==g.context && (draw as usize==g.surface)!=(read as usize==g.surface) {
        return fail(0x3009,FALSE); // Mixed window/pbuffer bindings cannot represent one scaled default framebuffer.
    }
    // SAFETY: the session pins its SDL window. Binding a window context is
    // allowed on the guest render thread after the previous owner releases it.
    // Worker handles remain native EGL handles, validated by the driver.
    let result=unsafe {
        if !context.is_null() && draw as usize==g.surface && read as usize==g.surface {
            if sdl2::sys::SDL_GL_MakeCurrent(g.window as *mut _, context)==0 { TRUE }
            else {
                // SDL may consume EGL's error while formatting its own error.
                let error=(g.driver.error)();
                fail(if error==SUCCESS { BAD_MATCH } else { error }, FALSE)
            }
        } else {
            let previous_window=sdl2::sys::SDL_GL_GetCurrentWindow();
            let previous_context=sdl2::sys::SDL_GL_GetCurrentContext();
            if !previous_window.is_null()
                && sdl2::sys::SDL_GL_MakeCurrent(std::ptr::null_mut(), std::ptr::null_mut())!=0 {
                fail(BAD_MATCH, FALSE)
            } else {
                let result=(g.driver.make_current)(display, draw, read, context);
                if result==FALSE && !previous_window.is_null() {
                    let error=(g.driver.error)();
                    // A rejected worker bind must preserve the old current
                    // window/context as well as its original EGL error.
                    if sdl2::sys::SDL_GL_MakeCurrent(previous_window, previous_context)==0 {
                        let _ = ERROR.try_with(|slot|slot.set(error));
                    }
                }
                result
            }
        }
    };
    if result==TRUE {
        // SAFETY: successful native context binding; driver query buffers are owned and bounded.
        unsafe{shader_gl::refresh_binding(&g,None);}
        if let Some(scale)=g.primary_scale() {
            // SAFETY: successful binding verified as our owning context/window, no Rust lock is held.
            unsafe {scale.bind_defaults();}
        }
    }
    if result==TRUE && previous!=context as usize && g.resources.lock().unwrap().retired_contexts.remove(&previous){shader_gl::forget_context(previous);g.capture_errors.lock().unwrap().remove(&previous);}
    g.trace_egl(|| format!("make-current returned success={}", result == TRUE));
    result
}
unsafe extern "C" fn current_display() -> Handle {
    // SAFETY: EGL current-state query, no preconditions.
    active().map_or(std::ptr::null_mut(), |g| unsafe {
        (g.driver.current_display)()
    })
}
unsafe extern "C" fn current_context() -> Handle {
    // SAFETY: EGL current-state query, no preconditions.
    active().map_or(std::ptr::null_mut(), |g| unsafe {
        (g.driver.current_context)()
    })
}
unsafe extern "C" fn current_surface(which: i32) -> Handle {
    // SAFETY: SDL/EGL current-state queries have no preconditions. The guest
    // window token stays stable even if SDL replaces its native EGL surface.
    active().map_or(std::ptr::null_mut(), |g| unsafe {
        if matches!(which, DRAW | 0x305a)
            && sdl2::sys::SDL_GL_GetCurrentWindow() as usize==g.window {
            g.surface as Handle
        } else {
            (g.driver.current_surface)(which)
        }
    })
}
unsafe extern "C" fn query_surface(
    display: Handle,
    surface: Handle,
    attribute: i32,
    out: *mut i32,
) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    if surface as usize == g.surface && matches!(attribute, WIDTH | HEIGHT) {
        if out.is_null() { return fail(BAD_PARAMETER, FALSE); }
        // SAFETY: `out` checked non-null; EGL out-parameter contract.
        unsafe { *out = if attribute == WIDTH { g.width } else { g.height }; }
        return TRUE;
    }
    // SAFETY: validated display; `out` forwarded under the EGL contract.
    unsafe { (g.driver.query_surface)(display, surface, attribute, out) }
}
unsafe extern "C" fn query_string(display: Handle, name: i32) -> *const c_char {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, std::ptr::null());
    };
    if !g.display_ok(display) {
        return std::ptr::null();
    }
    if name == EXTENSIONS {
        g.extensions.as_ptr()
    } else {
        // SAFETY: validated display.
        unsafe { (g.driver.query_string)(display, name) }
    }
}
unsafe extern "C" fn swap(display: Handle, surface: Handle) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    g.trace_egl(|| format!("swap begin primary_surface={}", surface as usize == g.surface));
    if !g.display_ok(display) {
        return FALSE;
    }
    // SAFETY: `read` checks its surface/context is current before any GL call.
    #[cfg(feature = "diag-probes")]
    let captured=if surface as usize==g.surface {
        g.capture.as_ref().and_then(|c|c.take(g.swaps.load(Ordering::Relaxed)+1).and_then(|index|unsafe{c.read(&g)}.map(|pixels|(index,pixels))))
    }else{None};
    if surface as usize==g.surface {
        if let Some(scale)=g.primary_scale() {
            // SAFETY: owning primary context/window are current; state is restored before guest resumes.
            unsafe {scale.present();}
        }
    }
    // SAFETY: the session pins its SDL window. Only a thread with that window
    // current may present it. SDL performs the one EGL swap plus the native
    // backend's presentation work; pbuffers retain their native EGL path.
    let result = unsafe {
        if surface as usize==g.surface {
            if sdl2::sys::SDL_GL_GetCurrentWindow() as usize!=g.window {
                fail(BAD_CURRENT_SURFACE, FALSE)
            } else {
                #[cfg(feature = "diag-probes")]
                if let Some(trace) = &g.interval_trace {
                    if g.primary_current() {
                        trace.measure_swap(|| sdl2::sys::SDL_GL_SwapWindow(g.window as *mut _));
                    } else {
                        sdl2::sys::SDL_GL_SwapWindow(g.window as *mut _);
                    }
                } else {
                    sdl2::sys::SDL_GL_SwapWindow(g.window as *mut _);
                }
                #[cfg(not(feature = "diag-probes"))]
                sdl2::sys::SDL_GL_SwapWindow(g.window as *mut _);
                TRUE
            }
        } else {
            (g.driver.swap)(display, surface)
        }
    };
    if result == TRUE && surface as usize == g.surface {
        let swaps=g.swaps.fetch_add(1, Ordering::Relaxed)+1;
        if swaps == 1 {
            super::render_thread::first_present();
            rudroid_common::diagnostic!("[SDL] first guest EGL present: primary_context_current={} capture_requested={}",
                g.primary_current(),g.capture_enabled());
        }
        #[cfg(feature = "diag-probes")]
        if let (Some(capture),Some((index,pixels)))=(&g.capture,captured){capture.write(&g,&pixels,swaps,index);}
        if let Some(presented) = g.presented {
            presented();
        }
    }
    result
}
unsafe extern "C" fn swap_damage(
    display: Handle,
    surface: Handle,
    _rects: *const i32,
    _count: i32,
) -> Bool {
    // SAFETY: forwarded EGL contract; damage rectangles are optional hints.
    unsafe { swap(display, surface) }
}
unsafe extern "C" fn interval(display: Handle, value: i32) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    // SAFETY: both arms first validate the original request through the
    // owning EGL driver. SDL is only called from the current primary
    // window context, and no diagnostic call consumes eglGetError.
    unsafe {
        #[cfg(feature = "diag-probes")]
        if let Some(trace) = g.interval_trace.as_ref().filter(|_| {
            g.primary_current() && (g.driver.current_display)() == display
        }) {
            let before = sdl2::sys::SDL_GL_GetSwapInterval();
            let result = (g.driver.interval)(display, value);
            let routed = result == TRUE && matches!(value, 0 | 1) && trace.routes_to_sdl();
            let status = routed.then(|| sdl2::sys::SDL_GL_SetSwapInterval(value));
            let after = sdl2::sys::SDL_GL_GetSwapInterval();
            trace.interval(value, before, result == TRUE, status, after);
            return result;
        }
        (g.driver.interval)(display, value)
    }
}

#[cfg(feature = "diag-probes")]
pub(super) fn report_wayland_interval() -> std::io::Result<()> {
    if let Some(g) = active() {
        #[cfg(feature = "diag-probes")]
        if let Some(trace) = &g.interval_trace {
            return trace.report();
        }
    }
    Ok(())
}
unsafe extern "C" fn get_error() -> i32 {
    let custom = ERROR.try_with(|error| error.replace(SUCCESS)).unwrap_or(SUCCESS);
    // SAFETY: eglGetError has no preconditions.
    let native = active().map_or(SUCCESS, |g| unsafe { (g.driver.error)() });
    if native != SUCCESS {
        native
    } else {
        custom
    }
}
unsafe extern "C" fn captured_gl_error()->u32 {
    let Some(g)=active() else{return 0;};let context=(g.driver.current_context)() as usize;
    {let mut pending=g.capture_errors.lock().unwrap_or_else(|p|p.into_inner());
        if let Some(errors)=pending.get_mut(&context) {if let Some(error)=errors.pop_front(){if errors.is_empty(){pending.remove(&context);}return error;}}}
    static NATIVE:OnceLock<usize>=OnceLock::new();
    let address=*NATIVE.get_or_init(||sdl2::sys::SDL_GL_GetProcAddress(c"glGetError".as_ptr()) as usize);
    if address==0{return 0;}let error:unsafe extern "C" fn()->u32=std::mem::transmute(address);error()
}
unsafe extern "C" fn get_proc_address(name: *const c_char) -> Handle {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the EGL caller lends a readable NUL-terminated name.
    let name = unsafe { CStr::from_ptr(name) };
    // Scope eglGetProcAddress to the active GLES/EGL session only; never expose
    // Vulkan or NDK symbols through EGL extension lookup.
    active().map_or(0, |graphics| graphics.resolve_from(name, Route::EglProc)) as Handle
}
unsafe extern "C" fn release_thread() -> Bool {
    // SAFETY: eglReleaseThread has no preconditions.
    let result = active().map_or(TRUE, |g| {
        // SAFETY: current-state query and release on the calling thread; no guest pointer.
        unsafe {let previous=(g.driver.current_context)() as usize;
            if !sdl2::sys::SDL_GL_GetCurrentWindow().is_null() {
                sdl2::sys::SDL_GL_MakeCurrent(std::ptr::null_mut(),std::ptr::null_mut());
            }
            let result=(g.driver.release_thread)();
            if result==TRUE && g.resources.lock().unwrap_or_else(|p|p.into_inner()).retired_contexts.remove(&previous){shader_gl::forget_context(previous);g.capture_errors.lock().unwrap_or_else(|p|p.into_inner()).remove(&previous);}
            result}
    });
    let _ = CACHED.try_with(|slot| *slot.borrow_mut() = (0, Weak::new()));
    let _ = ERROR.try_with(|error| error.set(SUCCESS));
    result
}
unsafe extern "C" fn terminate(display: Handle) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    // The host owns the display/window: a guest cannot terminate SDL's EGL
    // display. Its worker contexts and pbuffers use explicit destroy calls.
    if g.display_ok(display) {
        TRUE
    } else {
        FALSE
    }
}
unsafe extern "C" fn presentation_time(display: Handle, surface: Handle, _time: i64) -> Bool {
    let Some(g) = active() else {
        return fail(NOT_INITIALIZED, FALSE);
    };
    if !g.display_ok(display) {
        return FALSE;
    }
    if surface as usize != g.surface {
        return fail(BAD_PARAMETER, FALSE);
    }
    TRUE // Same reference policy: presentation timing belongs to the host.
}

#[cfg(test)]
#[path = "../tests/unit/guest_graphics.rs"]
pub(crate) mod tests;

#[cfg(test)]
pub(crate) fn verify_restarted_session(session: &mut crate::window::SdlSession) -> Weak<Graphics> {
    // SAFETY: creator-thread, exclusively borrowed live session; no callback.
    unsafe { session.activate_guest_graphics(None) }.unwrap();
    let graphics = active().expect("TLS must resolve the new service generation");
    assert_ne!(graphics.resolve(c"glClearColor"), 0);
    let weak = Arc::downgrade(&graphics);
    drop(graphics);
    // Caller destroys the SDL session after this temporary owner is gone.
    assert!(weak.upgrade().is_some());
    weak
}

#[cfg(test)]
#[path="../tests/unit/texture_gl.rs"]
mod texture_tests;
