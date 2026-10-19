//! INPUT:  explicit Vulkan selection, SDL-owned Wayland window and pinned native Vulkan loader
//! OUTPUT:  Android surface/proc-address bridge with real native Vulkan device and presentation calls
//! POS:    optional native graphics boundary; GLES, audio and input keep their existing owners
use ash::vk;
use rudroid_common::display_dimensions::RenderDimensions;
use std::{ffi::{c_char, c_void, CStr}, sync::{Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering}}};

const LAYER: &CStr = c"VK_LAYER_RUDROID_wayland";
static ACTIVE: Mutex<Weak<Graphics>> = Mutex::new(Weak::new());

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeWindow { display: usize, surface: usize }
pub(crate) struct Observed { pub version: String, pub renderer: String, pub vendor: String,
    pub major: u32, pub minor: u32 }

fn entry() -> Result<&'static ash::Entry, String> {
    static WSI: OnceLock<Result<libloading::Library, String>> = OnceLock::new();
    WSI.get_or_init(|| {
        let path = std::env::var_os("RUDROID_VULKAN_WSI_LIBRARY").ok_or("candidate WSI library path is absent")?;
        let path = std::path::Path::new(&path);
        if !path.is_absolute() || path.file_name().is_none_or(|name|name != "libVkLayer_window_system_integration.so") {
            return Err("candidate WSI path must name the absolute audited layer library".into());
        }
        // SAFETY: the trusted launcher supplies the audited native dependency.
        // Pinning it separately preserves every returned layer function pointer
        // across temporary-instance destruction and the loader's dlclose calls.
        unsafe { libloading::Library::new(path) }.map_err(|e|e.to_string())
    }).as_ref().map_err(Clone::clone)?;
    static ENTRY: OnceLock<Result<ash::Entry, String>> = OnceLock::new();
    ENTRY.get_or_init(|| {
        // SAFETY: trusted native system loader with the Vulkan C ABI. The
        // immutable Entry owns its library for every returned function's life.
        unsafe { ash::Entry::load_from("libvulkan.so.1") }.map_err(|e| e.to_string())
    }).as_ref().map_err(Clone::clone)
}

/// Query the physical pixel extent without raising the minimum SDL version for
/// ordinary GLES sessions. Older SDL installations can still load this module;
/// only an explicitly selected Vulkan session needs this optional entry.
pub(crate) fn pixel_extent(window: &sdl2::video::Window) -> Result<(i32, i32), String> {
    type GetSize = unsafe extern "C" fn(*mut sdl2::sys::SDL_Window, *mut i32, *mut i32);
    static QUERY: OnceLock<Result<(libloading::Library, GetSize), String>> = OnceLock::new();
    let (_, query) = QUERY.get_or_init(|| {
        // SAFETY: trusted system SDL2 C ABI. Pin the library for the copied
        // function's lifetime; this lookup runs only for Vulkan, never GLES.
        unsafe {
            let library = libloading::Library::new("libSDL2-2.0.so.0").map_err(|e| e.to_string())?;
            let query = *library.get::<GetSize>(b"SDL_GetWindowSizeInPixels\0")
                .map_err(|_| "Vulkan pixel extent requires SDL 2.26 or later".to_owned())?;
            Ok((library, query))
        }
    }).as_ref().map_err(Clone::clone)?;
    let (mut width, mut height) = (0, 0);
    // SAFETY: creator-owned live SDL window and uniquely writable scalar outputs.
    unsafe { query(window.raw(), &mut width, &mut height) };
    Ok((width, height))
}

impl NativeWindow {
    pub(crate) fn from_window(window: &sdl2::video::Window) -> Result<Self, String> {
        // SAFETY: SDL SysWMinfo requests the version of these compiled bindings,
        // as SDL_VERSION requires, not a newer runtime's possibly larger ABI.
        // Only the checked Wayland union member is read; SDL owns its objects.
        unsafe {
            let mut info: sdl2::sys::SDL_SysWMinfo = std::mem::zeroed();
            info.version = sdl2::sys::SDL_version {
                major: sdl2::sys::SDL_MAJOR_VERSION as u8,
                minor: sdl2::sys::SDL_MINOR_VERSION as u8,
                patch: sdl2::sys::SDL_PATCHLEVEL as u8,
            };
            if sdl2::sys::SDL_GetWindowWMInfo(window.raw(), &mut info) != sdl2::sys::SDL_bool::SDL_TRUE
                || info.subsystem != sdl2::sys::SDL_SYSWM_TYPE::SDL_SYSWM_WAYLAND {
                return Err(format!("Vulkan requires a live Wayland window: {}", sdl2::get_error()));
            }
            let wl = info.info.wl;
            if wl.display.is_null() || wl.surface.is_null() { return Err("SDL returned no Wayland display/surface".into()); }
            Ok(Self { display: wl.display as usize, surface: wl.surface as usize })
        }
    }

