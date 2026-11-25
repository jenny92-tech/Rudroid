//! INPUT:  guest AArch64 ELF, registered ABI sources and Linux mapping primitives
//! OUTPUT:  executable modules, scoped file-page residency and pre-unmap finalization
//! POS:    shared executable image registry with W^X mapping and scoped cleanup
//! Linux executable mapping backend (ticket 49).
//!
//! Maps a guest AArch64 shared object with real page permissions instead of
//! materializing it into a private `Vec<u8>`: the image range is reserved,
//! each PT_LOAD's file part is mapped `MAP_PRIVATE | MAP_FIXED` from the file
//! (a byte copy when offsets do not line up or the file is on a noexec mount), BSS is
//! zero, relocations
//! run through the shared [`super::relocations`] path while the memory is
//! writable, and then every segment is `mprotect`ed to its `p_flags` —
//! write permission is dropped from executable segments, so no RWX range
//! remains after relocation (W^X).
//!
//! Instruction-cache note: copied/relocated code needs explicit AArch64
//! cache maintenance before execution. Pristine file-backed code instead
//! uses Linux's executable-page fault synchronization and stays demand
//! paged. Partial-page tail stores are still maintained explicitly; copies,
//! possible text relocations or shared segment pages retain the full flush.
//! Later code patches keep their separate maintenance (detours, ticket 51).
//!
//! Ownership: `Drop` unmaps the range — destroying the module invalidates
//! every code pointer into it (the loader never drops live kit members; this
//! path exists for tests and future scoped session teardown). Guest threads
//! must be joined before destruction; a captured Bionic range finalizer
//! drains current-thread TLS/process callbacks before code is unmapped.
//! Source/image registries publish shared snapshots: mutations affect the
//! next lookup, callbacks run without registry locks, and an in-flight
//! image lookup keeps its mapping alive even if its handle is destroyed.
//!
//! Guest-code execution here proves only that self-compiled AArch64 ELF
//! images run; Android/Bionic semantics and Silksong stay pending.

use std::collections::HashMap;
use std::ffi::c_char;
use std::path::Path;

use libc::{
    c_void, mprotect, munmap, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_EXEC, PROT_READ,
    PROT_WRITE,
};

use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::elf::{ElfView, SymbolVersion, PF_W, PF_X};
use super::relocations;
use super::{GlobalSymbols, LoadError};
use rudroid_common::symbols::elf_exec::{
    RangeFinalizer, SymbolSource as NameSource, VersionedSymbolSource,
};

/// A guest module mapped with executable, permission-separated pages.
///
/// SAFETY (self-reference): `view` borrows the immutable metadata mmap,
/// which this owner retains until Drop. Runtime pages stay mapped until Drop.
pub struct ExecutableModule {
    map: *mut u8,
    /// Page-rounded mapping length passed to mmap/munmap/mprotect.
    map_len: usize,
    base: usize,
    view: ElfView<'static>,
    /// Read-only SHARED mapping of the backing file: the ELF tables the
    /// loader reads come from the shared page cache (clean, evictable) —
    /// not a private resident copy (ticket 85). Borrowed by `view`.
    meta: *mut u8,
    meta_map_len: usize,
    /// Shared explicitly registered cleanup providers, captured before
    /// constructors. Revocation cannot strand callbacks on existing images.
    cleanup_sources: Arc<Vec<SymbolSource>>,
    /// Stable callback name; program headers borrow the existing clean metadata.
    name: std::ffi::CString,
    phdr: *const libc::Elf64_Phdr,
    phnum: u16,
    // Serialized by the image registry; atomics permit shared Arc ownership.
    dl_references: AtomicUsize,
    root_destroyed: AtomicBool,
    // Images are published before constructors for reentrant loader queries.
    // Live export metadata becomes eligible only after those constructors return.
    initializers_done: AtomicBool,
}

// SAFETY: the module owns its mapping exclusively; no interior mutability.
unsafe impl Send for ExecutableModule {}
// SAFETY: file metadata is immutable; live-table reads copy through the
// kernel into bounded owned buffers. Mapping addresses never change.
// Guest-code execution and direct code/data pointer users
// retain their own synchronization and lifetime obligations.
unsafe impl Sync for ExecutableModule {}

// ---------------------------------------------------------------------------
// Ticket 86: execution-path symbol supply.
//
// A symbol DEFINED in the module binds to its own definition (reference
// `so_relocate_rel`); only undefined imports are looked up, in the
// reference `so_resolve_link` order:
//   0. the `glad_` rewrite for GL entry points (`glXxx` → `glad_glXxx`)
//      runs the whole lookup below with the rewritten name FIRST; only a
//      miss falls back to the exact name;
//   1. host/thunk SOURCES registered through
//      `rudroid_elf_symbol_source_register`, in registration order;
//   2. the GUEST IMAGE CHAIN: every exec-mapped module, newest-loaded first
//      — the reference prepends each module to its global `so_module` list
//      (`mod->next = head; head = mod`).
//
// Explicit non-goals (ticket 86): DT_NEEDED auto-discovery (the loader's
// guest plan orders loads; the dynamic table is recorded, not followed),
// TLS-import supply and arbitrary guest-file dlopen remain unsupported.
// Versioned imports bind through an explicit version-aware source or the
// guest chain; a native same-name fallback is never attempted.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct SymbolSource {
    /// Stable id returned by the register export; used to unregister.
    id: usize,
    callback: SourceCallback,
    user: *mut c_void,
    finalizer: Option<RangeFinalizer>,
}

#[derive(Clone, Copy)]
enum SourceCallback {
    NameOnly(NameSource),
    Versioned(VersionedSymbolSource),
}

