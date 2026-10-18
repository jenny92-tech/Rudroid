//! INPUT:  guest ELF bytes/files, common C contracts and optional Linux mapping services
//! OUTPUT:  ELF mapping/symbol APIs and a metadata-only import tuple visitor
//! POS:    shared guest metadata/relocation core behind dynamically loaded capability
//! rudroid-android-elf-load: minimal guest ELF map API (ticket 12).
//!
//! Usage Gate slice of the reference `loader/so_util.cpp`: map an AArch64
//! ELF shared object from a file path or in-memory buffer, apply relocations
//! and look symbols up by name — the minimum later tickets need to call
//! `JNI_OnLoad` and resolve guest hooks. Exec-mapped modules also offer the
//! reference `so_symbol_all` lookup (`rudroid_elf_exec_symbol_all`:
//! dynsym, then the unstripped file's `.symtab`). ARM32 encoding hooks and
//! section bookkeeping from the reference `so_util` are not here; inline
//! detours live in `rudroid_common::arm64_detour`.
//!
//! Two mapping models: the buffer load materializes the image into owned
//! writable memory on every host (testable off-device, never executable);
//! on Linux the exec backend (`exec.rs`) maps file-backed PT_LOAD segments
//! with W^X page permissions for real guest execution.

pub mod elf;
pub mod hash;
mod relocations;
mod packed;
mod imports;
pub use imports::rudroid_elf_visit_imports;

/// Linux executable mapping backend (ticket 49); absent on hosts without a
/// POSIX mmap pipeline — the buffer backend covers those.
#[cfg(target_os = "linux")]
pub mod exec;
#[cfg(target_os = "linux")]
mod libdl;
#[cfg(target_os = "linux")]
mod java;
#[cfg(target_os = "linux")]
pub use exec::{
    rudroid_elf_exec_destroy, rudroid_elf_exec_load, rudroid_elf_exec_load_base,
    rudroid_elf_exec_symbol, rudroid_elf_exec_symbol_all, rudroid_elf_symbol_source_register,
    rudroid_elf_symbol_source_register_versioned,
    rudroid_elf_symbol_source_register_finalized,
    rudroid_elf_symbol_source_unregister,
};
#[cfg(all(target_os = "linux", feature = "test-fixtures"))]
pub use exec::rudroid_elf_image_chain_reset;

/// Explicit test-build marker (test-fixtures feature only): the loader
/// refuses any kit member exporting it (`TestBuildMember`), independent of
/// which test-only entry points the build happens to carry.
#[cfg(feature = "test-fixtures")]
#[no_mangle]
pub extern "C" fn rudroid_test_build_marker() {}

use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::Mutex;

use rudroid_common::Kind;
use elf::{ElfError, ElfView};

rudroid_common::declare_descriptor!("android_elf_load", Kind::Runtime);

#[cfg(target_os = "linux")]
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api: Option<&rudroid_common::host_api::HostApiV0>) -> i32 {
    java::register(api)
}


/// A loaded guest module: the materialized image at a known base, the raw
/// ELF bytes the metadata views borrow, and the parsed layout.
///
/// SAFETY (self-reference): `view` borrows `raw`. `raw` is never resized or
/// mutated after parse, the Vec's heap buffer is stable across moves, and
/// `view` is declared first and drops before its backing buffers.
pub struct GuestModule {
    view: ElfView<'static>,
    // Borrowed by `view` (SAFETY note below); never resized or mutated, but
    // the compiler cannot see the transmuted borrow, hence the allow.
    #[allow(dead_code)]
    raw: Vec<u8>,
    /// Accessed through its pointer during relocation; kept alive by the
    /// struct (dropped last).
    #[allow(dead_code)]
    image: Vec<u8>,
    base: usize,
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// An OS mapping primitive failed (executable backend, ticket 49).
    Mapping(String),
    Elf(ElfError),
    Relocation { r_offset: u64, kind: u32, reason: String },
    MissingSymbol { symbol: String },
    Unsupported(&'static str),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "cannot read guest module: {e}"),
            LoadError::Mapping(reason) => {
                write!(f, "cannot map guest module: {reason}")
            }
            LoadError::Elf(e) => write!(f, "invalid guest module: {e}"),
            LoadError::Relocation { r_offset, kind, reason } => write!(
                f,
                "guest module relocation at vaddr {r_offset:#x} (type {kind}) failed: {reason}"
            ),
            LoadError::MissingSymbol { symbol } => write!(
                f,
                "guest module requires symbol `{symbol}` which no loaded module provides; \
                 load its provider module first"
            ),
            LoadError::Unsupported(what) => {
                write!(f, "guest module load unsupported here: {what}")
            }
        }
    }
}

impl std::error::Error for LoadError {}

impl From<ElfError> for LoadError {
    fn from(e: ElfError) -> Self {
        LoadError::Elf(e)
    }
}

