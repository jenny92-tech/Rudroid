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
mod imports;
#[cfg(target_os = "linux")]
mod live_symbols;
mod packed;
mod relocations;
pub use imports::rudroid_elf_visit_imports;

/// Linux executable mapping backend (ticket 49); absent on hosts without a
/// POSIX mmap pipeline — the buffer backend covers those.
#[cfg(target_os = "linux")]
pub mod exec;
#[cfg(target_os = "linux")]
mod java;
#[cfg(target_os = "linux")]
mod libdl;
#[cfg(all(target_os = "linux", feature = "test-fixtures"))]
pub use exec::rudroid_elf_image_chain_reset;
#[cfg(target_os = "linux")]
pub use exec::{
    rudroid_elf_exec_destroy, rudroid_elf_exec_load, rudroid_elf_exec_load_base,
    rudroid_elf_exec_symbol, rudroid_elf_exec_symbol_all, rudroid_elf_symbol_source_register,
    rudroid_elf_symbol_source_register_finalized, rudroid_elf_symbol_source_register_versioned,
    rudroid_elf_symbol_source_unregister,
};

/// Explicit test-build marker (test-fixtures feature only): the loader
/// refuses any kit member exporting it (`TestBuildMember`), independent of
/// which test-only entry points the build happens to carry.
#[cfg(feature = "test-fixtures")]
#[no_mangle]
pub extern "C" fn rudroid_test_build_marker() {}

use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::Mutex;

use elf::{ElfError, ElfView};
use rudroid_common::Kind;

rudroid_common::declare_descriptor!("android_elf_load", Kind::Runtime);

#[cfg(target_os = "linux")]
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(
    api: Option<&rudroid_common::host_api::HostApiV0>,
) -> i32 {
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
    Relocation {
        r_offset: u64,
        kind: u32,
        reason: String,
    },
    MissingSymbol {
        symbol: String,
    },
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
            LoadError::Relocation {
                r_offset,
                kind,
                reason,
            } => write!(
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
/// - an unversioned query skips VERSYM-hidden definitions (`foo@V1`) and
///   continues to a visible default (`foo@@V2`); no VERSYM keeps bare lookup.
///
/// Returns the symbol's `st_value` (module-relative), before any base is
/// added; `Err` when the name IS an export but of a class that has no plain
/// address (STT_TLS, ticket 86).
pub(crate) fn exported_symbol_value(
    view: &ElfView,
    name: &str,
) -> Result<Option<u64>, ExportReject> {
    match exported_symbol_index(view, name).map_err(ExportReject::MalformedVersion)? {
        Some(index) => defined_export(view, index),
        None => Ok(None),
    }
}

/// The version-aware counterpart of [`exported_symbol_value`], following
/// Bionic's per-DSO `check_symbol_version` rule. A provider with no VERSYM
/// satisfies any requested version. Otherwise the provider's own VERDEF is
/// searched by name and SysV hash; a missing requested node means GLOBAL
/// index 1. The candidate's VERSYM index must equal that node, with the
/// hidden bit masked. There is deliberately no bare-name fallback here.
pub(crate) fn exported_symbol_value_versioned(
    view: &ElfView,
    name: &str,
    required: elf::SymbolVersion<'_>,
) -> Result<Option<u64>, ExportReject> {
    let indices = exported_symbol_indices(view, name).map_err(ExportReject::MalformedVersion)?;
    if indices.is_empty() {
        return Ok(None);
    }
    let requested = view
        .verdef_version_index(required)
        .map_err(ExportReject::MalformedVersion)?;
    // The same bare name may legally occur more than once (`foo@V1`, V2).
    // A SYSV chain also reaches undefined imports, so discard non-exports
    // before interpreting their definition-only version tables. For a real
    // export, version selection still precedes TLS rejection just as in
    // Bionic's hash-chain callback.
    for index in indices {
        let Some((st_value, st_type)) = export_candidate(view, index) else {
            continue;
        };
        if !view
            .symbol_matches_version(index, requested)
            .map_err(ExportReject::MalformedVersion)?
        {
            continue;
        }
        if st_type == relocations::STT_TLS {
            return Err(ExportReject::Tls);
        }
        return Ok(Some(st_value));
    }
    Ok(None)
}

/// First eligible non-hidden-version candidate in the name's hash chain.
/// Rejecting candidates through the existing borrowed-name callback keeps
/// ordinary lookup allocation-free. TLS is checked only after version
/// selection: a hidden TLS predecessor cannot hide the default address,
/// while a selected TLS export still fails without a bare-name retry.
fn exported_symbol_index(view: &ElfView, name: &str) -> Result<Option<usize>, ElfError> {
    if name.is_empty() {
        return Ok(None);
    }
    let name_of = |index: usize| -> Result<&str, ElfError> {
        let (candidate, _, _) = view.dynsym(index)?;
        if candidate == name
            && (export_candidate(view, index).is_none()
                || view.versym(index)?.is_some_and(|version| version & 0x8000 != 0))
        {
            // `name` is nonempty, so the hash walker continues its chain.
            return Ok("");
        }
        Ok(candidate)
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
                // A valid GNU table remains authoritative. In particular,
                // malformed VERSYM must not turn into a SYSV/live fallback.
                return table.lookup(name, &name_of);
            }
        }
    }
    if let Some(table_vaddr) = view.sysv_hash {
        if let Some(raw) = raw_from(table_vaddr) {
            if let Ok(table) = hash::SysvTable::parse(raw) {
                return table.lookup(name, &name_of);
            }
        }
    }
    Ok(None)
}

