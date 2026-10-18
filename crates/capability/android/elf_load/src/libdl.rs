//! INPUT:  shared exec-image Arc snapshot, immutable ELF metadata and native GNU module iterator
//! OUTPUT:  scoped libdl.so/LIBC queries, reopen/close and 32-byte program-header iteration
//! POS:    exposes the loader's authoritative modules without copied headers or per-thread scratch
use super::elf::SymbolVersion;
use std::cell::Cell;
use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};

#[path = "native_companions.rs"]
mod native_companions;

#[repr(C)]
pub(crate) struct DlInfo {
    pub filename: *const c_char,
    pub base: *mut c_void,
    pub symbol_name: *const c_char,
    pub symbol_address: *mut c_void,
}
thread_local! { static ERROR: Cell<*const c_char> = const { Cell::new(std::ptr::null()) }; }
thread_local! {static ERROR_DETAILS:RefCell<Option<CString>>=const{RefCell::new(None)};}
static PROCESS_HANDLE: u8 = 0;
static SYNTHETIC_HANDLE: u8 = 0;
fn trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("RUDROID_TRACE_LIBDL").is_some())
}
fn trace(message: std::fmt::Arguments<'_>) {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 96 {
        eprintln!("[ELF-DL] {message}");
    }
}
fn fail(message: &'static CStr) {
    ERROR_DETAILS.with(|value| value.replace(None));
    ERROR.with(|error| error.set(message.as_ptr()));
}
fn fail_owned(message: String) {
    let message =
        CString::new(message).unwrap_or_else(|_| c"native companion error contains NUL".into());
    ERROR.with(|error| error.set(message.as_ptr()));
    ERROR_DETAILS.with(|value| value.replace(Some(message)));
}
fn synthetic_handle() -> *mut c_void {
    (&SYNTHETIC_HANDLE as *const u8).cast_mut().cast()
}
fn synthetic(name: &CStr) -> bool {
    if name == c"libvulkan.so" {
        return java_service_available(name);
    }
    matches!(
        name.to_bytes(),
        b"libandroid.so"
            | b"liblog.so"
            | b"libEGL.so"
            | b"libGLESv2.so"
            | b"libGLESv1_CM.so"
            | b"libc.so"
            | b"libm.so"
            | b"libdl.so"
            | b"libz.so"
    )
}
// System.load may refer to an Android service supplied by a capability instead
// of a separate ELF image. Check the actual provider, not merely a fake handle.
pub(crate) fn java_service_available(name: &CStr) -> bool {
    let probe = match name.to_bytes() {
        b"libc" | b"libc.so" => c"malloc",
        b"libm" | b"libm.so" => c"sin",
        b"libdl" | b"libdl.so" => c"dlopen",
        b"liblog" | b"liblog.so" => c"__android_log_print",
        b"libandroid" | b"libandroid.so" => c"AAssetManager_fromJava",
        b"libEGL" | b"libEGL.so" => c"eglGetDisplay",
        b"libGLESv2" | b"libGLESv2.so" => c"glGetString",
        b"libGLESv1_CM" | b"libGLESv1_CM.so" => c"glOrthof",
        b"libvulkan" | b"libvulkan.so" => c"vkGetInstanceProcAddr",
        b"libOpenSLES" | b"libOpenSLES.so" => c"slCreateEngine",
        b"libz" | b"libz.so" => c"zlibVersion",
        _ => return false,
    };
    super::exec::dynamic_symbol(probe).is_some()
}