/// Symbol provider consulted for undefined `R_AARCH64_ABS64`/`GLOB_DAT`
/// relocations (inter-module imports). Weak unresolved symbols bind to 0.
pub type GlobalSymbols<'a> = &'a HashMap<String, usize>;

/// Cross-module export lookup shared by the buffer and executable backends
/// (ticket 62). A dynamic symbol may satisfy another module's import only
/// when it is truly exported:
///
/// - `st_shndx != SHN_UNDEF` — an undefined entry (weak or strong) resolves
///   to nothing, never to `base + st_value`;
/// - binding is not `STB_LOCAL` (0) — locals are invisible across modules;
/// - visibility is not `STV_INTERNAL` (1) or `STV_HIDDEN` (2) — only
///   `STV_DEFAULT` (0) and `STV_PROTECTED` (3) are exported.
///
/// Returns the symbol's `st_value` (module-relative), before any base is
/// added; `Err` when the name IS an export but of a class that has no plain
/// address (STT_TLS, ticket 86).
pub(crate) fn exported_symbol_value(
    view: &ElfView,
    name: &str,
) -> Result<Option<u64>, ExportReject> {
    let name_of = |index: usize| -> Result<&str, ElfError> {
        view.dynsym(index).map(|(name, _, _)| name)
    };
    // Tables live inside LOAD segments; slice from the table start to the
    // end of its segment — walkers never read past their own structures.
    let raw_from = |table_vaddr: u64| -> Option<&[u8]> {
        let load = view
            .loads
            .iter()
            .find(|l| table_vaddr >= l.vaddr && table_vaddr < l.vaddr + l.filesz)?;
        let start = load.offset as usize + (table_vaddr - load.vaddr) as usize;
        view.bytes
            .get(start..load.offset as usize + load.filesz as usize)
    };

    if let Some(table_vaddr) = view.gnu_hash {
        if let Some(raw) = raw_from(table_vaddr) {
            if let Ok(table) = hash::GnuTable::parse(raw) {
                if let Ok(Some(index)) = table.lookup(name, &name_of) {
                    // Bloom/chain answered: an export of this module or not.
                    return defined_export(view, index);
                }
                return Ok(None); // bloom/chain answered: no such symbol
            }
        }
    }
    if let Some(table_vaddr) = view.sysv_hash {
        if let Some(raw) = raw_from(table_vaddr) {
            if let Ok(table) = hash::SysvTable::parse(raw) {
                if let Ok(Some(index)) = table.lookup(name, &name_of) {
                    return defined_export(view, index);
                }
            }
        }
    }
    Ok(None)
}

/// A dynamic symbol that is exported under the requested name but cannot be
/// handed out as an address (ticket 86: function/data/TLS are distinguished,
/// unsupported classes are rejected precisely).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportReject {
    /// STT_TLS: `st_value` is an offset into the module's TLS block.
    Tls,
}

impl ExportReject {
    /// The refusal reason for symbol `name`, for the C error channel.
    pub fn describe(self, name: &str) -> String {
        match self {
            ExportReject::Tls => format!(
                "symbol `{name}` is a TLS (STT_TLS) symbol — its value is a TLS-block \
                 offset, not an address; TLS lookup is not supported (ticket 86)"
            ),
        }
    }
}

/// Export eligibility of dynsym entry `index` (ticket 62); `Ok(None)` when
/// the entry is undefined, local, or hidden/internal, `Err` for an eligible
/// export without a plain address (STT_TLS, ticket 86).
fn defined_export(view: &ElfView, index: usize) -> Result<Option<u64>, ExportReject> {
    const SHN_UNDEF: u16 = 0;
    const STB_LOCAL: u8 = 0;
    const STV_INTERNAL: u8 = 1;
    const STV_HIDDEN: u8 = 2;
    let Ok((_, st_value, st_shndx)) = view.dynsym(index) else {
        return Ok(None);
    };
    if st_shndx == SHN_UNDEF {
        return Ok(None);
    }
    // st_info lives at entry offset +4, st_other at +5 (translated symtab).
    let Ok(offset) = view.symtab_entry_offset(index) else {
        return Ok(None);
    };
    let (Some(&info), Some(&other)) = (view.bytes.get(offset + 4), view.bytes.get(offset + 5))
    else {
        return Ok(None);
    };
    if info >> 4 == STB_LOCAL {
        return Ok(None);
    }
    let visibility = other & 0x3;
    if visibility == STV_INTERNAL || visibility == STV_HIDDEN {
        return Ok(None);
    }
    if info & 0xf == relocations::STT_TLS {
        return Err(ExportReject::Tls);
    }
    Ok(Some(st_value))
}

impl GuestModule {
    /// Load from an in-memory ELF image.
    pub fn from_bytes(
        bytes: Vec<u8>,
        globals: GlobalSymbols<'_>,
    ) -> Result<GuestModule, LoadError> {
        GuestModule::load(bytes, &|name| globals.get(name).copied())
    }