// SAFETY: the callback/user pair is the registrant's process-lifetime
// contract (thunk libraries are pinned); the host only calls through it.
unsafe impl Send for SymbolSource {}
// SAFETY: registrants keep the callback/user pair process-live and permit
// concurrent queries (see the registration contract below).
unsafe impl Sync for SymbolSource {}

static SYMBOL_SOURCES: OnceLock<Mutex<Arc<Vec<SymbolSource>>>> = OnceLock::new();
static NEXT_SOURCE_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

fn symbol_source_registry() -> &'static Mutex<Arc<Vec<SymbolSource>>> {
    SYMBOL_SOURCES.get_or_init(|| Mutex::new(Arc::new(Vec::new())))
}

fn symbol_sources() -> std::sync::MutexGuard<'static, Arc<Vec<SymbolSource>>> {
    symbol_source_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The guest image chain: exec-mapped modules in load order (lookups walk
/// it newest first). A lookup shares the snapshot's storage and ownership:
/// withdrawing a handle cannot unmap an image still being inspected.
pub(crate) fn guest_image_chain() -> Arc<Vec<Arc<ExecutableModule>>> {
    Arc::clone(
        &image_chain_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

static EXEC_IMAGE_CHAIN: OnceLock<Mutex<Arc<Vec<Arc<ExecutableModule>>>>> = OnceLock::new();

fn image_chain_registry() -> &'static Mutex<Arc<Vec<Arc<ExecutableModule>>>> {
    EXEC_IMAGE_CHAIN.get_or_init(|| Mutex::new(Arc::new(Vec::new())))
}

/// `glXxx` (but not `gla*`) entry points resolve against the thunk layer's
/// `glad_glXxx` names (reference `so_resolve_link` rewrite rule).
fn glad_rewrite(name: &str) -> Option<String> {
    if name.len() > 2 && name.starts_with("gl") && !name.starts_with("gla") {
        Some(format!("glad_{name}"))
    } else {
        None
    }
}

/// Resolve one name through the host/thunk layer (registered sources in
/// registration order).
fn host_symbol_lookup(name: &str) -> Option<usize> {
    host_symbol_lookup_versioned(name, None)
}

fn host_symbol_lookup_versioned(name: &str, version: Option<SymbolVersion<'_>>) -> Option<usize> {
    let c_name = if name.contains('\0') {
        std::ffi::CString::new(name.replace('\0', "\\0"))
    } else {
        std::ffi::CString::new(name)
    }
    .ok()?;
    let requirement = match version {
        Some(version) => Some((
            std::ffi::CString::new(version.file).ok()?,
            std::ffi::CString::new(version.name).ok()?,
        )),
        None => None,
    };
    let sources = Arc::clone(&symbol_sources());
    for source in sources.iter() {
        // SAFETY: the callback follows the registration contract; the C
        // string outlives the call.
        let address = unsafe {
            match source.callback {
                SourceCallback::NameOnly(callback) if requirement.is_none() => {
                    callback(c_name.as_ptr(), source.user)
                }
                SourceCallback::NameOnly(_) => continue,
                SourceCallback::Versioned(callback) => {
                    let (file, version) = requirement
                        .as_ref()
                        .map(|(file, version)| (file.as_ptr(), version.as_ptr()))
                        .unwrap_or((std::ptr::null(), std::ptr::null()));
                    callback(c_name.as_ptr(), file, version, source.user)
                }
            }
        };
        if address != 0 {
            return Some(address);
        }
    }
    // ELF-owned libdl services use this same image registry. Explicit
    // sources retain their order/priority; no generic native dlsym fallback.
    let address = super::libdl::symbol(name, version);
    if address == 0 {
        None
    } else {
        Some(address)
    }
}

/// Dynamic lookup borrows the caller's C string and shares sources/images.
pub(crate) fn dynamic_symbol(name: &std::ffi::CStr) -> Option<usize> {
    dynamic_symbol_lookup(name).ok().flatten()
}

/// Keep rejection distinct from absence through GL rewriting and dlsym's
/// native-companion fallback. The Option-only wrapper above is for presence
/// probes; a rejected guest definition must never unlock another address path.
pub(crate) fn dynamic_symbol_lookup(
    name: &std::ffi::CStr,
) -> Result<Option<usize>, super::ExportReject> {
    let bytes = name.to_bytes();
    // Valid GL entry names fit the shared proc cache's 256-byte bound. A
    // stack rewrite preserves relocation priority without a temporary String.
    if bytes.starts_with(b"gl")
        && !bytes.starts_with(b"gla")
        && bytes.len() > 2
        && bytes.len() <= 256
    {
        let mut rewritten = [0u8; 262];
        rewritten[..5].copy_from_slice(b"glad_");
        rewritten[5..5 + bytes.len()].copy_from_slice(bytes);
        let rewritten = std::ffi::CStr::from_bytes_with_nul(&rewritten[..6 + bytes.len()])
            .expect("stack rewrite includes its terminating NUL");
        if let Some(address) = dynamic_symbol_exact(rewritten)? {
            return Ok(Some(address));
        }
    }
    dynamic_symbol_exact(name)
}
fn dynamic_symbol_exact(name: &std::ffi::CStr) -> Result<Option<usize>, super::ExportReject> {
    let Ok(text) = name.to_str() else { return Ok(None); };
    let sources = Arc::clone(&symbol_sources());
    for source in sources.iter() {
        // SAFETY: registrants keep callbacks and `user` valid for the process
        // (register contract); `name` lives for the call, version args are null.
        let address = unsafe {
            match source.callback {
                SourceCallback::NameOnly(callback) => callback(name.as_ptr(), source.user),
                SourceCallback::Versioned(callback) => callback(
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    source.user,
                ),
            }
        };
        if address != 0 {
            return Ok(Some(address));
        }
    }
    let owned = super::libdl::symbol(text, None);
    if owned != 0 {
        Ok(Some(owned))
    } else {
        guest_chain_lookup(text)
    }
}

/// Reopen an authoritative load-plan image, returning one owned Arc reference.
pub(crate) fn reopen_guest(name: &std::ffi::CStr) -> Option<*mut ExecutableModule> {
    let chain = image_chain_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let module = chain.iter().rev().find(|module| {
        let path = module.name.to_bytes();
        path == name.to_bytes() || path.rsplit(|&byte| byte == b'/').next() == Some(name.to_bytes())
    })?;
    let references = module
        .dl_references
        .load(Ordering::Relaxed)
        .checked_add(1)?;
    module.dl_references.store(references, Ordering::Relaxed);
    Some(Arc::into_raw(Arc::clone(module)) as *mut ExecutableModule)
}

pub(crate) fn close_guest(handle: *mut ExecutableModule) -> bool {
    let mut chain = image_chain_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let Some(module) = chain.iter().find(|module| Arc::as_ptr(module) == handle) else {
        return false;
    };
    let references = module.dl_references.load(Ordering::Relaxed);
    if references == 0 {
        return false;
    }
    module
        .dl_references
        .store(references - 1, Ordering::Relaxed);
    if references == 1 && module.root_destroyed.load(Ordering::Relaxed) {
        Arc::make_mut(&mut chain).retain(|module| Arc::as_ptr(module) != handle);
    }
    drop(chain);
    // Consume exactly one reopen_guest Arc; finalizers execute outside the lock.
    // SAFETY: `handle` is in the chain with a live dl reference from `reopen_guest`'s `Arc::into_raw`.
    drop(unsafe { Arc::from_raw(handle) });
    true
}

pub(crate) fn guest_handle_symbol(handle: *mut ExecutableModule, name: &str) -> Option<usize> {
    let chain = guest_image_chain();
    chain
        .iter()
        .find(|module| {
            Arc::as_ptr(module) == handle && module.dl_references.load(Ordering::Relaxed) != 0
        })?
        .symbol(name)
}

/// Compare one handle-specific lookup with the runtime dynamic table. This is
/// a read-only evidence path: the normal lookup result is never changed here.
pub(crate) fn guest_handle_symbol_diagnostic(
    handle: *mut ExecutableModule,
    name: &str,
    claim: impl FnOnce() -> bool,
) -> Option<SymbolDiagnostic> {
    let chain = guest_image_chain();
    let module = chain.iter().find(|module| Arc::as_ptr(module) == handle)?;
    if !claim() {
        return None;
    }
    Some(module.symbol_diagnostic(name))
}

/// Visit bounded whole-chain diagnostics for one global dlsym. The caller
/// claims each module's work immediately before the live report is built;
/// this snapshot never changes lookup behavior.
pub(crate) fn guest_chain_symbol_diagnostics(
    name: &str,
    max_modules: usize,
    mut claim: impl FnMut() -> bool,
    mut report: impl FnMut(SymbolDiagnostic),
) {
    let chain = guest_image_chain();
    for module in chain.iter().rev().take(max_modules) {
        if !claim() {
            break;
        }
        report(module.symbol_diagnostic(name));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileSymbolResult {
    Hit(usize),
    Miss,
    Rejected(&'static str),
}

impl std::fmt::Display for FileSymbolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hit(address) => write!(f, "hit(0x{address:x})"),
            Self::Miss => f.write_str("miss"),
            Self::Rejected(reason) => write!(f, "rejected({reason})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolDiagnostic {
    module: String,
    name: String,
    file: FileSymbolResult,
    live: super::live_symbols::LiveReport,
}

impl std::fmt::Display for SymbolDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[ELF-LIVE] module={:?} name={:?} file={} live={} hash={} tags={}",
            self.module, self.name, self.file, self.live.result, self.live.hash, self.live.tags
        )
    }
}

pub(crate) fn guest_address_info(address: usize) -> Option<super::libdl::DlInfo> {
    let chain = guest_image_chain();
    let module = chain.iter().rev().find(|module| {
        module.view.loads.iter().any(|load| {
            let start = module.base.wrapping_add(load.vaddr as usize);
            address >= start && address - start < load.memsz as usize
        })
    })?;
    Some(super::libdl::DlInfo {
        filename: module.name.as_ptr(),
        base: module.base as *mut c_void,
        symbol_name: std::ptr::null(),
        symbol_address: std::ptr::null_mut(),
    })
}

/// The guest cross-module layer: previously loaded exec images, newest
/// first (reference `so_module` head-insertion order).
fn guest_chain_lookup(name: &str) -> Result<Option<usize>, super::ExportReject> {
    let chain = guest_image_chain();
    for module in chain.iter().rev() {
        // A genuine miss may try an older provider; a present but unusable
        // TLS/versioned definition must stop the whole address lookup.
        if let Some(address) = module.symbol_lookup(name)? {
            return Ok(Some(address));
        }
    }
    Ok(None)
}

/// Versioned guest lookup over the whole image chain, newest first. Bionic
/// does not restrict a lookup to the VERNEED `vn_file`: that name records
/// the dependency which introduced the requirement, not the only DSO that
/// may interpose. Each candidate DSO applies its own VERDEF/GLOBAL rule and
/// a wrong version does not hide a later provider's valid definition.
fn guest_chain_lookup_versioned(name: &str, required: SymbolVersion<'_>) -> Option<usize> {
    let chain = guest_image_chain();
    for module in chain.iter().rev() {
        match module.symbol_versioned(name, required) {
            Ok(Some(address)) => return Some(address),
            Ok(None) => continue,
            // A malformed table or unusable TLS definition is a hard miss for
            // this name: never fall through to an unrelated older provider.
            Err(_) => return None,
        }
    }
    None
}

/// Reference `so_resolve_link` for one undefined import: the `glad_`
/// rewritten name through every layer first, then the exact name.
/// `globals` is the in-process explicit layer of [`ExecutableModule::map`]
/// (empty on the production C path), consulted ahead of the sources.
fn resolve_link(
    name: &str,
    version: Option<SymbolVersion<'_>>,
    globals: GlobalSymbols<'_>,
) -> Option<usize> {
    if let Some(version) = version {
        // Registered version-aware host providers stay first. The guest
        // image chain is the second version-aware layer; `vn_file` does not
        // scope it. A versioned import never reaches `globals`, name-only
        // sources, `glad_` rewriting, or a bare guest export.
        return host_symbol_lookup_versioned(name, Some(version))
            .or_else(|| guest_chain_lookup_versioned(name, version));
    }
    let exact = |name: &str| -> Result<Option<usize>, super::ExportReject> {
        match globals
            .get(name)
            .copied()
            .or_else(|| host_symbol_lookup(name))
        {
            Some(address) => Ok(Some(address)),
            None => guest_chain_lookup(name),
        }
    };
    if let Some(rewritten) = glad_rewrite(name) {
        match exact(&rewritten) {
            Ok(Some(address)) => return Some(address),
            Ok(None) => {},
            Err(_) => return None, // Not permission to retry the original GL name.
        }
    }
    exact(name).ok().flatten()
}

fn page_size() -> usize {
    // SAFETY: sysconf(_SC_PAGESIZE) is always safe.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

fn align_up(value: usize, page: usize) -> usize {
    (value + page - 1) & !(page - 1)
}

fn align_down(value: usize, page: usize) -> usize {
    value & !(page - 1)
}

/// AArch64 cache maintenance over a byte range that was written by data
/// stores and will be executed: clean D-cache to the point of unification,
/// complete D-cache clean before invalidating I-cache, then order with
/// `dsb ish` + `isb`. Reuses the detour cache-line implementation.
/// Other targets are
/// architecturally coherent — nothing to do.
#[inline]
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn clear_cache_range(start: usize, end: usize) {
    // SAFETY: this byte range belongs to the module's mapped segment.
    unsafe { rudroid_common::arm64_detour::flush_code(start as *mut u8, end - start) };
}

fn shares_load_page(view: &ElfView, index: usize, page: usize) -> bool {
    let load = &view.loads[index];
    load.memsz != 0 && view.loads.iter().enumerate().any(|(other, segment)| {
        other != index && segment.memsz != 0
            && align_down(load.vaddr as usize, page) < align_up((segment.vaddr + segment.memsz) as usize, page)
            && align_down(segment.vaddr as usize, page) < align_up((load.vaddr + load.memsz) as usize, page)
    })
}

#[inline]
#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
fn clear_cache_range(_start: usize, _end: usize) {}

/// Full maintenance is required for any copy, possible text relocation, or
/// page shared by segments. Otherwise only tail-zeroing touched executable
/// file pages. Walking pristine cold code with dc/ic would fault the entire
/// segment into RAM, defeating demand paging before any guest code runs.
/// Linux synchronizes pristine file pages as executable PTEs are installed:
/// https://github.com/torvalds/linux/blob/v5.10/arch/arm64/include/asm/pgtable.h
/// https://github.com/torvalds/linux/blob/v5.10/arch/arm64/mm/flush.c
fn clear_executable_segments(
    view: &ElfView,
    base: usize,
    map_len: usize,
    page: usize,
    copied_loads: bool,
    writes: relocations::RelocationWrites,
) {
    let full = copied_loads

        || view.loads.iter().any(|load| {
            load.flags & PF_X != 0 && writes.intersects(load.vaddr, load.vaddr + load.memsz)
        });
    for load in &view.loads {
        if load.flags & PF_X == 0 || load.filesz == 0 {
            continue;
        }
        let mut start = load.vaddr as usize;
        let mut end = (start + load.filesz as usize).min(map_len);
        if !full {
            if end % page == 0 {
                continue;
            }
            // The file mapper zeroed this partial page while writable. Keep
            // explicit maintenance for the complete owned page, including
            // its zeroed suffix, without touching pristine earlier pages.
            start = align_down(end, page);
            end = align_up(end, page).min(map_len);
        }
        if end > start {
            clear_cache_range(base + start, base + end);
        }
    }
}

/// Run the guest's standard initializers: the optional single DT_INIT
/// function first, then every non-null `.init_array` constructor. Parse has
/// already bounded DT_INIT to a file-backed executable PT_LOAD, and
/// relocations have filled each array slot. Each initializer runs exactly
/// once; a crashing constructor kills the process — the guest is the buggy
/// party, exactly as in the reference.
///
/// SAFETY: callers must have run [`clear_executable_segments`] first.
unsafe fn run_initializers(path: &Path, view: &ElfView, base: usize) {
    if let Some(vaddr) = view.init {
        let entry = base + vaddr as usize;
        rudroid_common::diagnostic!(
            "[ELF] phase=DT_INIT-begin path={} entry={entry:#x}",
            path.display()
        );
        // SAFETY: parse bounded DT_INIT to executable code in this mapping;
        // the ELF initializer ABI is `void (*)(void)`.
        unsafe {
            let init: unsafe extern "C" fn() = std::mem::transmute(entry);
            init();
        }
        rudroid_common::diagnostic!(
            "[ELF] phase=DT_INIT-done path={} entry={entry:#x}",
            path.display()
        );
    }
    let Some((vaddr, size)) = view.init_array else {
        rudroid_common::diagnostic!(
            "[ELF] phase=initializers-done path={} dt_init={} init_array=0",
            path.display(),
            view.init.is_some()
        );
        return;
    };
    for index in 0..(size as usize / 8) {
        let slot = base + vaddr as usize + index * 8;
        // SAFETY: the range was parse-validated as file-backed and is fully
        // mapped; the pointer is relocation-final.
        let entry = unsafe { std::ptr::read_unaligned(slot as *const usize) };
        if entry != 0 && entry != usize::MAX {
            rudroid_common::diagnostic!(
                "[ELF] phase=init_array-begin path={} index={index} entry={entry:#x}",
                path.display()
            );
            // SAFETY: guest constructor code on RX pages of our mapping.
            let ctor: unsafe extern "C" fn() = unsafe { std::mem::transmute(entry) };
            // SAFETY: an ELF init_array entry takes no arguments (C ABI).
            unsafe { ctor() };
            rudroid_common::diagnostic!(
                "[ELF] phase=init_array-done path={} index={index} entry={entry:#x}",
                path.display()
            );
        }
    }
    rudroid_common::diagnostic!(
        "[ELF] phase=initializers-done path={} dt_init={} init_array={}",
        path.display(),
        view.init.is_some(),
        size as usize / 8
    );
}

impl ExecutableModule {
    /// Map the shared object at `path` for execution (in-process API —
    /// standalone, NOT appended to the guest image chain; production loads
    /// go through [`Self::map_and_register`]).
    ///
    /// # Safety
    /// The file must contain trusted executable code with matching import
    /// ABIs, and remain unmodified/untruncated until this module is dropped.
    /// Imports and their state must outlive every guest call. Constructors
    /// may execute immediately. Before Drop, join all guest threads and
    /// quiesce pointers/callbacks into this image, including TLS destructors.
    pub unsafe fn map(
        path: impl AsRef<Path>,
        globals: GlobalSymbols<'_>,
    ) -> Result<Self, LoadError> {
        // SAFETY: the caller owns the file/code/import lifetime contract.
        let module = unsafe { Self::map_with_layers(path.as_ref(), globals) }?;
        // SAFETY: the image is mapped, relocated and protected.
        unsafe { run_initializers(path.as_ref(), &module.view, module.base) };
        module.initializers_done.store(true, Ordering::Release);
        Ok(module)
    }

    /// Production load: map with the layered lookup and append the image to
    /// the guest chain (ticket 86 load-order semantics). The returned
    /// pointer is process-pinned by the caller (retained Arc, ticket 46).
    unsafe fn map_and_register(
        path: &Path,
        globals: GlobalSymbols<'_>,
    ) -> Result<*mut ExecutableModule, LoadError> {
        // SAFETY: the exec-load caller supplies trusted, stable guest code.
        let module = Arc::new(unsafe { Self::map_with_layers(path, globals) }?);
        let ptr = Arc::into_raw(Arc::clone(&module)) as *mut ExecutableModule;
        {
            let mut chain = image_chain_registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::make_mut(&mut chain).push(Arc::clone(&module));
        }
        // Publish the immutable, protected image before constructors: a
        // constructor's dl_iterate_phdr must see itself and may reenter.
        // SAFETY: as `map`.
        unsafe { run_initializers(path, &module.view, module.base) };
        module.initializers_done.store(true, Ordering::Release);
        Ok(ptr)
    }

    // Safety: stable backing file, trusted code/imports, quiescence at Drop.
    unsafe fn map_with_layers(path: &Path, globals: GlobalSymbols<'_>) -> Result<Self, LoadError> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::AsRawFd;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| LoadError::Unsupported("ELF path contains NUL"))?;
        rudroid_common::diagnostic!("[ELF] phase=open-begin path={}", path.display());
        let file = std::fs::File::open(path).map_err(LoadError::Io)?;
        let noexec = rudroid_common::filesystem::FilesystemStats::for_file(&file)
            .map_err(LoadError::Io)?
            .noexec;
        let file_len = usize::try_from(file.metadata().map_err(LoadError::Io)?.len())
            .ok()
            .filter(|len| *len <= isize::MAX as usize)
            .ok_or(LoadError::Unsupported(
                "ELF file exceeds slice address range",
            ))?;
        let page = page_size();
        rudroid_common::diagnostic!(
            "[ELF] phase=open-done path={} file_len={file_len} noexec={noexec}",
            path.display()
        );
        // Explicit same-kit lifecycle ABI; no symbol lookup/cast can turn a
        // legacy source's generic answer into an accidental callback. Share
        // the registry snapshot and keep providers process-pinned on revoke.
        let cleanup_sources = Arc::clone(&symbol_sources());

        // Ticket 85: metadata mapping — read-only SHARED over the whole
        // file. The ELF tables the loader reads live in the shared page
        // cache (clean, evictable), replacing the private resident `raw`
        // copy. Support premise: the backing file stays unmodified for the
        // process lifetime.
        let meta_len = align_up(file_len.max(1), page);
        // SAFETY: fd valid; mapping covers the file length.
        let meta = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                meta_len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if meta == MAP_FAILED {
            return Err(LoadError::Mapping(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let meta = meta as *mut u8;
        rudroid_common::diagnostic!(
            "[ELF] phase=metadata-mapped path={} len={meta_len}",
            path.display()
        );
        // SAFETY: bytes [0, file_len) are mapped readable above.
        let meta_bytes: &[u8] = unsafe { std::slice::from_raw_parts(meta as *const u8, file_len) };

        let result =
            (|| -> Result<(ElfView<'static>, *mut u8, usize, usize, usize, u16), LoadError> {
                let view = ElfView::parse(meta_bytes)?;
                rudroid_common::diagnostic!(
                    "[ELF] phase=parsed path={} loads={} needed={}",
                    path.display(),
                    view.loads.len(),
                    view.needed.len()
                );
                let phoff = u64::from_le_bytes(view.bytes[32..40].try_into().unwrap()) as usize;
                let stride = u16::from_le_bytes(view.bytes[54..56].try_into().unwrap()) as usize;
                let phnum = u16::from_le_bytes(view.bytes[56..58].try_into().unwrap());
                if stride != std::mem::size_of::<libc::Elf64_Phdr>()
                    || phoff % std::mem::align_of::<libc::Elf64_Phdr>() != 0
                {
                    return Err(LoadError::Unsupported(
                        "unaligned or nonstandard ELF64 program-header stride",
                    ));
                }

                // Ticket 85 rework: reject hostile layouts before anything is
                // mapped — a page covered by BOTH an executable and a writable
                // segment could only end RWX (forbidden) or lose +x on its code
                // half (the old two-pass bug). Real linker output never produces
                // this; see `reject_writable_executable_page_overlap`.
                // Bound the span before any page-rounding arithmetic.
                let span = view.image_span()? as usize;
                Self::reject_writable_executable_page_overlap(&view, page)?;
                let map_len = align_up(span.max(1), page);

                // Reserve the whole image span as private anonymous RW memory,
                // then replace each PT_LOAD's file-backed part with a private
                // mapping of the file itself (ticket 85): clean code pages are
                // page-cache shared and reclaimable; writes become COW private.
                // SAFETY: mapping primitive on a fresh reservation.
                let map = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        map_len,
                        PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS,
                        -1,
                        0,
                    )
                };
                if map == MAP_FAILED {
                    return Err(LoadError::Mapping(
                        std::io::Error::last_os_error().to_string(),
                    ));
                }
                let map: *mut u8 = map as *mut u8;
                let base = map as usize;
                rudroid_common::diagnostic!(
                    "[ELF] phase=reserved path={} base={base:#x} span={span} map_len={map_len}",
                    path.display()
                );

                let segment_result = (|| -> Result<(), LoadError> {
                    let mut copied_loads = noexec;
                    // File-backed mappings per PT_LOAD (offset must be
                    // page-aligned; ELF guarantees p_vaddr ≡ p_offset mod page,
                    // verified below). BSS tails stay anonymous.
                    for (index, load) in view.loads.iter().enumerate() {
                        if load.filesz == 0 {
                            continue;
                        }
                        let off = load.offset as usize;
                        let vaddr = load.vaddr as usize;
                        // File-backed mapping needs p_vaddr ≡ p_offset (mod
                        // page). Synthetic/hostile layouts that violate the
                        // congruence fall back to the byte copy (the reference
                        // memcpy loader accepts them too) — real linker output
                        // is always congruent and gets the reclaimable pages.
                        let congruent = vaddr % page == off % page;
                        let shared_pages = shares_load_page(&view, index, page);
                        rudroid_common::diagnostic!(
                            "[ELF] phase=segment-begin path={} index={index} vaddr={vaddr:#x} \
                             filesz={} flags={:#x} copied={} congruent={congruent}",
                            path.display(),
                            load.filesz,
                            load.flags,
                            !congruent || noexec || shared_pages
                        );
                        // noexec files may be read but their mappings cannot gain
                        // execute permission. Keep the entire reserved image
                        // anonymous in this case, including overlapping read-only
                        // pages, then use the same relocation/W^X/cache-flush path.
                        if !congruent || noexec || shared_pages {
                            copied_loads = true;
                            let src = &meta_bytes[off..off + load.filesz as usize];
                            // SAFETY: vaddr + filesz <= span <= map_len.
                            let dst = unsafe { map.add(vaddr) };
                            // SAFETY: disjoint ranges inside our own mappings.
                            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) };
                            rudroid_common::diagnostic!(
                                "[ELF] phase=segment-done path={} index={index} copied=true",
                                path.display()
                            );
                            continue;
                        }
                        let off_aligned = align_down(off, page);
                        let delta = off - off_aligned;
                        let start = base + vaddr - delta;
                        let len = align_up(load.filesz as usize + delta, page);
                        if start < base || start + len > base + map_len {
                            return Err(LoadError::Mapping(
                                "file-backed segment mapping exceeds the reserved span".to_owned(),
                            ));
                        }
                        // SAFETY: [start, start+len) sits inside our reserved
                        // region; the fd and page-aligned offset are valid.
                        let mapped = unsafe {
                            libc::mmap(
                                start as *mut libc::c_void,
                                len,
                                PROT_READ | PROT_WRITE,
                                MAP_PRIVATE | libc::MAP_FIXED,
                                file.as_raw_fd(),
                                off_aligned as i64,
                            )
                        };
                        if mapped == MAP_FAILED {
                            return Err(LoadError::Mapping(
                                std::io::Error::last_os_error().to_string(),
                            ));
                        }
                        // BSS tail semantics: the partial page past filesz may
                        // carry file bytes — zero it while writable (bionic
                        // zeros partial tails the same way). Offsets here are
                        // relative to the region start (data_end is absolute).
                        let data_end = base + load.vaddr as usize + load.filesz as usize;
                        let zero_end = align_up(data_end, page).min(base + map_len);
                        if zero_end > data_end {
                            // SAFETY: [data_end, zero_end) inside the mapped
                            // region (file mapping covers filesz-aligned-up).
                            unsafe {
                                std::ptr::write_bytes(
                                    map.add(data_end - base),
                                    0,
                                    zero_end - data_end,
                                )
                            };
                        }
                        rudroid_common::diagnostic!(
                            "[ELF] phase=segment-done path={} index={index} copied=false",
                            path.display()
                        );
                    }
                    // Relocations run while every page is writable. Defined
                    // symbols bind to this module inside `relocations`;
                    // undefined imports take the `resolve_link` order (ticket
                    // 86, reference `so_resolve_link`), then weak → 0.
                    rudroid_common::diagnostic!(
                        "[ELF] phase=relocate-begin path={} base={base:#x}",
                        path.display()
                    );
                    let writes = relocations::apply(&view, base, map, span, &|name, version| {
                        resolve_link(name, version, globals)
                    })?;
                    rudroid_common::diagnostic!(
                        "[ELF] phase=relocate-done path={}",
                        path.display()
                    );
                    // Install read-only first so it cannot erase execute permission
                    // on a shared page. Writable pages are installed last, without X.
                    for class in [0, PF_X, PF_W] {
                        for load in &view.loads {
                            let protection = if load.flags & PF_X != 0 { PF_X } else { load.flags & PF_W };
                            if protection == class {
                                Self::protect_segment(map, map_len, page, load, class == PF_W)?;
                            }
                        }
                    }
                    rudroid_common::diagnostic!(
                        "[ELF] phase=protected path={}",
                        path.display()
                    );
                    // Maintain all possibly written code before publication;
                    // pristine file code remains cold until its first use.
                    clear_executable_segments(&view, base, map_len, page, copied_loads, writes);
                    rudroid_common::diagnostic!(
                        "[ELF] phase=cache-maintained path={}",
                        path.display()
                    );
                    if noexec {
                        // Copying PT_LOADs faults their source file pages into this
                        // metadata mapping as well as the anonymous runtime image.
                        // Discard those redundant PTEs once, before publication or
                        // constructors. Keep the mapping and all ELF borrows alive;
                        // subsequent metadata queries fault back only the tables
                        // they read. Never advise the anonymous runtime image.
                        // SAFETY: meta is our page-aligned, read-only MAP_SHARED
                        // file mapping for meta_len bytes. MADV_DONTNEED preserves
                        // its address, permissions and immutable backing contents
                        // (the map caller must keep the file unmodified). Failure
                        // only retains clean pages; it cannot invalidate the image.
                        let _ =
                            unsafe { libc::madvise(meta.cast(), meta_len, libc::MADV_DONTNEED) };
                    }
                    Ok(())
                })();
                if let Err(error) = segment_result {
                    // Mid-way failure must not leak the region (ticket 49).
                    // Ticket 85 rework — single cleanup ownership: THIS closure
                    // owns the region mapping and unmaps it exactly once; the
                    // metadata mapping belongs to the caller's error path below
                    // and is never touched here. The old path munmapped meta in
                    // BOTH places — if a concurrent allocation reused the gap
                    // between the two calls, the second munmap pulled the
                    // stranger's mapping out from under it.
                    // SAFETY: the region mappings are ours and still whole here.
                    unsafe {
                        munmap(map as *mut libc::c_void, map_len);
                    }
                    return Err(error);
                }
                // SAFETY: `view` borrows the meta mapping, which the struct
                // keeps alive for the process (see Drop).
                let view: ElfView<'static> =
                    unsafe { std::mem::transmute::<ElfView<'_>, ElfView<'static>>(view) };
                Ok((view, map, map_len, base, phoff, phnum))
            })();

        match result {
            Ok((view, map, map_len, base, phoff, phnum)) => {
                Ok(ExecutableModule {
                    map,
                    map_len,
                    base,
                    view,
                    meta,
                    meta_map_len: meta_len,
                    cleanup_sources,
                    name,
                    // SAFETY: `phoff` lies inside the meta mapping (parse-validated).
                    phdr: unsafe { meta.add(phoff).cast() },
                    phnum,
                    dl_references: AtomicUsize::new(0),
                    root_destroyed: AtomicBool::new(false),
                    initializers_done: AtomicBool::new(false),
                })
            }
            Err(error) => {
                // Ticket 85 rework: the SOLE cleanup owner of the metadata
                // mapping. Every failure funnels here exactly once — parse
                // errors (region never created), region mmap failure, and
                // segment-stage errors (the region was already unmapped by
                // its own owner above; meta was never touched there).
                // SAFETY: created above, not yet published.
                unsafe {
                    munmap(meta as *mut libc::c_void, meta_len);
                }
                Err(error)
            }
        }
    }

    pub(crate) fn phdr_info(&self) -> super::libdl::PhdrInfo {
        super::libdl::PhdrInfo {
            addr: self.base,
            name: self.name.as_ptr(),
            phdr: self.phdr,
            phnum: self.phnum,
        }
    }

    /// Ticket 85 rework: refuse layouts where ONE page would need to be both
    /// executable and writable. The old two-pass W^X applied R|X in pass one
    /// and then R|W in pass two, so a page shared by an X and a W segment
    /// silently lost +x on its code half while the load reported success;
    /// the only "correct" union would be RWX, which is forbidden (ticket 49).
    /// Real linker output separates PT_LOADs page-apart; congruence-violating
    /// or hostile layouts are rejected structurally with a diagnostic.
    /// Page ranges mirror `protect_segment` exactly (memsz, page-rounding,
    /// clamped to the mapping).
    fn reject_writable_executable_page_overlap(
        view: &ElfView,
        page: usize,
    ) -> Result<(), LoadError> {
        for exec in &view.loads {
            if exec.memsz == 0 || exec.flags & PF_X == 0 {
                continue;
            }
            let x_start = align_down(exec.vaddr as usize, page);
            let x_end = align_up((exec.vaddr + exec.memsz) as usize, page);
            for write in &view.loads {
                if write.memsz == 0 || write.flags & PF_W == 0 {
                    continue;
                }
                let w_start = align_down(write.vaddr as usize, page);
                let w_end = align_up((write.vaddr + write.memsz) as usize, page);
                if x_start < w_end && w_start < x_end {
                    return Err(LoadError::Mapping(format!(
                        "execute segment [{x_start:#x},{x_end:#x}) and writable segment \
                         [{w_start:#x},{w_end:#x}) share page(s): the layout cannot honor \
                         W^X without either RWX or losing +x on code — refusing (ticket 85)"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Apply one segment's final protections. `writable` selects the
    /// second (writable) pass of the two-pass W^X application.
    fn protect_segment(
        map: *mut u8,
        map_len: usize,
        page: usize,
        load: &super::elf::LoadSegment,
        writable: bool,
    ) -> Result<(), LoadError> {
        if load.memsz == 0 { return Ok(()); }
        let start = align_down(load.vaddr as usize, page);
        let end = align_up((load.vaddr + load.memsz) as usize, page).min(map_len);
        if end <= start {
            return Ok(());
        }
        let mut prot = PROT_READ;
        if load.flags & PF_X != 0 {
            prot |= PROT_EXEC;
        }
        if writable {
            prot |= PROT_WRITE;
        }
        // SAFETY: [start, end) is page-aligned inside our mapping.
        // SAFETY: inside our mapping; page-aligned span.
        let rc = unsafe { mprotect(map.add(start) as *mut libc::c_void, end - start, prot) };
        if rc != 0 {
            return Err(LoadError::Mapping(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }

    /// Image base (load bias): vaddr 0 of the guest module.
    pub fn base(&self) -> usize {
        self.base
    }

    /// Mapping length in bytes (page-rounded).
    pub fn map_len(&self) -> usize {
        self.map_len
    }

    /// Look a dynamic symbol up by name; absolute host address. File exports
    /// retain the buffer backend's eligibility rules. After initializers, a
    /// file miss can use the stricter bounded live-table fallback. TLS has
    /// no process address and answers `None` (ticket 86).
    pub fn symbol(&self, name: &str) -> Option<usize> {
        self.symbol_lookup(name).ok().flatten()
    }

    /// Look up a versioned definition in this module using Bionic's per-DSO
    /// VERDEF/GLOBAL rule. A symbol with the right bare name but the wrong
    /// version, LOCAL index, or malformed version table is not an answer.
    fn symbol_versioned(
        &self,
        name: &str,
        required: SymbolVersion<'_>,
    ) -> Result<Option<usize>, super::ExportReject> {
        super::exported_symbol_value_versioned(&self.view, name, required)
            .map(|value| value.map(|value| self.base.wrapping_add(value as usize)))
    }

    /// [`Self::symbol`] keeping the reason a present export is refused.
    fn symbol_lookup(&self, name: &str) -> Result<Option<usize>, super::ExportReject> {
        match self.file_symbol_lookup(name)? {
            Some(address) => Ok(Some(address)),
            None if self.initializers_done.load(Ordering::Acquire) => {
                super::live_symbols::bind(&self.view, self.base, name)
            }
            None => Ok(None),
        }
    }

    fn file_symbol_lookup(&self, name: &str) -> Result<Option<usize>, super::ExportReject> {
        super::exported_symbol_value(&self.view, name)
            .map(|value| value.map(|value| self.base.wrapping_add(value as usize)))
    }

    /// Read-only file-versus-live evidence for a missed symbol. The original
    /// file result deliberately bypasses the normal live binding fallback;
    /// tracing must not change resolution or conceal an original file miss.
    pub(crate) fn symbol_diagnostic(&self, name: &str) -> SymbolDiagnostic {
        let file = match self.file_symbol_lookup(name) {
            Ok(Some(address)) => FileSymbolResult::Hit(address),
            Ok(None) => FileSymbolResult::Miss,
            Err(super::ExportReject::Tls) => FileSymbolResult::Rejected("tls"),
            Err(super::ExportReject::MalformedVersion(_)) => {
                FileSymbolResult::Rejected("malformed-version")
            }
        };
        SymbolDiagnostic {
            module: self.name.to_string_lossy().into_owned(),
            name: name.to_owned(),
            file,
            live: super::live_symbols::diagnose(&self.view, self.base, name),
        }
    }

    /// Reference `so_symbol_all`: the dynamic-symbol lookup first, then on a
    /// miss the file's section-header `.symtab` (unstripped builds keep
    /// hidden/local functions there). The ticket-85 metadata mapping
    /// already covers the whole backing file, so no re-open is needed. A
    /// TLS match is refused on either path (ticket 86).
    pub fn symbol_all(&self, name: &str) -> Result<Option<usize>, super::ExportReject> {
        if let Some(address) = self.symbol_lookup(name)? {
            return Ok(Some(address));
        }
        match self.view.section_symtab_lookup(name) {
            Some((_, info)) if info & 0xf == relocations::STT_TLS => Err(super::ExportReject::Tls),
            Some((st_value, _)) => Ok(Some(self.base.wrapping_add(st_value as usize))),
            None => Ok(None),
        }
    }
}

impl Drop for ExecutableModule {
    fn drop(&mut self) {
        // Guest callbacks must run while their code and DSO storage are
        // still mapped. Destroy requires joined threads/quiescent callbacks.
        for finalizer in self
            .cleanup_sources
            .iter()
            .filter_map(|source| source.finalizer)
        {
            // SAFETY: registrant finalizer for the still-mapped range (register contract).
            unsafe {
                finalizer(self.map as usize, self.map_len);
            }
        }
        // SAFETY: both mappings were created in `map` and dropped exactly
        // once; the backing file must stay unmodified for the process
        // lifetime (documented support premise, ticket 85).
        unsafe {
            munmap(self.map as *mut libc::c_void, self.map_len);
            munmap(self.meta as *mut libc::c_void, self.meta_map_len);
        }
    }
}

// ---------------------------------------------------------------------------
// C exports (Linux only). Handles are distinct from the buffer-load handle.

#[path = "exec_exports.rs"]
mod exports;
pub use exports::*;

#[cfg(test)]
#[path = "../tests/unit/exec_lookup_tests.rs"]
mod lookup_tests;

#[cfg(test)]
#[path = "../tests/unit/live_symbols.rs"]
mod live_symbols_tests;