    pub(crate) fn inspect(window: &sdl2::video::Window) -> Result<(Self, Observed), String> {
        if std::env::var("DISABLE_WSI_LAYER").as_deref() != Ok("1") {
            return Err("Vulkan candidate requires the firmware implicit WSI disabled in its launcher environment".into());
        }
        let native = Self::from_window(window)?;
        let entry = entry()?;
        // SAFETY: query with no native handles; every returned record is owned.
        let extensions = unsafe { entry.enumerate_instance_extension_properties(Some(LAYER)) }
            .map_err(|e| format!("private Vulkan WSI layer unavailable: {e:?}"))?;
        if !extensions.iter().any(|e| e.extension_name_as_c_str().ok() == Some(ash::khr::wayland_surface::NAME)) {
            return Err("private Vulkan layer has no Wayland surface extension".into());
        }
        let names = [ash::khr::surface::NAME.as_ptr(), ash::khr::wayland_surface::NAME.as_ptr()];
        let layers = [LAYER.as_ptr()];
        let app = vk::ApplicationInfo::default().application_name(c"Rudroid preflight")
            .api_version(vk::API_VERSION_1_1);
        let info = vk::InstanceCreateInfo::default().application_info(&app)
            .enabled_extension_names(&names).enabled_layer_names(&layers);
        // SAFETY: initialized borrowed arrays remain alive for the synchronous
        // call; this temporary native instance is retired below on every exit.
        let instance = unsafe { entry.create_instance(&info, None) }.map_err(|e| format!("Vulkan preflight instance: {e:?}"))?;
        let surface_api = ash::khr::surface::Instance::new(entry, &instance);
        let wayland = ash::khr::wayland_surface::Instance::new(entry, &instance);
        let create = vk::WaylandSurfaceCreateInfoKHR::default()
            .display(native.display as *mut vk::wl_display).surface(native.surface as *mut vk::wl_surface);
        // SAFETY: creator-thread SDL resources outlive this temporary surface.
        let created = unsafe { wayland.create_wayland_surface(&create, None) };
        let result = match created {
            Ok(surface) => {
                // SAFETY: owned live instance/surface, native queries copy into
                // owned records; no device or asynchronous GPU work is started.
                let result = unsafe { (|| {
                    let physical = instance.enumerate_physical_devices().map_err(|e| format!("Vulkan devices: {e:?}"))?
                        .into_iter().find(|p| instance.get_physical_device_properties(*p).device_type != vk::PhysicalDeviceType::CPU)
                        .ok_or("Vulkan has no hardware device")?;
                    let props = instance.get_physical_device_properties(physical);
                    let supported = instance.get_physical_device_queue_family_properties(physical).iter().enumerate()
                        .any(|(i,q)| q.queue_count > 0 && q.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                            && surface_api.get_physical_device_surface_support(physical, i as u32, surface).unwrap_or(false));
                    if !supported { return Err("Vulkan hardware cannot present to this Wayland surface".into()); }
                    let renderer = props.device_name_as_c_str().map_err(|_| "invalid Vulkan device name")?.to_string_lossy().into_owned();
                    Ok(Observed { version: format!("Vulkan {}.{}.{}", vk::api_version_major(props.api_version),
                        vk::api_version_minor(props.api_version), vk::api_version_patch(props.api_version)),
                        renderer, vendor: format!("vendor 0x{:04x}", props.vendor_id),
                        major: vk::api_version_major(props.api_version), minor: vk::api_version_minor(props.api_version) })
                })() };
                // SAFETY: no GPU work, no further surface calls, SDL stays live.
                unsafe { surface_api.destroy_surface(surface, None) };
                result
            }
            Err(error) => Err(format!("Vulkan Wayland preflight: {error:?}")),
        };
        // SAFETY: temporary surface destroyed, no device/child handles remain.
        unsafe { instance.destroy_instance(None) };
        result.map(|observed| (native, observed))
    }
}

