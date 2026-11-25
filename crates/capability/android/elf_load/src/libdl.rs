//! INPUT:  shared exec-image Arc snapshot, immutable ELF metadata and native GNU module iterator
//! OUTPUT:  scoped libdl.so/LIBC queries, reopen/close and 32-byte program-header iteration
//! POS:    exposes the loader's authoritative modules without copied headers or per-thread scratch
use super::elf::SymbolVersion;
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

#[path = "native_companions.rs"]
mod native_companions;
#[path = "native_alias.rs"]
mod native_alias;

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
const TRACE_SYMBOLS_ENV: &str = "RUDROID_TRACE_LIBDL_SYMBOLS";
const MAX_TRACE_FILTER_NAMES: usize = 16;
const MAX_TRACE_FILTER_NAME_BYTES: usize = 128;
const MAX_TRACE_DIAGNOSED_NAMES: usize = 64;
const MAX_TRACE_CHAIN_MODULES: usize = 8;
const MAX_TRACE_MODULE_REPORTS: usize = 32;

#[derive(Debug, Default, PartialEq, Eq)]
struct TraceFilter {
    names: Vec<String>,
}

impl TraceFilter {
    fn parse(value: &str) -> Self {
        let mut names = Vec::new();
        for entry in value.split(',').take(MAX_TRACE_FILTER_NAMES) {
            let name = entry.trim();
            if name.is_empty()
                || name.len() > MAX_TRACE_FILTER_NAME_BYTES
                || names
                    .iter()
                    .any(|selected: &String| selected.as_str() == name)
            {
                continue;
            }
            names.push(name.to_owned());
        }
        Self { names }
    }

    fn matches(&self, name: &str) -> bool {
        self.names
            .iter()
            .any(|selected| selected.as_str() == name)
    }
}

fn trace_filter() -> Option<&'static TraceFilter> {
    static FILTER: OnceLock<Option<TraceFilter>> = OnceLock::new();
    FILTER
        .get_or_init(|| match std::env::var(TRACE_SYMBOLS_ENV) {
            Ok(value) => Some(TraceFilter::parse(&value)),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => Some(TraceFilter::default()),
        })
        .as_ref()
}

fn diagnostic_requested(filter: Option<&TraceFilter>, name: &str, found: bool) -> bool {
    match filter {
        Some(filter) => filter.matches(name),
        None => !found,
    }
}

#[derive(Debug)]
struct TraceWork {
    names: BTreeSet<String>,
    name_limit: usize,
    module_reports_remaining: usize,
}

impl TraceWork {
    fn new() -> Self {
        Self {
            names: BTreeSet::new(),
            name_limit: MAX_TRACE_DIAGNOSED_NAMES,
            module_reports_remaining: MAX_TRACE_MODULE_REPORTS,
        }
    }

    #[cfg(test)]
    fn with_limits(name_limit: usize, module_reports: usize) -> Self {
        Self {
            names: BTreeSet::new(),
            name_limit,
            module_reports_remaining: module_reports,
        }
    }

    fn claim_name(&mut self, name: &str) -> bool {
        if self.module_reports_remaining == 0
            || name.is_empty()
            || name.len() > MAX_TRACE_FILTER_NAME_BYTES
            || self.names.contains(name)
            || self.names.len() >= self.name_limit
        {
            return false;
        }
        self.names.insert(name.to_owned());
        true
    }

    fn claim_module(&mut self) -> bool {
        let Some(remaining) = self.module_reports_remaining.checked_sub(1) else {
            return false;
        };
        self.module_reports_remaining = remaining;
        true
    }
}

fn trace_work() -> &'static Mutex<TraceWork> {
    static WORK: OnceLock<Mutex<TraceWork>> = OnceLock::new();
    WORK.get_or_init(|| Mutex::new(TraceWork::new()))
}

fn claim_trace_name(name: &str) -> bool {
    trace_work()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .claim_name(name)
}