    /// Constructor with an arbitrary import resolver: the C API passes the
    /// load-order registry so a module's undefined imports bind against the
    /// exports of previously loaded modules (ticket 48).
    fn load(
        bytes: Vec<u8>,
        source: &dyn Fn(&str) -> Option<usize>,
    ) -> Result<GuestModule, LoadError> {
        let view = ElfView::parse(&bytes)?;
        let span = view.image_span()? as usize;
        let mut image = vec![0u8; span];
        // Copy the file-backed part of every PT_LOAD; BSS stays zero. The
        // ranges were validated at parse time; the get_mut here is the
        // defensive belt so a future regression surfaces as a diagnostic,
        // never as an indexing panic (ticket 47).
        for load in &view.loads {
            if load.filesz > 0 {
                let src = view.vaddr_slice(load.vaddr, load.filesz)?;
                let start = load.vaddr as usize;
                let target = image
                    .get_mut(start..start + src.len())
                    .ok_or(LoadError::Elf(ElfError::TruncatedSection("PT_LOAD copy")))?;
                target.copy_from_slice(src);
            }
        }
        // SAFETY: see the struct docs — `view` keeps borrowing `raw`.
        let view: ElfView<'static> =
            unsafe { std::mem::transmute::<ElfView<'_>, ElfView<'static>>(view) };
        let base = image.as_ptr() as usize;
        // Buffer backend: undefined imports resolve through `source` only
        // (the registered host/thunk layer belongs to the exec path, 86).
        // SAFETY: initialized, exclusively owned image Vec; its length covers the reservation.
        unsafe{relocations::apply(&view, base, image.as_mut_ptr(), span, &|name, version| {
            if version.is_some() { None } else { source(name) }
        })?;}
        // SAFETY: see the struct docs — `view` keeps borrowing `raw`.
        let view: ElfView<'static> =
            unsafe { std::mem::transmute::<ElfView<'_>, ElfView<'static>>(view) };
        Ok(GuestModule {
            raw: bytes,
            base,
            image,
            view,
        })
    }

    /// Load from a file path (read into memory; see the crate docs for the
    /// file-backed mmap follow-up).
    pub fn from_path(
        path: impl AsRef<std::path::Path>,
        globals: GlobalSymbols<'_>,
    ) -> Result<GuestModule, LoadError> {
        let bytes = std::fs::read(path).map_err(LoadError::Io)?;
        GuestModule::from_bytes(bytes, globals)
    }

    /// Look a dynamic symbol up by name (GNU hash first, SYSV fallback).
    /// Returns the absolute host address or `None` (TLS exports included —
    /// they have no address).
    pub fn symbol(&self, name: &str) -> Option<usize> {
        self.symbol_lookup(name).ok().flatten()
    }

    /// [`Self::symbol`] keeping the reason a present export is refused.
    fn symbol_lookup(&self, name: &str) -> Result<Option<usize>, ExportReject> {
        // Export eligibility lives in one place shared with the executable
        // backend (ticket 62).
        exported_symbol_value(&self.view, name)
            .map(|value| value.map(|value| self.base.wrapping_add(value as usize)))
    }

    /// Base address the image was materialized at.
    pub fn base(&self) -> usize {
        self.base
    }
}

// ---------------------------------------------------------------------------
// C error channel (ticket 65): load failures keep their specific reason
// across the C ABI.

thread_local! {
    /// The calling thread's most recent C-API failure. Cleared by every
    /// entry point on entry, set on failure; a UTF-8 NUL-terminated message.
    static LAST_ERROR: std::cell::RefCell<Option<std::ffi::CString>> =
        const { std::cell::RefCell::new(None) };
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

fn set_last_error(message: &std::fmt::Arguments<'_>) {
    // Messages contain no interior NUL (paths came from CStrings, Display
    // strings carry none), so format! then CString::new cannot fail here.
    let stored = std::ffi::CString::new(format!("{message}")).unwrap_or_default();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(stored));
}

/// Copy the calling thread's last C-API error into `buffer`.
///
/// Protocol (ticket 65): every C entry point clears this state on entry and
/// sets it on failure, so the message is valid until the NEXT C-API call on
/// the same thread. The return value is the full length INCLUDING the NUL
/// terminator; `0` means no error is recorded. When `buffer` is non-null and
/// `capacity > 0`, at most `capacity - 1` message bytes plus a NUL are
/// written (truncation never overruns), so a null/zero query followed by an
/// exact-size fetch always round-trips the whole message. Concurrent failing
/// calls on different threads never observe each other's message.
///
/// # Safety
/// `buffer` must be null or point to `capacity` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_last_error(buffer: *mut c_char, capacity: usize) -> usize {
    let message = LAST_ERROR
        .with(|slot| slot.borrow().as_ref().map(|c| c.as_bytes_with_nul().to_vec()));
    let Some(bytes) = message else {
        return 0;
    };
    let needed = bytes.len();
    if !buffer.is_null() && capacity > 0 {
        let copy_len = needed.saturating_sub(1).min(capacity - 1);
        // SAFETY: caller promised `capacity` writable bytes; copy_len < capacity.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buffer, copy_len);
            *buffer.add(copy_len) = 0;
        }
    }
    needed
}