struct Instance { native: ash::Instance, android_surface: bool }
struct Device { native: ash::Device, present: Option<vk::PFN_vkQueuePresentKHR> }
#[derive(Default)]
struct State { instance: Option<Arc<Instance>>, device: Option<Arc<Device>>, creating_instance: bool, creating_device: bool }
pub(crate) struct Graphics {
    entry: &'static ash::Entry,
    native: NativeWindow,
    dimensions: RenderDimensions,
    live: AtomicBool,
    presented: Option<extern "C" fn()>,
    state: Mutex<State>,
    presents: AtomicU64,
}
fn active() -> Option<Arc<Graphics>> {
    ACTIVE.lock().unwrap_or_else(|p|p.into_inner()).upgrade().filter(|g|g.live.load(Ordering::Acquire))
}
impl Graphics {
    /// # Safety
    /// Creator-owned SDL display/surface and callback remain valid until every
    /// guest call ends, all Vulkan children are destroyed, and shutdown runs.
    /// SDL's actual buffer extent must equal dimensions.width/height. A scaled
    /// session requires its configured native viewport and linear filter.
    pub(crate) unsafe fn activate(native: NativeWindow, dimensions: RenderDimensions,
        presented: Option<extern "C" fn()>) -> Result<Arc<Self>, String> {
        if dimensions.scaled() && dimensions.sharp { return Err("Vulkan supports linear native-buffer scaling only".into()); }
        let mut slot = ACTIVE.lock().unwrap_or_else(|p|p.into_inner());
        if slot.upgrade().is_some() { return Err("a Vulkan guest session is already active".into()); }
        let g = Arc::new(Self { entry: entry()?, native, dimensions, live: AtomicBool::new(true),
            presented, state: Mutex::new(State::default()), presents: AtomicU64::new(0) });
        *slot = Arc::downgrade(&g);
        Ok(g)
    }
    pub(crate) fn shutdown(&self) {
        self.live.store(false, Ordering::Release);
        let mut slot = ACTIVE.lock().unwrap_or_else(|p|p.into_inner());
        if std::ptr::eq(slot.as_ptr(), self) { *slot = Weak::new(); }
    }
    fn instance(&self) -> Option<Arc<Instance>> { self.state.lock().unwrap_or_else(|p|p.into_inner()).instance.clone() }
    fn device(&self) -> Option<Arc<Device>> { self.state.lock().unwrap_or_else(|p|p.into_inner()).device.clone() }
}

struct Creation { owner: Arc<Graphics>, device: bool }
impl Creation {
    fn claim(owner: Arc<Graphics>, device: bool) -> Option<Self> {
        {
            let mut state = owner.state.lock().unwrap_or_else(|p|p.into_inner());
            if device {
                if state.creating_device || state.device.is_some() { return None; }
                state.creating_device = true;
            } else {
                if state.creating_instance || state.instance.is_some() { return None; }
                state.creating_instance = true;
            }
        }
        Some(Self { owner, device })
    }
}
impl Drop for Creation {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|p|p.into_inner());
        if self.device { state.creating_device = false; } else { state.creating_instance = false; }
    }
}

macro_rules! address {
    ($function:ident) => {{
        // SAFETY: each match pairs the canonical Vulkan command with its exact
        // ash prototype below. Consumers cast back under Vulkan's proc ABI.
        Some(std::mem::transmute::<usize, unsafe extern "system" fn()>($function as usize))
    }};
}

