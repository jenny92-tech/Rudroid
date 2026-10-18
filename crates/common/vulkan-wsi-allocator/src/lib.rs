//! INPUT:  upstream wsialloc v3 requests and the target's existing GBM driver
//! OUTPUT:  exact-layout single-plane DMA-BUFs, with fd ownership transferred only on success
//! POS:    Rust native allocation boundary for optional Wayland Vulkan presentation
#![cfg(target_os = "linux")]

use std::{ffi::c_void, fs::{File, OpenOptions}, os::fd::{AsRawFd, IntoRawFd, OwnedFd, FromRawFd},
    sync::{Mutex, OnceLock}};

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct Format { pub fourcc: u32, pub modifier: u64, pub flags: u64 }

#[repr(C)]
pub struct AllocateInfo {
    pub formats: *const Format,
    pub format_count: u32,
    pub width: u32,
    pub height: u32,
    pub flags: u64,
}

#[repr(C)]
pub struct AllocateResult {
    pub format: Format,
    pub average_row_strides: [i32; 4],
    pub offsets: [u32; 4],
    pub buffer_fds: [i32; 4],
    pub is_disjoint: bool,
}
impl Default for AllocateResult {
    fn default() -> Self {
        Self { format: Format::default(), average_row_strides: [0; 4], offsets: [0; 4],
            buffer_fds: [-1; 4], is_disjoint: false }
    }
}

type Handle = *mut c_void;
struct Gbm {
    _library: libloading::Library,
    create_device: unsafe extern "C" fn(i32) -> Handle,
    destroy_device: unsafe extern "C" fn(Handle),
    create: unsafe extern "C" fn(Handle, u32, u32, u32, *const u64, u32) -> Handle,
    destroy: unsafe extern "C" fn(Handle),
    modifier: unsafe extern "C" fn(Handle) -> u64,
    format: unsafe extern "C" fn(Handle) -> u32,
    planes: unsafe extern "C" fn(Handle) -> i32,
    stride: unsafe extern "C" fn(Handle) -> u32,
    offset: unsafe extern "C" fn(Handle, i32) -> u32,
    fd: unsafe extern "C" fn(Handle) -> i32,
}
impl Gbm {
    fn load() -> Result<&'static Self, &'static str> {
        static TABLE: OnceLock<Result<Gbm, &'static str>> = OnceLock::new();
        TABLE.get_or_init(|| {
            // SAFETY: trusted system GBM C ABI. The library and every copied
            // function remain owned by this process-pinned immutable table.
            unsafe {
                let library = libloading::Library::new("libgbm.so.1").map_err(|_| "GBM library")?;
                macro_rules! symbol { ($name:literal) => { *library.get(concat!($name, "\0").as_bytes()).map_err(|_| $name)? }; }
                Ok(Self {
                    create_device: symbol!("gbm_create_device"), destroy_device: symbol!("gbm_device_destroy"),
                    create: symbol!("gbm_bo_create_with_modifiers"), destroy: symbol!("gbm_bo_destroy"),
                    modifier: symbol!("gbm_bo_get_modifier"), format: symbol!("gbm_bo_get_format"),
                    planes: symbol!("gbm_bo_get_plane_count"), stride: symbol!("gbm_bo_get_stride"),
                    offset: symbol!("gbm_bo_get_offset"), fd: symbol!("gbm_bo_get_fd"), _library: library,
                })
            }
        }).as_ref().map_err(|e| *e)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Layout { width: u32, height: u32, fourcc: u32, modifier: u64, stride: u32 }
pub struct Allocator { gbm: &'static Gbm, device: usize, _drm: File, gate: Mutex<Option<Layout>> }
impl Drop for Allocator {
    fn drop(&mut self) {
        // SAFETY: unique allocator destruction follows native caller quiescence;
        // every BO was destroyed synchronously, and the DRM File still lives.
        unsafe { (self.gbm.destroy_device)(self.device as Handle) };
    }
}
struct Buffer<'a> { owner: &'a Allocator, handle: Handle }
impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        // SAFETY: a non-null native BO owned by this guard; its device and table
        // outlive it. Exported DMA-BUF fds own independent kernel references.
        unsafe { (self.owner.gbm.destroy)(self.handle) };
    }
}

fn valid_geometry(info: &AllocateInfo) -> bool {
    (1..=8192).contains(&info.width) && (1..=8192).contains(&info.height)
        && (1..=256).contains(&info.format_count) && info.flags == 0 && !info.formats.is_null()
}
fn supported_format(format: Format) -> bool {
    // ABGR/ARGB and their X variants: exactly one 32-bit format plane.
    [0x34324241, 0x34325241, 0x34324258, 0x34325258].contains(&format.fourcc)
        && format.flags & !1 == 0 && format.modifier != 0x00ff_ffff_ffff_ffff
}