// ---------------------------------------------------------------------------
// C exports consumed by the loader/jni_vm (symbols documented in
// rudroid_common::symbols::elf_load).

/// Opaque module handle.
pub struct RudroidGuestModule {
    module: GuestModule,
}

/// Modules loaded through the C API, in load order (ticket 48): a later
/// module's undefined imports resolve against the exported symbols of every
/// previously loaded module. Handles stay pinned for the process life (the
/// same ownership contract as the loader kit); `destroy` deregisters before
/// dropping so the registry can never dangle.
static C_LOADED_MODULES: Mutex<Vec<CModulePtr>> = Mutex::new(Vec::new());

/// Raw handle wrapper: registry entries point at modules whose Box handles
/// are owned by C callers. The pinning contract (process-life handles) and
/// deregistration in `destroy` keep every stored pointer valid while it is
/// in the list.
struct CModulePtr(*const GuestModule);
// SAFETY: see the type docs; the pointee is only accessed under the
// registry lock and lives until deregistered.
unsafe impl Send for CModulePtr {}
unsafe impl Sync for CModulePtr {}

fn c_registry_lookup(name: &str) -> Option<usize> {
    let modules = C_LOADED_MODULES.lock().unwrap();
    for entry in modules.iter() {
        // SAFETY: registered by the C load path; valid until deregistered by
        // `destroy`, and the lock protects the walk.
        let module: &GuestModule = unsafe { &*entry.0 };
        if let Some(address) = module.symbol(name) {
            return Some(address);
        }
    }
    None
}

fn c_register(module: &GuestModule) {
    C_LOADED_MODULES.lock().unwrap().push(CModulePtr(module as *const GuestModule));
}

fn c_deregister(module: &GuestModule) {
    let mut modules = C_LOADED_MODULES.lock().unwrap();
    modules.retain(|candidate| candidate.0 != module as *const GuestModule);
}

/// The executable backend's mapped module (ticket 49/86): re-exported so
/// integration tests can name the handle type of the exec C exports.
#[cfg(target_os = "linux")]
pub use exec::ExecutableModule;