/// # Safety
/// Native Vulkan callers lend valid handles and a readable NUL command name.
unsafe extern "system" fn get_instance_proc(instance: vk::Instance, name: *const c_char) -> vk::PFN_vkVoidFunction {
    if name.is_null() { return None; }
    let Some(g) = active() else { return None; };
    // SAFETY: native command name and handle follow the Vulkan query ABI.
    unsafe {
        let name = CStr::from_ptr(name);
        match name.to_bytes() {
            b"vkGetInstanceProcAddr" => return address!(get_instance_proc),
            b"vkCreateInstance" => return address!(create_instance),
            b"vkEnumerateInstanceExtensionProperties" => return address!(instance_extensions),
            _ => (),
        }
        if instance == vk::Instance::null() { return (g.entry.static_fn().get_instance_proc_addr)(instance, name.as_ptr()); }
        let own = g.instance()?;
        if own.native.handle() != instance { return None; }
        match name.to_bytes() {
            b"vkDestroyInstance" => address!(destroy_instance),
            b"vkCreateDevice" => address!(create_device),
            b"vkGetDeviceProcAddr" => address!(get_device_proc),
            b"vkCreateAndroidSurfaceKHR" if own.android_surface => address!(create_android_surface),
            b"vkQueuePresentKHR" if (g.entry.static_fn().get_instance_proc_addr)(instance, name.as_ptr()).is_some() => address!(queue_present),
            b"vkDestroyDevice" => address!(destroy_device),
            _ => (g.entry.static_fn().get_instance_proc_addr)(instance, name.as_ptr()),
        }
    }
}
/// # Safety
/// Valid live native device and NUL command name, under Vulkan's synchronization rules.
unsafe extern "system" fn get_device_proc(device: vk::Device, name: *const c_char) -> vk::PFN_vkVoidFunction {
    if name.is_null() { return None; }
    let g = active()?;
    let own = g.device()?;
    if own.native.handle() != device { return None; }
    // SAFETY: borrowed command/known device; forwarded native functions stay pinned.
    unsafe {
        match CStr::from_ptr(name).to_bytes() {
            b"vkGetDeviceProcAddr" => address!(get_device_proc),
            b"vkDestroyDevice" => address!(destroy_device),
            b"vkQueuePresentKHR" if own.present.is_some() => address!(queue_present),
            _ => (g.instance()?.native.fp_v1_0().get_device_proc_addr)(device, name),
        }
    }
}
/// Borrowed known graphics source, before the guest loader resolves Vulkan imports.
pub(crate) fn resolve(name: &CStr) -> usize {
    let Some(g) = active() else { return 0; };
    match name.to_bytes() {
        b"ANativeWindow_fromSurface" => from_surface as usize,
        b"ANativeWindow_acquire" => acquire as usize,
        b"ANativeWindow_release" => release as usize,
        b"ANativeWindow_getWidth" => width as usize,
        b"ANativeWindow_getHeight" => height as usize,
        b"ANativeWindow_getFormat" => format as usize,
        b"ANativeWindow_setBuffersGeometry" => geometry as usize,
        b"vkGetInstanceProcAddr" => get_instance_proc as usize,
        b"vkGetDeviceProcAddr" => get_device_proc as usize,
        b"vkCreateInstance" => create_instance as usize,
        b"vkDestroyInstance" => destroy_instance as usize,
        b"vkCreateDevice" => create_device as usize,
        b"vkDestroyDevice" => destroy_device as usize,
        b"vkCreateAndroidSurfaceKHR" => create_android_surface as usize,
        b"vkEnumerateInstanceExtensionProperties" => instance_extensions as usize,
        b"vkQueuePresentKHR" => queue_present as usize,
        n if n.starts_with(b"vk") => {
            let instance = g.instance().map_or(vk::Instance::null(), |i|i.native.handle());
            // SAFETY: the borrowed CStr is valid; absent instance limits the
            // resolver to Vulkan's global commands rather than fake handles.
            let address = unsafe { get_instance_proc(instance, name.as_ptr()) }.map_or(0, |f|f as usize);
            if address != 0 { return address; }
            static EXPORTS: OnceLock<Result<libloading::Library, String>> = OnceLock::new();
            let library = EXPORTS.get_or_init(|| {
                // SAFETY: trusted native loader already validated/pinned by Entry;
                // the explicit export owner keeps dlsym addresses process-live.
                unsafe { libloading::Library::new("libvulkan.so.1") }.map_err(|e|e.to_string())
            });
            if !n.get(2).is_some_and(u8::is_ascii_uppercase) { return 0; }
            // SAFETY: public native Vulkan command export, returned as an opaque
            // address only. Its consumer uses the command's actual Vulkan ABI;
            // this zero-argument carrier is never invoked as such here.
            library.as_ref().ok().and_then(|library| unsafe {
                library.get::<unsafe extern "system" fn()>(name.to_bytes_with_nul()).ok()
            }).map_or(0, |symbol| *symbol as usize)
        }
        _ => super::guest_graphics::egl_link_symbol(name),
    }
}