fn process_handle() -> *mut c_void {
    (&PROCESS_HANDLE as *const u8).cast_mut().cast()
}
unsafe extern "C" fn error() -> *mut c_char {
    ERROR
        .with(|error| error.replace(std::ptr::null()))
        .cast_mut()
}
unsafe extern "C" fn open(filename: *const c_char, flags: c_int) -> *mut c_void {
    // Android ARM64 flags. Reopen globally published load-plan images;
    // DT_NEEDED discovery and RTLD_LOCAL namespaces are separate work.
    if flags & !(1 | 2 | 4 | 0x100) != 0 || flags & 3 == 3 {
        fail(c"unsupported guest dlopen flags");
        return std::ptr::null_mut();
    }
    if filename.is_null() {
        return process_handle();
    }
    let filename = CStr::from_ptr(filename);
    if trace_enabled() { trace(format_args!("dlopen {filename:?} flags={flags:#x}")); }
    match super::exec::reopen_guest(filename) {
        Some(handle) => handle.cast(),
        None => {
            if native_companions::approved(filename).is_some() {
                return match native_companions::open(filename, flags & 4 != 0) {
                    Ok(handle) => handle,
                    Err(reason) => {
                        fail_owned(reason);
                        std::ptr::null_mut()
                    }
                };
            }
            if synthetic(filename) {
                return synthetic_handle();
            }
            fail(c"guest library is absent from the load plan");
            std::ptr::null_mut()
        }
    }
}
unsafe extern "C" fn close(handle: *mut c_void) -> c_int {
    if handle == process_handle() || handle == synthetic_handle() {
        return 0;
    }
    if native_companions::close(handle) {
        return 0;
    }
    if !handle.is_null() && super::exec::close_guest(handle.cast()) {
        0
    } else {
        fail(c"invalid guest dlclose handle");
        -1
    }
}
unsafe extern "C" fn lookup(handle: *mut c_void, name: *const c_char) -> *mut c_void {
    if name.is_null() {
        fail(c"null guest dlsym name");
        return std::ptr::null_mut();
    }
    let name = CStr::from_ptr(name);
    let address = if handle.is_null() || handle == process_handle() || handle == synthetic_handle()
    {
        super::exec::dynamic_symbol(name)
    } else if handle as usize == usize::MAX {
        fail(c"RTLD_NEXT requires an unsupported caller-specific lookup scope");
        return std::ptr::null_mut();
    } else if native_companions::is_handle(handle) {
        // Android/graphics wrappers keep precedence over genuine native companions.
        super::exec::dynamic_symbol(name).or_else(|| native_companions::symbol(handle, name))
    } else {
        name.to_str()
            .ok()
            .and_then(|name| super::exec::guest_handle_symbol(handle.cast(), name))
    };
    if address.is_none() && trace_enabled() { trace(format_args!("dlsym {name:?} handle={handle:p} found=false")); }
    match address {
        Some(address) => address as *mut c_void,
        None => {
            fail(c"guest symbol or handle was not found");
            std::ptr::null_mut()
        }
    }
}
unsafe extern "C" fn address_info(address: *const c_void, output: *mut DlInfo) -> c_int {
    if output.is_null() {
        return 0;
    }
    if let Some(info) = super::exec::guest_address_info(address as usize) {
        output.write(info);
        return 1;
    }
    // GNU and ARM64 Bionic share the four-pointer Dl_info public layout.
    let mut info: libc::Dl_info = std::mem::zeroed();
    if libc::dladdr(address, &mut info) == 0 {
        return 0;
    }
    output.write(DlInfo {
        filename: info.dli_fname,
        base: info.dli_fbase,
        symbol_name: info.dli_sname,
        symbol_address: info.dli_saddr,
    });
    1
}

#[repr(C)]
pub(crate) struct PhdrInfo {
    pub addr: usize,
    pub name: *const c_char,
    pub phdr: *const libc::Elf64_Phdr,
    pub phnum: u16,
}
type Callback = unsafe extern "C" fn(*mut PhdrInfo, usize, *mut c_void) -> c_int;

#[cfg(all(target_arch = "aarch64", target_env = "gnu"))]
unsafe extern "C" fn iterate(callback: Option<Callback>, user: *mut c_void) -> c_int {
    let Some(callback) = callback else {
        *libc::__errno_location() = libc::EINVAL;
        return -1;
    };
    // One Arc clone pins every image/metadata/name for this whole iteration.
    // No registry lock remains held when guest callbacks execute or reenter.
    let chain = super::exec::guest_image_chain();
    struct Context {
        callback: Callback,
        user: *mut c_void,
    }
    unsafe extern "C" fn host(
        info: *mut libc::dl_phdr_info,
        _size: usize,
        data: *mut c_void,
    ) -> c_int {
        let context = &*data.cast::<Context>();
        let mut prefix = PhdrInfo {
            addr: (*info).dlpi_addr as usize,
            name: (*info).dlpi_name,
            phdr: (*info).dlpi_phdr,
            phnum: (*info).dlpi_phnum,
        };
        (context.callback)(&mut prefix, std::mem::size_of::<PhdrInfo>(), context.user)
    }
    let mut context = Context { callback, user };
    let result = libc::dl_iterate_phdr(Some(host), (&mut context as *mut Context).cast());
    if result != 0 {
        return result;
    }
    for module in chain.iter().rev() {
        let mut info = module.phdr_info();
        let result = callback(&mut info, std::mem::size_of::<PhdrInfo>(), user);
        if result != 0 {
            return result;
        }
    }
    0
}
pub(crate) fn symbol(name: &str, version: Option<SymbolVersion<'_>>) -> usize {
    #[cfg(all(target_arch = "aarch64", target_env = "gnu"))]
    if version.is_none_or(|v| v.file == "libdl.so" && v.name == "LIBC") {
        return match name {
            "dl_iterate_phdr" => iterate as *const () as usize,
            "dlopen" => open as *const () as usize,
            "dlclose" => close as *const () as usize,
            "dlsym" => lookup as *const () as usize,
            "dlerror" => error as *const () as usize,
            "dladdr" => address_info as *const () as usize,
            _ => 0,
        };
    }
    let _ = (name, version);
    0
}

#[cfg(all(test, target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/libdl.rs"]
mod tests;