/// All dynsym indices reachable under `name` in the authoritative hash table.
/// Versioned resolution uses this to look past an older/wrong-version symbol
/// that happens to be the first same-name chain entry. Table damage is an
/// error, never an empty match set.
fn exported_symbol_indices(view: &ElfView, name: &str) -> Result<Vec<usize>, ElfError> {
    let name_of =
        |index: usize| -> Result<&str, ElfError> { view.dynsym(index).map(|(name, _, _)| name) };
    let raw_from = |table_vaddr: u64| -> Option<&[u8]> {
        let load = view
            .loads
            .iter()
            .find(|load| table_vaddr >= load.vaddr && table_vaddr < load.vaddr + load.filesz)?;
        let start = load.offset as usize + (table_vaddr - load.vaddr) as usize;
        view.bytes
            .get(start..load.offset as usize + load.filesz as usize)
    };

    if let Some(table_vaddr) = view.gnu_hash {
        let raw = raw_from(table_vaddr)
            .ok_or(ElfError::TruncatedSection("DT_GNU_HASH"))?;
        return hash::GnuTable::parse(raw)?.lookup_all(name, &name_of);
    }
    if let Some(table_vaddr) = view.sysv_hash {
        let raw = raw_from(table_vaddr).ok_or(ElfError::TruncatedSection("DT_HASH"))?;
        return hash::SysvTable::parse(raw)?.lookup_all(name, &name_of);
    }
    Ok(Vec::new())
}

/// A dynamic symbol that is exported under the requested name but cannot be
/// handed out as an address (ticket 86: function/data/TLS are distinguished,
/// unsupported classes are rejected precisely).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportReject {
    /// STT_TLS: `st_value` is an offset into the module's TLS block.
    Tls,
    /// A version table or hash chain needed for the lookup is malformed.
    MalformedVersion(ElfError),
}

impl ExportReject {
    /// The refusal reason for symbol `name`, for the C error channel.
    pub fn describe(&self, name: &str) -> String {
        match self {
            ExportReject::Tls => format!(
                "symbol `{name}` is a TLS (STT_TLS) symbol — its value is a TLS-block \
                 offset, not an address; TLS lookup is not supported (ticket 86)"
            ),
            ExportReject::MalformedVersion(error) => format!(
                "symbol `{name}` has malformed symbol-version tables: {error}"
            ),
        }
    }
}

/// Export eligibility of dynsym entry `index` (ticket 62); `Ok(None)` when
/// the entry is undefined, local, or hidden/internal, `Err` for an eligible
/// export without a plain address (STT_TLS, ticket 86).
fn defined_export(view: &ElfView, index: usize) -> Result<Option<u64>, ExportReject> {
    let Some((st_value, st_type)) = export_candidate(view, index) else {
        return Ok(None);
    };
    if st_type == relocations::STT_TLS {
        return Err(ExportReject::Tls);
    }
    Ok(Some(st_value))
}

/// The export-visible part of dynsym entry `index`: `None` for an undefined,
/// local, hidden or internal entry, otherwise `(st_value, st_type)`. TLS is
/// returned here so a versioned lookup can filter the requested version
/// before reporting that an actual matching export has no address form.
fn export_candidate(view: &ElfView, index: usize) -> Option<(u64, u8)> {
    const SHN_UNDEF: u16 = 0;
    const STB_LOCAL: u8 = 0;
    const STV_INTERNAL: u8 = 1;
    const STV_HIDDEN: u8 = 2;
    let Ok((_, st_value, st_shndx)) = view.dynsym(index) else {
        return None;
    };
    if st_shndx == SHN_UNDEF || st_shndx >= 0xff00 {
        return None;
    }
    // st_info lives at entry offset +4, st_other at +5 (translated symtab).
    let Ok(offset) = view.symtab_entry_offset(index) else {
        return None;
    };
    let (Some(&info), Some(&other)) = (view.bytes.get(offset + 4), view.bytes.get(offset + 5))
    else {
        return None;
    };
    if info >> 4 == STB_LOCAL || info & 0xf == 10 {
        return None;
    }
    let visibility = other & 0x3;
    if visibility == STV_INTERNAL || visibility == STV_HIDDEN {
        return None;
    }
    Some((st_value, info & 0xf))
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
        unsafe {
            relocations::apply(&view, base, image.as_mut_ptr(), span, &|name, version| {
                if version.is_some() {
                    None
                } else {
                    source(name)
                }
            })?;
        }
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
    let message = LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|c| c.as_bytes_with_nul().to_vec())
    });
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
    C_LOADED_MODULES
        .lock()
        .unwrap()
        .push(CModulePtr(module as *const GuestModule));
}

fn c_deregister(module: *const GuestModule) {
    let mut modules = C_LOADED_MODULES.lock().unwrap();
    modules.retain(|candidate| candidate.0 != module);
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
pub static EXEC_LOAD_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
        unsafe { CStr::from_ptr(path) }
            .to_str()
            .ok()
            .map(str::to_owned)
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
            set_last_error(&format_args!(
                "cannot load guest module from bytes: {error}"
            ));
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
pub unsafe extern "C" fn rudroid_elf_load_stats(data_loads: *mut usize, exec_loads: *mut usize) {
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
            set_last_error(&format_args!(
                "rudroid_elf_symbol: {}",
                reject.describe(name)
            ));
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
        let boxed = unsafe {
            c_deregister(std::ptr::addr_of!((*handle).module));
            Box::from_raw(handle)
        };
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
pub mod fixture;