/// # Safety
/// Native count/output follow vkEnumerateInstanceExtensionProperties; names are NUL strings.
unsafe extern "system" fn instance_extensions(layer: *const c_char, count: *mut u32,
    output: *mut vk::ExtensionProperties) -> vk::Result {
    let Some(g) = active() else { return vk::Result::ERROR_INITIALIZATION_FAILED; };
    if count.is_null() { return vk::Result::ERROR_INITIALIZATION_FAILED; }
    // SAFETY: bounded native enumeration and caller-provided output lease.
    unsafe {
        if !layer.is_null() { return (g.entry.fp_v1_0().enumerate_instance_extension_properties)(layer, count, output); }
        let mut extensions = match g.entry.enumerate_instance_extension_properties(None) { Ok(v) => v, Err(e) => return e };
        let extra = match g.entry.enumerate_instance_extension_properties(Some(LAYER)) { Ok(v) => v, Err(e) => return e };
        for extension in extra {
            if !extensions.iter().any(|e| e.extension_name == extension.extension_name) { extensions.push(extension); }
        }
        extensions.retain(|e| e.extension_name_as_c_str().ok() != Some(ash::khr::wayland_surface::NAME));
        let mut android = vk::ExtensionProperties::default();
        let name = ash::khr::android_surface::NAME.to_bytes_with_nul();
        for (to, &from) in android.extension_name.iter_mut().zip(name) { *to = from as c_char; }
        android.spec_version = ash::khr::android_surface::SPEC_VERSION;
        if !extensions.iter().any(|e|e.extension_name == android.extension_name) { extensions.push(android); }
        if output.is_null() { *count = extensions.len() as u32; return vk::Result::SUCCESS; }
        let size = (*count as usize).min(extensions.len());
        std::ptr::copy_nonoverlapping(extensions.as_ptr(), output, size);
        *count = size as u32;
        if size < extensions.len() { vk::Result::INCOMPLETE } else { vk::Result::SUCCESS }
    }
}
/// # Safety
/// Exact Vulkan create ABI: input chains/strings readable for this call, output writable;
/// allocator callbacks/guest code stay valid for their Vulkan object lifetime.
unsafe extern "system" fn create_instance(info: *const vk::InstanceCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>, output: *mut vk::Instance) -> vk::Result {
    let Some(g) = active() else { return vk::Result::ERROR_INITIALIZATION_FAILED; };
    if info.is_null() || output.is_null() { return vk::Result::ERROR_INITIALIZATION_FAILED; }
    let Some(_claim) = Creation::claim(g.clone(), false) else { return vk::Result::ERROR_TOO_MANY_OBJECTS; };
    // SAFETY: native borrowed C input; local pointer arrays survive through the
    // synchronous native call. Vulkan copies their values, including callbacks.
    unsafe {
        *output = vk::Instance::null();
        let original = &*info;
        if original.s_type != vk::StructureType::INSTANCE_CREATE_INFO || original.enabled_extension_count > 128
            || original.enabled_layer_count > 32 || (original.enabled_extension_count != 0 && original.pp_enabled_extension_names.is_null())
            || (original.enabled_layer_count != 0 && original.pp_enabled_layer_names.is_null()) { return vk::Result::ERROR_INITIALIZATION_FAILED; }
        let mut extensions = Vec::with_capacity(original.enabled_extension_count as usize + 1);
        let mut android = false;
        for i in 0..original.enabled_extension_count as usize {
            let name = *original.pp_enabled_extension_names.add(i);
            if name.is_null() { return vk::Result::ERROR_EXTENSION_NOT_PRESENT; }
            let name = if CStr::from_ptr(name) == ash::khr::android_surface::NAME { android = true; ash::khr::wayland_surface::NAME.as_ptr() } else { name };
            if !extensions.iter().any(|p|CStr::from_ptr(*p) == CStr::from_ptr(name)) { extensions.push(name); }
        }
        let mut layers = Vec::with_capacity(original.enabled_layer_count as usize + 1);
        for i in 0..original.enabled_layer_count as usize {
            let name = *original.pp_enabled_layer_names.add(i);
            if name.is_null() { return vk::Result::ERROR_LAYER_NOT_PRESENT; }
            layers.push(name);
        }
        if !layers.iter().any(|p| CStr::from_ptr(*p) == LAYER) { layers.push(LAYER.as_ptr()); }
        let mut adjusted = *original;
        adjusted.enabled_extension_count = extensions.len() as u32; adjusted.pp_enabled_extension_names = extensions.as_ptr();
        adjusted.enabled_layer_count = layers.len() as u32; adjusted.pp_enabled_layer_names = layers.as_ptr();
        let result = (g.entry.fp_v1_0().create_instance)(&adjusted, allocator, output);
        if result == vk::Result::SUCCESS {
            let native = ash::Instance::load(g.entry.static_fn(), *output);
            g.state.lock().unwrap_or_else(|p|p.into_inner()).instance = Some(Arc::new(Instance { native, android_surface: android }));
            eprintln!("[VULKAN] guest instance created; Android surface translated={android}");
        }
        result
    }
}
/// # Safety
/// Caller has stopped all users, destroyed child objects and lends compatible allocator callbacks.
unsafe extern "system" fn destroy_instance(instance: vk::Instance, allocator: *const vk::AllocationCallbacks<'_>) {
    let Some(g) = active() else { return; };
    let Some(own) = g.instance().filter(|i|i.native.handle() == instance) else { return; };
    // SAFETY: quiescent valid instance under the caller's Vulkan destroy contract.
    unsafe { (own.native.fp_v1_0().destroy_instance)(instance, allocator) };
    g.state.lock().unwrap_or_else(|p|p.into_inner()).instance = None;
}
/// # Safety
/// Physical device belongs to the live guest instance; C input chains/output/allocator
/// follow vkCreateDevice and all native callbacks remain valid for their objects.
unsafe extern "system" fn create_device(physical: vk::PhysicalDevice, info: *const vk::DeviceCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>, output: *mut vk::Device) -> vk::Result {
    let Some(g) = active() else { return vk::Result::ERROR_INITIALIZATION_FAILED; };
    let Some(instance) = g.instance() else { return vk::Result::ERROR_INITIALIZATION_FAILED; };
    if info.is_null() || output.is_null() { return vk::Result::ERROR_INITIALIZATION_FAILED; }
    let Some(_claim) = Creation::claim(g.clone(), true) else { return vk::Result::ERROR_TOO_MANY_OBJECTS; };
    // SAFETY: borrowed Vulkan create ABI, pinned instance table; native function
    // pointers are loaded for this exact returned device before publishing it.
    unsafe {
        *output = vk::Device::null();
        let result = (instance.native.fp_v1_0().create_device)(physical, info, allocator, output);
        if result == vk::Result::SUCCESS {
            let native = ash::Device::load(instance.native.fp_v1_0(), *output);
            let raw = (instance.native.fp_v1_0().get_device_proc_addr)(*output, c"vkQueuePresentKHR".as_ptr());
            let present = raw.map(|f|std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkQueuePresentKHR>(f));
            g.state.lock().unwrap_or_else(|p|p.into_inner()).device = Some(Arc::new(Device { native, present }));
            eprintln!("[VULKAN] guest native device created; presentation entry={}", present.is_some());
        }
        result
    }
}
/// # Safety
/// All device work is idle, children destroyed and allocator compatible with creation.
unsafe extern "system" fn destroy_device(device: vk::Device, allocator: *const vk::AllocationCallbacks<'_>) {
    let Some(g) = active() else { return; };
    let Some(own) = g.device().filter(|d|d.native.handle() == device) else { return; };
    // SAFETY: exact live native device under Vulkan's destruction contract.
    unsafe { (own.native.fp_v1_0().destroy_device)(device, allocator) };
    g.state.lock().unwrap_or_else(|p|p.into_inner()).device = None;
    eprintln!("[VULKAN] guest device destroyed; successful_presents={}", g.presents.load(Ordering::Relaxed));
}
/// # Safety
/// Native Android create info/output are valid and the ANativeWindow lease remains
/// live for the call. Preserve the caller's allocation callbacks without translation.
unsafe extern "system" fn create_android_surface(instance: vk::Instance, info: *const vk::AndroidSurfaceCreateInfoKHR<'_>,
    allocator: *const vk::AllocationCallbacks<'_>, output: *mut vk::SurfaceKHR) -> vk::Result {
    let Some(g) = active() else { return vk::Result::ERROR_SURFACE_LOST_KHR; };
    let Some(own) = g.instance().filter(|i|i.native.handle() == instance && i.android_surface) else { return vk::Result::ERROR_EXTENSION_NOT_PRESENT; };
    if info.is_null() || output.is_null() { return vk::Result::ERROR_INITIALIZATION_FAILED; }
    // SAFETY: readable native create info, known process-owned window token and
    // SDL native resources guaranteed live by activation. No pNext is guessed.
    unsafe {
        if (*info).s_type != vk::StructureType::ANDROID_SURFACE_CREATE_INFO_KHR || !(*info).p_next.is_null()
            || (*info).window as usize != Arc::as_ptr(&g) as usize || !(*info).flags.is_empty() { return vk::Result::ERROR_SURFACE_LOST_KHR; }
        let create = vk::WaylandSurfaceCreateInfoKHR::default()
            .display(g.native.display as *mut vk::wl_display).surface(g.native.surface as *mut vk::wl_surface);
        let api = ash::khr::wayland_surface::Instance::new(g.entry, &own.native);
        let result = (api.fp().create_wayland_surface_khr)(instance, &create, allocator, output);
        eprintln!("[VULKAN] Android/Wayland surface: {result:?}");
        result
    }
}
/// # Safety
/// Queue belongs to the single live native device. Present info, synchronization
/// objects and writable per-swapchain results follow vkQueuePresentKHR's contract.
unsafe extern "system" fn queue_present(queue: vk::Queue, info: *const vk::PresentInfoKHR<'_>) -> vk::Result {
    let Some(g) = active() else { return vk::Result::ERROR_DEVICE_LOST; };
    let Some(call) = g.device().and_then(|d|d.present) else { return vk::Result::ERROR_EXTENSION_NOT_PRESENT; };
    // SAFETY: exact native presentation prototype; no state lock crosses driver calls.
    let result = unsafe { call(queue, info) };
    if matches!(result, vk::Result::SUCCESS | vk::Result::SUBOPTIMAL_KHR) {
        if g.presents.fetch_add(1, Ordering::Relaxed) == 0 {
            super::render_thread::first_present();
            eprintln!("[VULKAN] first guest frame presented");
        }
        if let Some(presented) = g.presented { presented(); }
    }
    result
}