fn valid_drm_path(path: &str) -> bool {
    path.strip_prefix("/dev/dri/renderD").and_then(|s| s.parse::<u16>().ok())
        .is_some_and(|n| (128..=255).contains(&n))
        || path.strip_prefix("/dev/dri/card").and_then(|s| s.parse::<u16>().ok())
            .is_some_and(|n| n <= 15)
}

fn open_drm_with(explicit: Option<&str>, mut open: impl FnMut(&str) -> std::io::Result<File>) -> std::io::Result<File> {
    if let Some(path) = explicit {
        if !valid_drm_path(path) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "expected a DRM device node"));
        }
        return open(path);
    }
    match open("/dev/dri/renderD128") {
        // Proprietary Mali firmware may expose GBM through the primary card
        // with no render node. Buffer allocation requires no DRM master lease.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => open("/dev/dri/card0"),
        result => result,
    }
}

#[no_mangle]
pub static wsialloc_symbol_version_3: u32 = 3;

/// # Safety
/// `output` is null or writable for one allocator pointer. A successful handle
/// must be deleted exactly once, after all allocation calls have stopped.
#[no_mangle]
pub unsafe extern "C" fn wsialloc_new(output: *mut *mut Allocator) -> i32 {
    if output.is_null() { return -1; }
    // SAFETY: writable native output under the function contract.
    unsafe { *output = std::ptr::null_mut() };
    let Ok(gbm) = Gbm::load() else { return -3; };
    let explicit = std::env::var("RUDROID_VULKAN_GBM_DEVICE").ok();
    let drm = match open_drm_with(explicit.as_deref(), |path| OpenOptions::new().read(true).write(true).open(path)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => return -1,
        Err(_) => return -3,
    };
    // SAFETY: open DRM fd stays owned by Allocator; exact native GBM prototype.
    let device = unsafe { (gbm.create_device)(drm.as_raw_fd()) };
    if device.is_null() { return -3; }
    let owner = Box::new(Allocator { gbm, device: device as usize, _drm: drm, gate: Mutex::new(None) });
    // SAFETY: a unique Box is transferred to the native caller; delete reclaims it.
    unsafe { *output = Box::into_raw(owner) };
    0
}

/// # Safety
/// Null or a unique live result of wsialloc_new, with every allocation call ended.
#[no_mangle]
pub unsafe extern "C" fn wsialloc_delete(allocator: *mut Allocator) {
    if !allocator.is_null() {
        // SAFETY: caller transfers the unique Box back exactly once, quiescent.
        unsafe { drop(Box::from_raw(allocator)) };
    }
}

/// # Safety
/// Allocator is live for the call; info and its bounded format array are readable;
/// output is uniquely writable. The caller assumes ownership of returned fds only
/// on success, then closes or transfers each exactly once under the wsialloc ABI.
#[no_mangle]
pub unsafe extern "C" fn wsialloc_alloc(allocator: *mut Allocator, info: *const AllocateInfo,
    output: *mut AllocateResult) -> i32 {
    if allocator.is_null() || info.is_null() || output.is_null() { return -1; }
    // SAFETY: borrowed live owner/input and unique output follow the native ABI;
    // all native object guards below end before this owner borrow ends.
    unsafe {
        *output = AllocateResult::default();
        let owner = &*allocator;
        let info = &*info;
        if !valid_geometry(info) { return -1; }
        let formats = std::slice::from_raw_parts(info.formats, info.format_count as usize);
        let mut layout = owner.gate.lock().unwrap_or_else(|p| p.into_inner());
        for &format in formats {
            if !supported_format(format) { continue; }
            let handle = (owner.gbm.create)(owner.device as Handle, info.width, info.height,
                format.fourcc, &format.modifier, 1);
            if handle.is_null() { continue; }
            let buffer = Buffer { owner, handle };
            if (owner.gbm.modifier)(handle) != format.modifier || (owner.gbm.format)(handle) != format.fourcc
                || (owner.gbm.planes)(handle) != 1 || (owner.gbm.offset)(handle, 0) != 0 { continue; }
            let stride = (owner.gbm.stride)(handle);
            if stride == 0 || stride > i32::MAX as u32 { continue; }
            let selected = Layout { width: info.width, height: info.height, fourcc: format.fourcc,
                modifier: format.modifier, stride };
            // The upstream swapchain reuses its first image's layout for all
            // images. Refuse a driver layout change before exporting any fd.
            if layout.is_some_and(|previous| previous != selected) { continue; }
            let fd = (owner.gbm.fd)(handle);
            if fd < 0 { continue; }
            let fd = OwnedFd::from_raw_fd(fd);
            // Complete every fallible operation before the single ownership transfer.
            drop(buffer);
            *layout = Some(selected);
            *output = AllocateResult { format, average_row_strides: [stride as i32, 0, 0, 0],
                offsets: [0; 4], buffer_fds: [fd.into_raw_fd(), -1, -1, -1], is_disjoint: false };
            return 0;
        }
    }
    -2
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
