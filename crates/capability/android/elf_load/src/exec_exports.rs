//! INPUT: loader C requests and process-pinned mapped images
//! OUTPUT: checked load, lookup and source registration ABI
//! POS: ELF execution capability C exports

use super::*;

/// # Safety
/// `path` must be NUL-terminated; returns null on failure (reason on the
/// `rudroid_elf_last_error` channel, ticket 65).
/// The file/code/imports follow `ExecutableModule::map`'s contract, including
/// an unchanged backing file and guest quiescence before eventual destruction.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_elf_exec_load(path: *const c_char) -> *mut ExecutableModule {
    use std::ffi::CStr;
    crate::clear_last_error();
    crate::EXEC_LOAD_CALLS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    if path.is_null() {
        crate::set_last_error(&format_args!("rudroid_elf_exec_load: path is null"));
        return std::ptr::null_mut();
    }
    // SAFETY: non-null `path` is a NUL-terminated string (caller contract).
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        crate::set_last_error(&format_args!(
            "rudroid_elf_exec_load: path is not valid UTF-8"
        ));
        return std::ptr::null_mut();
    };
    let globals = HashMap::new();
    // SAFETY: the exec-load caller supplies trusted code and a stable file.
    match unsafe { ExecutableModule::map_and_register(Path::new(path), &globals) } {
        Ok(module) => module,
        Err(error) => {
            crate::set_last_error(&format_args!("cannot map guest module {path}: {error}"));
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
    crate::clear_last_error();
    match handle.as_ref() {
        Some(handle) => handle.base(),
        None => {
            crate::set_last_error(&format_args!("rudroid_elf_exec_load_base: handle is null"));
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
    crate::clear_last_error();
    let Some(handle) = handle.as_ref() else {
        crate::set_last_error(&format_args!("rudroid_elf_exec_symbol: handle is null"));
        return 0;
    };
    if name.is_null() {
        crate::set_last_error(&format_args!("rudroid_elf_exec_symbol: name is null"));
        return 0;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        crate::set_last_error(&format_args!(
            "rudroid_elf_exec_symbol: name is not valid UTF-8"
        ));
        return 0;
    };
    match handle.symbol_lookup(name) {
        Ok(address) => address.unwrap_or(0),
        Err(reject) => {
            crate::set_last_error(&format_args!(
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
    crate::clear_last_error();
    let Some(handle) = handle.as_ref() else {
        crate::set_last_error(&format_args!("rudroid_elf_exec_symbol_all: handle is null"));
        return 0;
    };
    if name.is_null() {
        crate::set_last_error(&format_args!("rudroid_elf_exec_symbol_all: name is null"));
        return 0;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        crate::set_last_error(&format_args!(
            "rudroid_elf_exec_symbol_all: name is not valid UTF-8"
        ));
        return 0;
    };
    match handle.symbol_all(name) {
        Ok(address) => address.unwrap_or(0),
        Err(reject) => {
            crate::set_last_error(&format_args!(
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
    crate::clear_last_error();
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
        crate::set_last_error(&format_args!("rudroid_elf_exec_destroy: handle is null"));
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
    callback: Option<unsafe extern "C" fn(*const c_char, *mut c_void) -> usize>,
    user: *mut c_void,
) -> c_int {
    let Some(callback) = callback else {
        crate::set_last_error(&format_args!("symbol source: null callback"));
        return -1;
    };
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
    crate::clear_last_error();
    let Some(callback) = callback else {
        crate::set_last_error(&format_args!("version-aware symbol source: null callback"));
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
    crate::clear_last_error();
    let (Some(callback), Some(finalizer)) = (callback, finalizer) else {
        crate::set_last_error(&format_args!(
            "finalized symbol source: null callback or finalizer"
        ));
        return -1;
    };
    // SAFETY: forwards this function's own contract.
    unsafe {
        register_source(
            name,
            SourceCallback::Versioned(callback),
            user,
            Some(finalizer),
        )
    }
}

pub(super) unsafe fn register_source(
    name: *const c_char,
    callback: SourceCallback,
    user: *mut c_void,
    finalizer: Option<RangeFinalizer>,
) -> c_int {
    use std::ffi::CStr;
    crate::clear_last_error();
    if name.is_null() {
        crate::set_last_error(&format_args!(
            "rudroid_elf_symbol_source_register: null name or callback"
        ));
        return -1;
    }
    // SAFETY: non-null `name` is a NUL-terminated string (caller contract).
    let Ok(label) = unsafe { CStr::from_ptr(name) }.to_str() else {
        crate::set_last_error(&format_args!(
            "rudroid_elf_symbol_source_register: name is not valid UTF-8"
        ));
        return -1;
    };
    // The label is validated (UTF-8, non-null) but not retained: sources are
    // identified by the returned id alone.
    let _ = label;
    let Ok(id) = NEXT_SOURCE_ID.fetch_update(
        std::sync::atomic::Ordering::AcqRel,
        std::sync::atomic::Ordering::Acquire,
        |id| (id <= c_int::MAX as usize).then_some(id + 1),
    ) else {
        crate::set_last_error(&format_args!("ELF symbol source id exhausted"));
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
    crate::clear_last_error();
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