fn claim_trace_module() -> bool {
    trace_work()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .claim_module()
}

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
fn trace_live(diagnostic: impl std::fmt::Display) {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 32 {
        eprintln!("{diagnostic}");
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
    if matches!(name.to_bytes(), b"libvulkan.so" | b"libGLESv2.so") {
        return java_service_available(name);
    }
    matches!(
        name.to_bytes(),
        b"libandroid.so"
            | b"liblog.so"
            | b"libEGL.so"
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
    if let Some(handle) = native_alias::open(filename) {
        return handle;
    }
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
    if native_alias::close(handle) || native_companions::close(handle) {
        return 0;
    }
    if !handle.is_null() && super::exec::close_guest(handle.cast()) {
        0
    } else {
        fail(c"invalid guest dlclose handle");
        -1
    }
}
// A native companion is lower priority than the Android/guest namespace, and
// is eligible only on a genuine miss, never after a rejected definition.
fn companion_fallback(
    result: Result<Option<usize>, super::ExportReject>,
    fallback: impl FnOnce() -> Option<usize>,
) -> Result<Option<usize>, super::ExportReject> {
    result.map(|address| address.or_else(fallback))
}

unsafe extern "C" fn lookup(handle: *mut c_void, name: *const c_char) -> *mut c_void {
    if name.is_null() {
        fail(c"null guest dlsym name");
        return std::ptr::null_mut();
    }
    let name = CStr::from_ptr(name);
    let address = if handle.is_null() || handle == process_handle() || handle == synthetic_handle()
    {
        super::exec::dynamic_symbol_lookup(name)
    } else if handle as usize == usize::MAX {
        fail(c"RTLD_NEXT requires an unsupported caller-specific lookup scope");
        return std::ptr::null_mut();
    } else if let Some(address) = native_alias::symbol(handle, name) {
        Ok((address != 0).then_some(address))
    } else if native_companions::is_handle(handle) {
        // Android/graphics wrappers keep precedence over ordinary native companions.
        companion_fallback(super::exec::dynamic_symbol_lookup(name), || native_companions::symbol(handle, name))
    } else {
        Ok(name.to_str().ok()
            .and_then(|name| super::exec::guest_handle_symbol(handle.cast(), name)))
    };
    let found = address.as_ref().is_ok_and(|address| address.is_some());
    if trace_enabled() {
        if !found {
            trace(format_args!("dlsym {name:?} handle={handle:p} found=false"));
        }
        if let Ok(name) = name.to_str() {
            if diagnostic_requested(trace_filter(), name, found) {
                let guest_scope = handle.is_null()
                    || handle == process_handle()
                    || handle == synthetic_handle()
                    || (!native_companions::is_handle(handle) && !native_alias::is_handle(handle));
                if guest_scope && claim_trace_name(name) {
                    if handle.is_null()
                        || handle == process_handle()
                        || handle == synthetic_handle()
                    {
                        super::exec::guest_chain_symbol_diagnostics(
                            name,
                            MAX_TRACE_CHAIN_MODULES,
                            claim_trace_module,
                            |diagnostic| trace_live(diagnostic),
                        );
                    } else if let Some(diagnostic) = super::exec::guest_handle_symbol_diagnostic(
                        handle.cast(),
                        name,
                        claim_trace_module,
                    ) {
                        trace_live(diagnostic);
                    }
                }
            }
        }
    }
    match address {
        Ok(Some(address)) => address as *mut c_void,
        Ok(None) => {
            fail(c"guest symbol or handle was not found");
            std::ptr::null_mut()
        }
        Err(reject) => {
            fail_owned(reject.describe(name.to_str().unwrap_or("<invalid symbol name>")));
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
            _ if version.is_none() => native_alias::import(name).unwrap_or(0),
            _ => 0,
        };
    }
    let _ = (name, version);
    0
}

pub(super) fn native_alias_available(name: &CStr) -> bool {
    native_alias::available(name)
}

use rudroid_common::{native_alias::GuestLibdl, registry::HostStr};

extern "C" fn capability_open(name: HostStr<'_>, flags: c_int) -> *mut c_void {
    // SAFETY: the pinned capability lends a valid HostStr for the call.
    unsafe { open(name.as_ptr(), flags) }
}

extern "C" fn capability_lookup(handle: *mut c_void, name: HostStr<'_>) -> *mut c_void {
    // SAFETY: the name is borrowed for the call and handles use the existing validated table.
    unsafe { lookup(handle, name.as_ptr()) }
}

#[no_mangle]
extern "C" fn rudroid_elf_guest_libdl() -> &'static GuestLibdl {
    static API: GuestLibdl = GuestLibdl {
        register_alias: native_alias::register,
        open: capability_open,
        lookup: capability_lookup,
    };
    &API
}

/// Publish the process-pinned guest libdl table for Mono / engine services.
pub(crate) fn publish_guest_libdl_service(host: &rudroid_common::host_api::HostApiV0) -> bool {
    host.register_service_typed(rudroid_elf_guest_libdl())
}

#[cfg(all(test, target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/libdl.rs"]
mod tests;

#[cfg(test)]
mod trace_tests {
    use super::*;

    #[test]
    fn symbol_filter_uses_bounded_exact_names() {
        let filter = TraceFilter::parse(" foo,bar,foo,, ");
        assert_eq!(filter.names, vec!["foo".to_owned(), "bar".to_owned()]);
        assert!(filter.matches("foo"));
        assert!(filter.matches("bar"));
        assert!(!filter.matches("fo"));
        assert!(!filter.matches("FOO"));
        assert!(!filter.matches("bar "));

        let too_many = (0..=MAX_TRACE_FILTER_NAMES)
            .map(|index| format!("name{index}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            TraceFilter::parse(&too_many).names.len(),
            MAX_TRACE_FILTER_NAMES
        );
        let overlong = "x".repeat(MAX_TRACE_FILTER_NAME_BYTES + 1);
        assert!(TraceFilter::parse(&overlong).names.is_empty());
    }

    #[test]
    fn default_is_miss_only_while_selected_hits_are_controls() {
        assert!(!diagnostic_requested(None, "foo", true));
        assert!(diagnostic_requested(None, "foo", false));

        let filter = TraceFilter::parse("foo");
        assert!(diagnostic_requested(Some(&filter), "foo", true));
        assert!(diagnostic_requested(Some(&filter), "foo", false));
        assert!(!diagnostic_requested(Some(&filter), "bar", false));

        let empty = TraceFilter::parse("");
        assert!(!diagnostic_requested(Some(&empty), "foo", true));
        assert!(!diagnostic_requested(Some(&empty), "foo", false));
    }

    #[test]
    fn selected_hit_reaches_diagnostic_work() {
        let filter = TraceFilter::parse("foo");
        let mut work = TraceWork::with_limits(4, 2);
        let mut computed = 0;

        if diagnostic_requested(Some(&filter), "foo", true)
            && work.claim_name("foo")
            && work.claim_module()
        {
            computed += 1;
        }

        assert_eq!(computed, 1);
    }

    #[test]
    fn selection_precedes_work_and_duplicates_do_not_starve_other_names() {
        let filter = TraceFilter::parse("foo,bar");
        let mut work = TraceWork::with_limits(4, 8);
        let mut computed = 0;
        let mut compute = |_module: usize| computed += 1;

        for (name, found) in [
            ("unselected", false),
            ("foo", false),
            ("foo", false),
            ("bar", false),
            ("unselected", true),
        ] {
            if !diagnostic_requested(Some(&filter), name, found) || !work.claim_name(name) {
                continue;
            }
            for module in 0..2 {
                if !work.claim_module() {
                    break;
                }
                compute(module);
            }
        }

        assert_eq!(computed, 4);
        assert_eq!(work.names.len(), 2);
    }

    #[test]
    fn module_work_budget_stops_actual_computation() {
        let mut work = TraceWork::with_limits(4, 3);
        let mut computed = 0;
        let mut compute = |_module: usize| computed += 1;

        for module in 0..8 {
            if !work.claim_module() {
                break;
            }
            compute(module);
        }

        assert_eq!(computed, 3);
        assert!(!work.claim_module());
    }

    #[test]
    fn name_and_module_limits_are_shared_and_hard_stops() {
        let mut work = TraceWork::with_limits(2, 2);
        assert!(work.claim_name("foo"));
        assert!(!work.claim_name("foo"));
        assert!(work.claim_name("bar"));
        assert!(!work.claim_name("baz"));
        assert!(work.claim_module());
        assert!(work.claim_module());
        assert!(!work.claim_module());
        assert!(!work.claim_name("qux"));
    }
}
