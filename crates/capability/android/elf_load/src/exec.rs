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
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::elf::{ElfView, SymbolVersion, PF_W, PF_X};
use rudroid_common::symbols::elf_exec::{RangeFinalizer, SymbolSource as NameSource, VersionedSymbolSource};
use super::relocations;
use super::{GlobalSymbols, LoadError};


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
}

// SAFETY: the module owns its mapping exclusively; no interior mutability.
unsafe impl Send for ExecutableModule {}
// SAFETY: symbol queries read immutable ELF metadata; mapping addresses
// never change. Guest-code execution and direct code/data pointer users
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
// TLS-import supply and versioned guest-definition lookup remain unsupported.
// Versioned imports bind through an explicit version-aware source or the
// owned libdl provider, never a native same-name fallback.
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
static NEXT_SOURCE_ID: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(1);

fn symbol_source_registry() -> &'static Mutex<Arc<Vec<SymbolSource>>> {
    SYMBOL_SOURCES.get_or_init(|| Mutex::new(Arc::new(Vec::new())))
}

fn symbol_sources() -> std::sync::MutexGuard<'static, Arc<Vec<SymbolSource>>> {
    symbol_source_registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The guest image chain: exec-mapped modules in load order (lookups walk
/// it newest first). A lookup shares the snapshot's storage and ownership:
/// withdrawing a handle cannot unmap an image still being inspected.
pub(crate) fn guest_image_chain() -> Arc<Vec<Arc<ExecutableModule>>> {
    Arc::clone(&image_chain_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
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
    }.ok()?;
    let requirement = match version {
        Some(version) => Some((std::ffi::CString::new(version.file).ok()?,
                              std::ffi::CString::new(version.name).ok()?)),
        None => None,
    };
    let sources = Arc::clone(&symbol_sources());
    for source in sources.iter() {
        // SAFETY: the callback follows the registration contract; the C
        // string outlives the call.
        let address = unsafe { match source.callback {
            SourceCallback::NameOnly(callback) if requirement.is_none() =>
                callback(c_name.as_ptr(), source.user),
            SourceCallback::NameOnly(_) => continue,
            SourceCallback::Versioned(callback) => {
                let (file, version) = requirement.as_ref()
                    .map(|(file, version)| (file.as_ptr(), version.as_ptr()))
                    .unwrap_or((std::ptr::null(), std::ptr::null()));
                callback(c_name.as_ptr(), file, version, source.user)
            }
        } };
        if address != 0 {
            return Some(address);
        }
    }
    // ELF-owned libdl services use this same image registry. Explicit
    // sources retain their order/priority; no generic native dlsym fallback.
    let address = super::libdl::symbol(name, version);
    if address == 0 { None } else { Some(address) }
}

/// Dynamic lookup borrows the caller's C string and shares sources/images.
pub(crate) fn dynamic_symbol(name: &std::ffi::CStr) -> Option<usize> {
    let bytes = name.to_bytes();
    // Valid GL entry names fit the shared proc cache's 256-byte bound. A
    // stack rewrite preserves relocation priority without a temporary String.
    if bytes.starts_with(b"gl") && !bytes.starts_with(b"gla") && bytes.len() > 2 && bytes.len() <= 256 {
        let mut rewritten = [0u8; 262];
        rewritten[..5].copy_from_slice(b"glad_");
        rewritten[5..5 + bytes.len()].copy_from_slice(bytes);
        let rewritten = std::ffi::CStr::from_bytes_with_nul(&rewritten[..6 + bytes.len()]).ok()?;
        if let Some(address) = dynamic_symbol_exact(rewritten) { return Some(address); }
    }
    dynamic_symbol_exact(name)
}
fn dynamic_symbol_exact(name: &std::ffi::CStr) -> Option<usize> {
    let text = name.to_str().ok()?;
    let sources = Arc::clone(&symbol_sources());
    for source in sources.iter() {
        // SAFETY: registrants keep callbacks and `user` valid for the process
        // (register contract); `name` lives for the call, version args are null.
        let address = unsafe { match source.callback {
            SourceCallback::NameOnly(callback) => callback(name.as_ptr(), source.user),
            SourceCallback::Versioned(callback) => callback(name.as_ptr(), std::ptr::null(), std::ptr::null(), source.user),
        } };
        if address != 0 { return Some(address); }
    }
    let owned = super::libdl::symbol(text, None);
    if owned != 0 { Some(owned) } else { guest_chain_lookup(text) }
}

/// Reopen an authoritative load-plan image, returning one owned Arc reference.
pub(crate) fn reopen_guest(name: &std::ffi::CStr) -> Option<*mut ExecutableModule> {
    let chain = image_chain_registry().lock().unwrap_or_else(|p| p.into_inner());
    let module = chain.iter().rev().find(|module| {
        let path = module.name.to_bytes();
        path == name.to_bytes() || path.rsplit(|&byte| byte == b'/').next() == Some(name.to_bytes())
    })?;
    let references = module.dl_references.load(Ordering::Relaxed).checked_add(1)?;
    module.dl_references.store(references, Ordering::Relaxed);
    Some(Arc::into_raw(Arc::clone(module)) as *mut ExecutableModule)
}

pub(crate) fn close_guest(handle: *mut ExecutableModule) -> bool {
    let mut chain = image_chain_registry().lock().unwrap_or_else(|p| p.into_inner());
    let Some(module) = chain.iter().find(|module| Arc::as_ptr(module) == handle) else { return false; };
    let references = module.dl_references.load(Ordering::Relaxed);
    if references == 0 { return false; }
    module.dl_references.store(references - 1, Ordering::Relaxed);
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
    chain.iter().find(|module| Arc::as_ptr(module) == handle
        && module.dl_references.load(Ordering::Relaxed) != 0)?.symbol(name)
}

pub(crate) fn guest_address_info(address: usize) -> Option<super::libdl::DlInfo> {
    let chain = guest_image_chain();
    let module = chain.iter().rev().find(|module| module.view.loads.iter().any(|load| {
        let start = module.base.wrapping_add(load.vaddr as usize);
        address >= start && address - start < load.memsz as usize
    }))?;
    Some(super::libdl::DlInfo {
        filename: module.name.as_ptr(), base: module.base as *mut c_void,
        symbol_name: std::ptr::null(), symbol_address: std::ptr::null_mut(),
    })
}

/// The guest cross-module layer: previously loaded exec images, newest
/// first (reference `so_module` head-insertion order).
fn guest_chain_lookup(name: &str) -> Option<usize> {
    let chain = guest_image_chain();
    let found = chain.iter().rev().find_map(|module| module.symbol(name));
    found
}

/// Reference `so_resolve_link` for one undefined import: the `glad_`
/// rewritten name through every layer first, then the exact name.
/// `globals` is the in-process explicit layer of [`ExecutableModule::map`]
/// (empty on the production C path), consulted ahead of the sources.
fn resolve_link(name: &str, version: Option<SymbolVersion<'_>>, globals: GlobalSymbols<'_>) -> Option<usize> {
    if version.is_some() {
        return host_symbol_lookup_versioned(name, version);
    }
    let exact = |name: &str| {
        globals
            .get(name)
            .copied()
            .or_else(|| host_symbol_lookup(name))
            .or_else(|| guest_chain_lookup(name))
    };
    if let Some(rewritten) = glad_rewrite(name) {
        if let Some(address) = exact(&rewritten) {
            return Some(address);
        }
    }
    exact(name)
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
    let shared_pages = view.loads.iter().enumerate().any(|(index, a)| {
        a.memsz != 0 && view.loads[index + 1..].iter().any(|b| {
            b.memsz != 0
                && align_down(a.vaddr as usize, page) < align_up((b.vaddr + b.memsz) as usize, page)
                && align_down(b.vaddr as usize, page) < align_up((a.vaddr + a.memsz) as usize, page)
        })
    });
    let full = copied_loads || shared_pages || view.loads.iter().any(|load| {
        load.flags & PF_X != 0 && writes.intersects(load.vaddr, load.vaddr + load.memsz)
    });
    for load in &view.loads {
        if load.flags & PF_X == 0 || load.filesz == 0 {
            continue;
        }
        let mut start = load.vaddr as usize;
        let mut end = (start + load.filesz as usize).min(map_len);
        if !full {
            if end % page == 0 { continue; }
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

/// Run the guest's `.init_array` constructors (reference `so_initialize`):
/// relocations have already filled every entry, so each non-null word is an
/// absolute `void (*)(void)` host address inside our mapping. A crashing
/// constructor kills the process — the guest is the buggy party, exactly as
/// in the reference.
///
/// SAFETY: callers must have run [`clear_executable_segments`] first.
unsafe fn run_init_array(view: &ElfView, base: usize) {
    let Some((vaddr, size)) = view.init_array else {
        return;
    };
    for index in 0..(size as usize / 8) {
        let slot = base + vaddr as usize + index * 8;
        // SAFETY: the range was parse-validated as file-backed and is fully
        // mapped; the pointer is relocation-final.
        let entry = unsafe { std::ptr::read_unaligned(slot as *const usize) };
        if entry != 0 {
            // SAFETY: guest constructor code on RX pages of our mapping.
            let ctor: unsafe extern "C" fn() = unsafe { std::mem::transmute(entry) };
            // SAFETY: an ELF init_array entry takes no arguments (C ABI).
            unsafe { ctor() };
        }
    }
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
    pub unsafe fn map(path: impl AsRef<Path>, globals: GlobalSymbols<'_>) -> Result<Self, LoadError> {
        // SAFETY: the caller owns the file/code/import lifetime contract.
        let module = unsafe { Self::map_with_layers(path.as_ref(), globals) }?;
        // SAFETY: the image is mapped, relocated and protected.
        unsafe { run_init_array(&module.view, module.base) };
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
        unsafe { run_init_array(&module.view, module.base) };
        Ok(ptr)
    }

    // Safety: stable backing file, trusted code/imports, quiescence at Drop.
    unsafe fn map_with_layers(path: &Path, globals: GlobalSymbols<'_>) -> Result<Self, LoadError> {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| LoadError::Unsupported("ELF path contains NUL"))?;
        let file = std::fs::File::open(path).map_err(LoadError::Io)?;
        let noexec = rudroid_common::filesystem::FilesystemStats::for_file(&file)
            .map_err(LoadError::Io)?.noexec;
        let file_len = usize::try_from(file.metadata().map_err(LoadError::Io)?.len())
            .ok().filter(|len| *len <= isize::MAX as usize)
            .ok_or(LoadError::Unsupported("ELF file exceeds slice address range"))?;
        let page = page_size();
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
            return Err(LoadError::Mapping(std::io::Error::last_os_error().to_string()));
        }
        let meta = meta as *mut u8;
        // SAFETY: bytes [0, file_len) are mapped readable above.
        let meta_bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(meta as *const u8, file_len) };

        let result = (|| -> Result<(ElfView<'static>, *mut u8, usize, usize, usize, u16), LoadError> {
            let view = ElfView::parse(meta_bytes)?;
            let phoff = u64::from_le_bytes(view.bytes[32..40].try_into().unwrap()) as usize;
            let stride = u16::from_le_bytes(view.bytes[54..56].try_into().unwrap()) as usize;
            let phnum = u16::from_le_bytes(view.bytes[56..58].try_into().unwrap());
            if stride != std::mem::size_of::<libc::Elf64_Phdr>()
                || phoff % std::mem::align_of::<libc::Elf64_Phdr>() != 0
            {
                return Err(LoadError::Unsupported("unaligned or nonstandard ELF64 program-header stride"));
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

            let segment_result = (|| -> Result<(), LoadError> {
                let mut copied_loads = noexec;
                // File-backed mappings per PT_LOAD (offset must be
                // page-aligned; ELF guarantees p_vaddr ≡ p_offset mod page,
                // verified below). BSS tails stay anonymous.
                for load in &view.loads {
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
                    // noexec files may be read but their mappings cannot gain
                    // execute permission. Keep the entire reserved image
                    // anonymous in this case, including overlapping read-only
                    // pages, then use the same relocation/W^X/cache-flush path.
                    if !congruent || noexec {
                        copied_loads = true;
                        let src = view.vaddr_slice(load.vaddr, load.filesz)?;
                        // SAFETY: vaddr + filesz <= span <= map_len.
                        let dst = unsafe { map.add(vaddr) };
                        // SAFETY: disjoint ranges inside our own mappings.
                        unsafe {
                            std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len())
                        };
                        continue;
                    }
                    let off_aligned = align_down(off, page);
                    let delta = off - off_aligned;
                    let start = base + vaddr - delta;
                    let len = align_up(load.filesz as usize + delta, page);
                    if start < base || start + len > base + map_len {
                        return Err(LoadError::Mapping(
                            "file-backed segment mapping exceeds the reserved span"
                                .to_owned(),
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
                }
                // Relocations run while every page is writable. Defined
                // symbols bind to this module inside `relocations`;
                // undefined imports take the `resolve_link` order (ticket
                // 86, reference `so_resolve_link`), then weak → 0.
                let writes = relocations::apply(&view, base, map, span, &|name, version| resolve_link(name, version, globals))?;
                // W^X, two-pass: read/execute first, then writable — two
                // segments sharing one page (rare layouts) must never end
                // up RWX (ticket 49; ticket 85 file-backed pages honor the
                // same invariant, COW writes stay private).
                for load in &view.loads {
                    if load.flags & PF_W != 0 && load.flags & PF_X == 0 {
                        continue;
                    }
                    Self::protect_segment(map, map_len, page, load, false)?;
                }
                for load in &view.loads {
                    if load.flags & PF_W == 0 || load.flags & PF_X != 0 {
                        continue;
                    }
                    Self::protect_segment(map, map_len, page, load, true)?;
                }
                // Maintain all possibly written code before publication;
                // pristine file code remains cold until its first use.
                clear_executable_segments(&view, base, map_len, page, copied_loads, writes);
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
                    let _ = unsafe {
                        libc::madvise(meta.cast(), meta_len, libc::MADV_DONTNEED)
                    };
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
                })
            },
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
            addr: self.base, name: self.name.as_ptr(), phdr: self.phdr, phnum: self.phnum,
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
            if exec.flags & PF_X == 0 {
                continue;
            }
            let x_start = align_down(exec.vaddr as usize, page);
            let x_end = align_up((exec.vaddr + exec.memsz) as usize, page);
            for write in &view.loads {
                if write.flags & PF_W == 0 {
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

    /// Look a dynamic symbol up by name; absolute host address. Shares the
    /// export-eligibility rules with the buffer backend (ticket 62); a TLS
    /// export has no address and answers `None` (ticket 86).
    pub fn symbol(&self, name: &str) -> Option<usize> {
        self.symbol_lookup(name).ok().flatten()
    }

    /// [`Self::symbol`] keeping the reason a present export is refused.
    fn symbol_lookup(&self, name: &str) -> Result<Option<usize>, super::ExportReject> {
        super::exported_symbol_value(&self.view, name)
            .map(|value| value.map(|value| self.base.wrapping_add(value as usize)))
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
        for finalizer in self.cleanup_sources.iter().filter_map(|source| source.finalizer) {
            // SAFETY: registrant finalizer for the still-mapped range (register contract).
            unsafe { finalizer(self.map as usize, self.map_len); }
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

/// # Safety
/// `path` must be NUL-terminated; returns null on failure (reason on the
/// `rudroid_elf_last_error` channel, ticket 65).
/// The file/code/imports follow `ExecutableModule::map`'s contract, including
/// an unchanged backing file and guest quiescence before eventual destruction.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_exec_load(path: *const c_char) -> *mut ExecutableModule {
    use std::ffi::CStr;
    super::clear_last_error();
    super::EXEC_LOAD_CALLS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    if path.is_null() {
        super::set_last_error(&format_args!("rudroid_elf_exec_load: path is null"));
        return std::ptr::null_mut();
    }
    // SAFETY: non-null `path` is a NUL-terminated string (caller contract).
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        super::set_last_error(&format_args!(
            "rudroid_elf_exec_load: path is not valid UTF-8"
        ));
        return std::ptr::null_mut();
    };
    let globals = HashMap::new();
    // SAFETY: the exec-load caller supplies trusted code and a stable file.
    match unsafe { ExecutableModule::map_and_register(Path::new(path), &globals) } {
        Ok(module) => module,
        Err(error) => {
            super::set_last_error(&format_args!("cannot map guest module {path}: {error}"));
            std::ptr::null_mut()
        }
    }
}

/// Image base (load bias) of an exec-mapped module (ticket 84): the guest
/// event base and every address derived through `rudroid_elf_exec_symbol`
/// live in this one image, so identity is single-sourced.
///
/// # Safety
/// `handle` must come from `rudroid_elf_exec_load`; null is diagnosed on
/// the error channel (ticket 65).
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_exec_load_base(handle: *const ExecutableModule) -> usize {
    super::clear_last_error();
    match handle.as_ref() {
        Some(handle) => handle.base(),
        None => {
            super::set_last_error(&format_args!("rudroid_elf_exec_load_base: handle is null"));
            0
        }
    }
}

/// # Safety
/// `handle` must come from `rudroid_elf_exec_load`; null arguments are
/// diagnosed on the error channel (ticket 65).
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_exec_symbol(
    handle: *const ExecutableModule,
    name: *const c_char,
) -> usize {
    use std::ffi::CStr;
    super::clear_last_error();
    let Some(handle) = handle.as_ref() else {
        super::set_last_error(&format_args!("rudroid_elf_exec_symbol: handle is null"));
        return 0;
    };
    if name.is_null() {
        super::set_last_error(&format_args!("rudroid_elf_exec_symbol: name is null"));
        return 0;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        super::set_last_error(&format_args!("rudroid_elf_exec_symbol: name is not valid UTF-8"));
        return 0;
    };
    match handle.symbol_lookup(name) {
        Ok(address) => address.unwrap_or(0),
        Err(reject) => {
            super::set_last_error(&format_args!(
                "rudroid_elf_exec_symbol: {}",
                reject.describe(name)
            ));
            0
        }
    }
}

/// Reference `so_symbol_all` over an exec-mapped module: dynsym first,
/// then the file's `.symtab` (see [`ExecutableModule::symbol_all`]).
/// Returns an absolute address in the same image as
/// `rudroid_elf_exec_symbol`/`rudroid_elf_exec_load_base`; 0 on a miss
/// (no error recorded) or on a refused TLS match (reason on the error
/// channel, ticket 65).
///
/// # Safety
/// `handle` must come from `rudroid_elf_exec_load`; null arguments are
/// diagnosed on the error channel (ticket 65).
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_exec_symbol_all(
    handle: *const ExecutableModule,
    name: *const c_char,
) -> usize {
    use std::ffi::CStr;
    super::clear_last_error();
    let Some(handle) = handle.as_ref() else {
        super::set_last_error(&format_args!("rudroid_elf_exec_symbol_all: handle is null"));
        return 0;
    };
    if name.is_null() {
        super::set_last_error(&format_args!("rudroid_elf_exec_symbol_all: name is null"));
        return 0;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        super::set_last_error(&format_args!(
            "rudroid_elf_exec_symbol_all: name is not valid UTF-8"
        ));
        return 0;
    };
    match handle.symbol_all(name) {
        Ok(address) => address.unwrap_or(0),
        Err(reject) => {
            super::set_last_error(&format_args!(
                "rudroid_elf_exec_symbol_all: {}",
                reject.describe(name)
            ));
            0
        }
    }
}

/// # Safety
/// `handle` must come from `rudroid_elf_exec_load` and not be freed twice;
/// null is a no-op (diagnosed on the error channel, ticket 65).
/// No guest code or direct handle operation may still use it.
/// Guest threads/TLS destructors and concurrent finalizers must have
/// completed/joined; current-thread callbacks drain before unmapping.
/// Registry lookup snapshots retain the image until their queries finish; returned
/// symbol addresses themselves do not carry a lifetime lease.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_exec_destroy(handle: *mut ExecutableModule) {
    super::clear_last_error();
    if !handle.is_null() {
        // Withdraw the root ownership. Open libdl references keep this same
        // image published until their last close; in-flight snapshots pin it
        // after withdrawal. No finalizer runs under the registry lock.
        let mut chain = image_chain_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(module) = chain.iter().find(|module| Arc::as_ptr(module) == handle) {
            module.root_destroyed.store(true, Ordering::Relaxed);
            if module.dl_references.load(Ordering::Relaxed) == 0 {
                Arc::make_mut(&mut chain).retain(|module| Arc::as_ptr(module) != handle);
            }
        }
        drop(chain);
        // SAFETY: the caller hands back the root reference from `Arc::into_raw`, once.
        drop(unsafe { Arc::from_raw(handle) });
    } else {
        super::set_last_error(&format_args!("rudroid_elf_exec_destroy: handle is null"));
    }
}

/// Register a host/thunk symbol source (ticket 86). Sources answer a
/// module's UNDEFINED imports, in registration order, before the guest
/// image chain (reference: host symbol tables first), with the `glad_`
/// rewritten name tried before the exact one. The
/// registrant keeps `callback`/`user` valid for the process lifetime
/// (thunk libraries are pinned like every kit member); `name` is a static
/// diagnostic label. Returns a non-negative source id, or -1 on null
/// callback / failed reservation.
///
/// # Safety
/// `callback` must follow the documented contract (return 0 on miss, never
/// retain the borrowed name, permit concurrent queries); `user` is opaque
/// registrant state valid for the process lifetime, including after revoke.
/// Callbacks run outside the source lock and may register/revoke/resolve;
/// changes affect later lookups, not the current query's snapshot.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_symbol_source_register(
    name: *const c_char,
    callback: unsafe extern "C" fn(name: *const c_char, user: *mut c_void) -> usize,
    user: *mut c_void,
) -> c_int {
    // SAFETY: forwards this function's own contract.
    unsafe { register_source(name, SourceCallback::NameOnly(callback), user, None) }
}

/// Register a source that explicitly accepts the full name/library/version
/// requirement. It also receives unversioned queries (two null strings).
/// Name-only sources and guest images are never used as version fallbacks.
///
/// # Safety
/// Non-null strings must be NUL-terminated and valid for this call. The
/// callback/user pair must remain process-live, support concurrent/reentrant
/// calls, retain none of the borrowed query strings, and return 0 for every
/// unsupported ABI tuple. Revocation affects subsequent queries only.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_symbol_source_register_versioned(
    name: *const c_char,
    callback: Option<VersionedSymbolSource>,
    user: *mut c_void,
) -> c_int {
    super::clear_last_error();
    let Some(callback) = callback else {
        super::set_last_error(&format_args!("version-aware symbol source: null callback"));
        return -1;
    };
    // SAFETY: forwards this function's own contract.
    unsafe { register_source(name, SourceCallback::Versioned(callback), user, None) }
}

/// Version-aware source with an explicitly typed pre-unmap cleanup callback.
///
/// # Safety
/// Same process-pinned/concurrent/reentrant source contract as above. The
/// finalizer must remain valid after revocation, accept a still-live image
/// range, retain no image pointer, and run without unwinding. Destroyed
/// images must be quiescent (other guest threads/finalizers joined).
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_symbol_source_register_finalized(
    name: *const c_char,
    callback: Option<VersionedSymbolSource>,
    user: *mut c_void,
    finalizer: Option<RangeFinalizer>,
) -> c_int {
    super::clear_last_error();
    let (Some(callback), Some(finalizer)) = (callback, finalizer) else {
        super::set_last_error(&format_args!("finalized symbol source: null callback or finalizer"));
        return -1;
    };
    // SAFETY: forwards this function's own contract.
    unsafe { register_source(name, SourceCallback::Versioned(callback), user, Some(finalizer)) }
}

unsafe fn register_source(name: *const c_char, callback: SourceCallback, user: *mut c_void, finalizer: Option<RangeFinalizer>) -> c_int {
    use std::ffi::CStr;
    super::clear_last_error();
    if name.is_null() {
        super::set_last_error(&format_args!(
            "rudroid_elf_symbol_source_register: null name or callback"
        ));
        return -1;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(label) = unsafe { CStr::from_ptr(name) }.to_str() else {
        super::set_last_error(&format_args!(
            "rudroid_elf_symbol_source_register: name is not valid UTF-8"
        ));
        return -1;
    };
    // The label is validated (UTF-8, non-null) but not retained: sources are
    // identified by the returned id alone.
    let _ = label;
    let Ok(id) = NEXT_SOURCE_ID.fetch_update(
        std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire,
        |id| (id <= c_int::MAX as usize).then_some(id + 1),
    ) else {
        super::set_last_error(&format_args!("ELF symbol source id exhausted"));
        return -1;
    };
    Arc::make_mut(&mut symbol_sources()).push(SymbolSource {
        id,
        callback,
        user,
        finalizer,
    });
    id as c_int
}

/// Test/diagnostic surface (test-fixtures feature only): clear the guest
/// image chain. Images themselves stay pinned (ticket 46) — this only
/// withdraws their lookup identity, letting each test start from an empty
/// chain.
///
/// # Safety
/// Never call while guest code from a chained image may run.
#[cfg(all(target_os = "linux", feature = "test-fixtures"))]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_image_chain_reset() {
    let mut chain = image_chain_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::make_mut(&mut chain).clear();
}

/// Revoke a registered source (ticket 86: a failed supply must be
/// revocable). Unknown ids return -1; the id is never reused.
///
/// # Safety
/// `id` must come from either symbol-source registration export.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_symbol_source_unregister(id: c_int) -> c_int {
    super::clear_last_error();
    if id < 0 {
        return -1;
    }
    let id = id as usize;
    let mut sources = symbol_sources();
    let before = sources.len();
    Arc::make_mut(&mut sources).retain(|source| source.id != id);
    if sources.len() == before {
        return -1;
    }
    0
}

#[cfg(test)]
#[path = "../tests/unit/exec_lookup_tests.rs"]
mod lookup_tests;