/// Ticket 84 instrumentation: backend load-call counts. Tests snapshot the
/// deltas across a guest bring-up to prove the production path performs
/// exactly ONE load per module (data+exec double loading is gone). Plain
/// atomics — test/diagnostic surface, never read by production logic.
pub static BUFFER_LOAD_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static EXEC_LOAD_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// # Safety
/// `path` must be NUL-terminated; returns null on failure (the reason is
/// available through `rudroid_elf_last_error`, ticket 65).
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_load_from_path(
    path: *const c_char,
) -> *mut RudroidGuestModule {
    clear_last_error();
    BUFFER_LOAD_CALLS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let Some(path) = (|| {
        if path.is_null() {
            return None;
        }
        // SAFETY: non-null `path` is a NUL-terminated string (caller contract).
        unsafe { CStr::from_ptr(path) }.to_str().ok().map(str::to_owned)
    })() else {
        set_last_error(&format_args!(
            "rudroid_elf_load_from_path: path is null or not valid UTF-8"
        ));
        return std::ptr::null_mut();
    };
    // Imports may come from previously loaded modules, in load order (ticket 48).
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            set_last_error(&format_args!("cannot load guest module {path}: {error}"));
            return std::ptr::null_mut();
        }
    };
    match GuestModule::load(bytes, &c_registry_lookup) {
        Ok(module) => {
            let registered = Box::into_raw(Box::new(RudroidGuestModule { module }));
            // SAFETY: freshly boxed; freed only by the matching unload.
            c_register(unsafe { &(*registered).module });
            registered
        }
        Err(error) => {
            set_last_error(&format_args!("cannot load guest module {path}: {error}"));
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `bytes` must point to `len` readable ELF bytes; null/invalid arguments
/// are diagnosable through `rudroid_elf_last_error` (ticket 65).
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_load_from_bytes(
    bytes: *const u8,
    len: usize,
) -> *mut RudroidGuestModule {
    clear_last_error();
    if bytes.is_null() {
        set_last_error(&format_args!(
            "rudroid_elf_load_from_bytes: bytes pointer is null"
        ));
        return std::ptr::null_mut();
    }
    // SAFETY: non-null `bytes` points to `len` readable bytes (caller contract).
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();
    match GuestModule::load(slice, &c_registry_lookup) {
        Ok(module) => {
            let registered = Box::into_raw(Box::new(RudroidGuestModule { module }));
            // SAFETY: freshly boxed; freed only by the matching unload.
            c_register(unsafe { &(*registered).module });
            registered
        }
        Err(error) => {
            set_last_error(&format_args!("cannot load guest module from bytes: {error}"));
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `handle` must come from the load functions; null arguments are diagnosed
/// through `rudroid_elf_last_error` (ticket 65). A missing symbol is a
/// documented miss (0), not an error.
/// Ticket 84 instrumentation: report the backend load-call counts (buffer
/// views vs exec mappings). Test/diagnostic surface — the loader never
/// branches on these.
///
/// # Safety
/// Either pointer may be null (that count is skipped); both must be
/// writable when non-null.
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_load_stats(
    data_loads: *mut usize,
    exec_loads: *mut usize,
) {
    use std::sync::atomic::Ordering;
    if !data_loads.is_null() {
        // SAFETY: non-null caller-provided out pointer.
        unsafe { data_loads.write(BUFFER_LOAD_CALLS.load(Ordering::Acquire)) };
    }
    if !exec_loads.is_null() {
        // SAFETY: non-null caller-provided out pointer.
        unsafe { exec_loads.write(EXEC_LOAD_CALLS.load(Ordering::Acquire)) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_symbol(
    handle: *const RudroidGuestModule,
    name: *const c_char,
) -> usize {
    clear_last_error();
    let Some(handle) = handle.as_ref() else {
        set_last_error(&format_args!("rudroid_elf_symbol: handle is null"));
        return 0;
    };
    if name.is_null() {
        set_last_error(&format_args!("rudroid_elf_symbol: name is null"));
        return 0;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        set_last_error(&format_args!("rudroid_elf_symbol: name is not valid UTF-8"));
        return 0;
    };
    match handle.module.symbol_lookup(name) {
        Ok(address) => address.unwrap_or(0),
        Err(reject) => {
            set_last_error(&format_args!("rudroid_elf_symbol: {}", reject.describe(name)));
            0
        }
    }
}

/// # Safety
/// `handle` must come from the load functions and not be freed twice; null is
/// a no-op (diagnosable through `rudroid_elf_last_error`, ticket 65).
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_destroy(handle: *mut RudroidGuestModule) {
    clear_last_error();
    if !handle.is_null() {
        // SAFETY: the handle came from Box::into_raw and is not freed twice.
        let boxed = unsafe { Box::from_raw(handle) };
        c_deregister(&boxed.module);
        drop(boxed);
    } else {
        set_last_error(&format_args!("rudroid_elf_destroy: handle is null"));
    }
}

/// # Safety
/// `handle` must come from the load functions; null yields 0 with a
/// diagnostic on the error channel (ticket 65).
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_load_base(handle: *const RudroidGuestModule) -> usize {
    clear_last_error();
    match handle.as_ref() {
        Some(handle) => handle.module.base(),
        None => {
            set_last_error(&format_args!("rudroid_elf_load_base: handle is null"));
            0
        }
    }
}

/// Synthetic test fixture: a minimal but fully valid AArch64 shared object
/// (GNU hash, one defined symbol, one RELATIVE + one weak ABS64 relocation).
/// Only compiled for cross-crate tests; never ships in the kit surface.
#[cfg(feature = "test-fixtures")]
pub mod fixture {
    use super::elf::{EM_AARCH64, PT_DYNAMIC, PT_LOAD};
    use super::hash;

    const EHDR: usize = 0;
    const PHDR: usize = 64; // 2 * 56 bytes
    const DYNAMIC: usize = 176; // 10 entries * 16
    const DYNSTR: usize = 336;
    const DYNSYM: usize = 384; // 3 entries * 24
    const GNU_HASH: usize = 456;
    const DATA: usize = 512; // answer slot + import slot
    const RELA: usize = 528; // 2 entries * 24
    const IMAGE_SIZE: usize = 640;

    pub const ANSWER_SYMBOL: &str = "rudroid_answer";

    fn str_table() -> Vec<u8> {
        // offset 0: "", 1: "rudroid_answer", 16: "rudroid_import"
        let mut table = Vec::new();
        table.push(0);
        let first = table.len() as u32;
        table.extend_from_slice(b"rudroid_answer\0");
        let second = table.len() as u32;
        table.extend_from_slice(b"rudroid_import\0");
        assert_eq!(first, 1);
        assert_eq!(second, 16);
        table
    }

    /// Build the fixture image: one RW LOAD segment, PT_DYNAMIC, GNU hash,
    /// two dynsyms and two RELA entries (RELATIVE +0x100, weak ABS64).
    pub fn build_minimal_aarch64_so() -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_SIZE];

        image[EHDR..EHDR + 4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        image[4] = 2; // ELFCLASS64
        image[5] = 1; // little-endian
        image[6] = 1; // EV_CURRENT
        image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        image[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        image[32..40].copy_from_slice(&(PHDR as u64).to_le_bytes()); // e_phoff
        image[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        image[56..58].copy_from_slice(&2u16.to_le_bytes()); // e_phnum

        let mut ph = |base: usize, p_type: u32, vaddr: u64, filesz: u64| {
            image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
            image[base + 8..base + 16].copy_from_slice(&0u64.to_le_bytes()); // offset == vaddr
            image[base + 16..base + 24].copy_from_slice(&vaddr.to_le_bytes());
            image[base + 32..base + 40].copy_from_slice(&filesz.to_le_bytes());
            image[base + 40..base + 48].copy_from_slice(&filesz.to_le_bytes());
        };
        ph(PHDR, PT_LOAD, 0, IMAGE_SIZE as u64);
        ph(PHDR + 56, PT_DYNAMIC, DYNAMIC as u64, 10 * 16);

        let dyn_entries: Vec<(u64, u64)> = vec![
            (6, DYNSYM as u64),            // DT_SYMTAB
            (5, DYNSTR as u64),            // DT_STRTAB
            (10, 31),                      // DT_STRSZ
            (0x6ffffef5, GNU_HASH as u64), // DT_GNU_HASH
            (7, RELA as u64),              // DT_RELA
            (8, 48),                       // DT_RELASZ
            (11, 24),                      // DT_SYMENT
            (0, 0),                        // DT_NULL
        ];
        for (index, (tag, value)) in dyn_entries.iter().enumerate() {
            let at = DYNAMIC + index * 16;
            image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
            image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
        }

        let strtab = str_table();
        image[DYNSTR..DYNSTR + strtab.len()].copy_from_slice(&strtab);

        let mut write_sym = |at: usize, name: u32, info: u8, shndx: u16, value: u64| {
            image[at..at + 4].copy_from_slice(&name.to_le_bytes());
            image[at + 4] = info;
            image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
            image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
        };
        write_sym(DYNSYM, 0, 0, 0, 0);
        write_sym(DYNSYM + 24, 1, 0x12, 1, DATA as u64); // GLOBAL, defined
        write_sym(DYNSYM + 48, 16, 0x22, 0, 0); // WEAK, undefined

        let h_answer = hash::gnu_hash(ANSWER_SYMBOL);
        image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 12..GNU_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
        image[GNU_HASH + 16..GNU_HASH + 24]
            .copy_from_slice(&(1u64 << (h_answer % 64)).to_le_bytes());
        image[GNU_HASH + 24..GNU_HASH + 28].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 28..GNU_HASH + 32]
            .copy_from_slice(&((h_answer | 1) as u32).to_le_bytes());

        image[RELA..RELA + 8].copy_from_slice(&((DATA + 8) as u64).to_le_bytes());
        image[RELA + 8..RELA + 16]
            .copy_from_slice(&((0u64 << 32) | 1027).to_le_bytes()); // RELATIVE
        image[RELA + 16..RELA + 24].copy_from_slice(&0x100i64.to_le_bytes());

        image[RELA + 24..RELA + 32].copy_from_slice(&((DATA + 16) as u64).to_le_bytes());
        image[RELA + 32..RELA + 40]
            .copy_from_slice(&((2u64 << 32) | 257).to_le_bytes()); // ABS64
        image[RELA + 40..RELA + 48].copy_from_slice(&0i64.to_le_bytes());

        image
    }

    /// Ticket 85 rework failure-injection fixture: the minimal image with
    /// `rudroid_import` flipped from WEAK-undefined (resolves to 0) to
    /// GLOBAL-undefined. Relocation then fails with a structured
    /// `MissingSymbol` AFTER both mappings exist — the probe for the
    /// single-owner cleanup path (`exec.rs`): no metadata or region mapping
    /// may survive a failed load, and nothing may be unmapped twice.
    pub fn build_missing_symbol_aarch64_so() -> Vec<u8> {
        let mut image = build_minimal_aarch64_so();
        // ST_BIND(Global)=1 << 4 | STT(Object)=2 → 0x12 (the defined
        // `rudroid_answer` uses the same binding); shndx stays 0 (undefined).
        image[DYNSYM + 48 + 4] = 0x12;
        image
    }

    /// Ticket 85 rework rejection fixture: two PT_LOADs sharing ONE page —
    /// executable [0x0,0x400) and writable [0x400,0x440) both land in page
    /// 0 (4096 bytes). Real linker output separates segments page-apart;
    /// this layout cannot honor W^X without either RWX or losing +x on the
    /// code half, so `exec.rs` must reject it structurally instead of
    /// reporting a load whose code then faults.
    pub fn build_overlapping_wx_aarch64_so() -> Vec<u8> {
        // Body layout with room for 3 program headers (64 + 3*56 = 232):
        const PHDR: usize = 64;
        const DYNAMIC: usize = 232; // 10 entries * 16 → ends 392
        const DYNSTR: usize = 392;
        const DYNSYM: usize = 424; // 3 * 24 → ends 496
        const GNU_HASH: usize = 496;
        const DATA: usize = 528;
        const RELA: usize = 560; // 2 * 24 → ends 608
        const LOAD2: usize = 0x400; // writable segment start, same page
        const IMAGE_SIZE: usize = 0x440;
        const PF_R: u32 = 4;
        const PF_W: u32 = 2;
        const PF_X: u32 = 1;

        let mut image = vec![0u8; IMAGE_SIZE];
        image[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        image[4] = 2; // ELFCLASS64
        image[5] = 1; // little-endian
        image[6] = 1; // EV_CURRENT
        image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        image[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        image[32..40].copy_from_slice(&(PHDR as u64).to_le_bytes()); // e_phoff
        image[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        image[56..58].copy_from_slice(&3u16.to_le_bytes()); // e_phnum

        let mut ph = |base: usize, p_type: u32, flags: u32, off: u64, vaddr: u64, filesz: u64| {
            image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
            image[base + 4..base + 8].copy_from_slice(&flags.to_le_bytes());
            image[base + 8..base + 16].copy_from_slice(&off.to_le_bytes());
            image[base + 16..base + 24].copy_from_slice(&vaddr.to_le_bytes());
            image[base + 32..base + 40].copy_from_slice(&filesz.to_le_bytes());
            image[base + 40..base + 48].copy_from_slice(&filesz.to_le_bytes());
        };
        // R|X covers page 0's first 1024 bytes; R|W starts at 1024 — same
        // 4096-byte page, so the two final protections cannot both hold.
        ph(PHDR, PT_LOAD, PF_R | PF_X, 0, 0, LOAD2 as u64);
        ph(
            PHDR + 56,
            PT_LOAD,
            PF_R | PF_W,
            LOAD2 as u64,
            LOAD2 as u64,
            (IMAGE_SIZE - LOAD2) as u64,
        );
        ph(PHDR + 112, PT_DYNAMIC, PF_R, DYNAMIC as u64, DYNAMIC as u64, 10 * 16);

        let dyn_entries: Vec<(u64, u64)> = vec![
            (6, DYNSYM as u64),            // DT_SYMTAB
            (5, DYNSTR as u64),            // DT_STRTAB
            (10, 31),                      // DT_STRSZ
            (0x6ffffef5, GNU_HASH as u64), // DT_GNU_HASH
            (7, RELA as u64),              // DT_RELA
            (8, 48),                       // DT_RELASZ
            (11, 24),                      // DT_SYMENT
            (0, 0),                        // DT_NULL
        ];
        for (index, (tag, value)) in dyn_entries.iter().enumerate() {
            let at = DYNAMIC + index * 16;
            image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
            image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
        }

        let strtab = str_table();
        image[DYNSTR..DYNSTR + strtab.len()].copy_from_slice(&strtab);

        let mut write_sym = |at: usize, name: u32, info: u8, shndx: u16, value: u64| {
            image[at..at + 4].copy_from_slice(&name.to_le_bytes());
            image[at + 4] = info;
            image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
            image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
        };
        write_sym(DYNSYM, 0, 0, 0, 0);
        write_sym(DYNSYM + 24, 1, 0x12, 1, DATA as u64); // GLOBAL, defined
        write_sym(DYNSYM + 48, 16, 0x22, 0, 0); // WEAK, undefined

        let h_answer = hash::gnu_hash(ANSWER_SYMBOL);
        image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 12..GNU_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
        image[GNU_HASH + 16..GNU_HASH + 24]
            .copy_from_slice(&(1u64 << (h_answer % 64)).to_le_bytes());
        image[GNU_HASH + 24..GNU_HASH + 28].copy_from_slice(&1u32.to_le_bytes());
        image[GNU_HASH + 28..GNU_HASH + 32]
            .copy_from_slice(&((h_answer | 1) as u32).to_le_bytes());

        image[RELA..RELA + 8].copy_from_slice(&((DATA + 8) as u64).to_le_bytes());
        image[RELA + 8..RELA + 16]
            .copy_from_slice(&((0u64 << 32) | 1027).to_le_bytes()); // RELATIVE
        image[RELA + 16..RELA + 24].copy_from_slice(&0x100i64.to_le_bytes());
        image[RELA + 24..RELA + 32].copy_from_slice(&((DATA + 16) as u64).to_le_bytes());
        image[RELA + 32..RELA + 40]
            .copy_from_slice(&((2u64 << 32) | 257).to_le_bytes()); // ABS64 (weak → 0)
        image[RELA + 40..RELA + 48].copy_from_slice(&0i64.to_le_bytes());

        // Distinguishable bytes in the writable segment (the load must fail
        // before any of this is mapped).
        image[LOAD2..LOAD2 + 4].copy_from_slice(&0xB16D_B16Du32.to_le_bytes());
        image
    }

    /// Modes for [`build_versioned_aarch64_so`] (ticket 86).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum VersionMode {
        Strong,
        Weak,
        Baseline,
    }

    /// Ticket 86 rework: the minimal image extended with GNU symbol
    /// versioning — DT_VERSYM/DT_VERNEED/DT_VERNEEDNUM, version string
    /// `TESTVER_1.0`, requiring file `libc.so.6`, and `rudroid_import`
    /// carrying version index 2. `mode` selects the negative cases:
    /// - `Strong`: versioned strong undefined import → the load must be
    ///   REFUSED with a structured diagnostic (never bound by bare name).
    /// - `Weak`: versioned weak undefined import → binds 0, load succeeds.
    /// - `Baseline`: VERSYM index 1 (`*global*`, no requirement) → normal
    ///   resolution path, unaffected by the version gate.
    pub fn build_versioned_aarch64_so(mode: VersionMode) -> Vec<u8> {
        let mut image = build_minimal_aarch64_so();
        const VERSTR: usize = 576; // "TESTVER_1.0\0" (12) at file vaddr
        const FILESTR: usize = 588; // "libc.so.6\0" (10) at file vaddr
        // VERNEED/VERNAUX name fields are DYNSTR OFFSETS (relative to
        // DT_STRTAB at 336), not absolute vaddrs — mirrors dynsym st_name.
        const VERSTR_OFF: u32 = (VERSTR - DYNSTR) as u32; // 240
        const FILESTR_OFF: u32 = (FILESTR - DYNSTR) as u32; // 252
        const VERSYM: usize = 598; // 3 * u16
        const VERNEED: usize = 604; // Verneed (16)
        const VERNAUX: usize = 620; // Vernaux (16)
        assert_eq!(image.len(), 640);
        image[VERSTR..VERSTR + 12].copy_from_slice(b"TESTVER_1.0\0");
        image[FILESTR..FILESTR + 10].copy_from_slice(b"libc.so.6\0");
        // VERSYM per dynsym: [0]=LOCAL, [1]=rudroid_answer (global baseline),
        // [2]=rudroid_import (version index 2, or baseline in Baseline mode).
        let import_versym: u16 = match mode {
            VersionMode::Baseline => 1,
            _ => 2,
        };
        image[VERSYM..VERSYM + 2].copy_from_slice(&0u16.to_le_bytes());
        image[VERSYM + 2..VERSYM + 4].copy_from_slice(&1u16.to_le_bytes());
        image[VERSYM + 4..VERSYM + 6].copy_from_slice(&import_versym.to_le_bytes());
        // Verneed: version=1, cnt=1, file=FILESTR, aux=16, next=0.
        image[VERNEED..VERNEED + 2].copy_from_slice(&1u16.to_le_bytes());
        image[VERNEED + 2..VERNEED + 4].copy_from_slice(&1u16.to_le_bytes());
        image[VERNEED + 4..VERNEED + 8].copy_from_slice(&FILESTR_OFF.to_le_bytes());
        image[VERNEED + 8..VERNEED + 12].copy_from_slice(&16u32.to_le_bytes());
        image[VERNEED + 12..VERNEED + 16].copy_from_slice(&0u32.to_le_bytes());
        // Vernaux: hash, flags=0, other=2, name=VERSTR, next=0.
        let vhash = hash::gnu_hash("TESTVER_1.0");
        image[VERNAUX..VERNAUX + 4].copy_from_slice(&vhash.to_le_bytes());
        image[VERNAUX + 4..VERNAUX + 6].copy_from_slice(&0u16.to_le_bytes());
        image[VERNAUX + 6..VERNAUX + 8].copy_from_slice(&2u16.to_le_bytes());
        image[VERNAUX + 8..VERNAUX + 12].copy_from_slice(&VERSTR_OFF.to_le_bytes());
        image[VERNAUX + 12..VERNAUX + 16].copy_from_slice(&0u32.to_le_bytes());
        // Dynamic table has 10 slots, 8 used (index 7 is DT_NULL): retarget
        // 7/8/9 at the version tables. STRSZ (slot 2) must cover the new
        // version strings: VERSTR+12 = 588 ≤ DYNSTR+STRSZ → STRSZ = 262.
        let dyn_at = |slot: usize| DYNAMIC + slot * 16;
        image[dyn_at(2) + 8..dyn_at(2) + 16].copy_from_slice(&262u64.to_le_bytes());
        image[dyn_at(7)..dyn_at(7) + 8].copy_from_slice(&super::elf::DT_VERSYM.to_le_bytes());
        image[dyn_at(7) + 8..dyn_at(7) + 16].copy_from_slice(&(VERSYM as u64).to_le_bytes());
        image[dyn_at(8)..dyn_at(8) + 8].copy_from_slice(&super::elf::DT_VERNEED.to_le_bytes());
        image[dyn_at(8) + 8..dyn_at(8) + 16].copy_from_slice(&(VERNEED as u64).to_le_bytes());
        image[dyn_at(9)..dyn_at(9) + 8].copy_from_slice(&super::elf::DT_VERNEEDNUM.to_le_bytes());
        image[dyn_at(9) + 8..dyn_at(9) + 16].copy_from_slice(&1u64.to_le_bytes());
        // The stock import is WEAK-undefined (0x22); Strong mode flips it
        // to GLOBAL-undefined (0x12) so the gate's refusal path runs.
        if matches!(mode, VersionMode::Strong) {
            image[DYNSYM + 48 + 4] = 0x12;
        }
        image
    }
}