/// # Safety
/// NDK callers pair each fromSurface/acquire with one release. Opaque leases may
/// outlive activation, but callers stop using native resources before shutdown.
unsafe extern "C" fn from_surface(_: *mut c_void, surface: *mut c_void) -> *mut c_void {
    if surface.is_null() { return std::ptr::null_mut(); }
    active().map_or(std::ptr::null_mut(), |g|Arc::into_raw(g) as *mut c_void)
}
/// # Safety
/// Caller owns a live lease produced by this backend, excluding its last release.
unsafe extern "C" fn acquire(window: *mut c_void) {
    // SAFETY: increments one caller-owned live Arc lease, not an SDL native pointer.
    if !window.is_null() { unsafe { Arc::increment_strong_count(window.cast::<Graphics>()) }; }
}
/// # Safety
/// Exactly one release for a caller-owned lease; never a token from another backend.
unsafe extern "C" fn release(window: *mut c_void) {
    // SAFETY: caller transfers back one Arc reference while its code stays pinned.
    if !window.is_null() { unsafe { Arc::decrement_strong_count(window.cast::<Graphics>()) }; }
}
fn window_state(window: *mut c_void) -> Option<Arc<Graphics>> { active().filter(|g|Arc::as_ptr(g) as usize == window as usize) }
extern "C" fn width(window: *mut c_void) -> i32 { window_state(window).map_or(-19, |g|g.dimensions.width) }
extern "C" fn height(window: *mut c_void) -> i32 { window_state(window).map_or(-19, |g|g.dimensions.height) }
extern "C" fn format(window: *mut c_void) -> i32 { window_state(window).map_or(-19, |_|1) }
extern "C" fn geometry(window: *mut c_void, w: i32, h: i32, format: i32) -> i32 {
    let Some(g) = window_state(window) else { return -19; };
    if !matches!(format, 0 | 1) || !((w == 0 && h == 0) || (w == g.dimensions.width && h == g.dimensions.height)) { return -95; }
    0
}
