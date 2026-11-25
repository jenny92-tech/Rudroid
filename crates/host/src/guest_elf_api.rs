//! INPUT: pinned ELF capability library
//! OUTPUT: resolved C function table and error diagnostic
//! POS: guest image loading ABI adapter

use super::*;

/// C API of the android_elf_load capability, resolved once per bring-up.
///
/// Ownership (ticket 46): mapped guest modules stay mapped for the process
/// lifetime — guest code may already have been entered by the time this
/// struct is dropped, so `Drop` intentionally never calls the capability's
/// destroy entry. The loader pins the owning capability library for the
/// process life, so the stored function pointers can never dangle.
pub(super) struct ElfLoadApi {
    pub(super) from_path: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    /// Image base / load bias of a mapped module (ticket 48): module events
    /// carry this, never the opaque handle address.
    pub(super) load_base: unsafe extern "C" fn(*const c_void) -> usize,
    pub(super) symbol: unsafe extern "C" fn(*const c_void, *const c_char) -> usize,
    // Used by the hook/JNI_OnLoad wiring on the guest bring-up path
    // (crates/docs/ACCEPTANCE.md); not exercised by every test build yet.
    #[allow(dead_code)]
    pub(super) destroy: unsafe extern "C" fn(*mut c_void),
    /// Ticket 65 error channel (`rudroid_elf_last_error`), required since
    /// ticket 72. Kits are same-build only (ticket 57): this library already
    /// passed the loader's `runtime_build_id` check, so a member without the
    /// channel cannot be a legitimate deployment — resolving it like every
    /// other C API entry keeps ticket 65 diagnostics from silently degrading
    /// exactly on the broken kits where they matter most.
    pub(super) last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
    /// Ticket 49 executable backend (Linux only), resolved tolerantly: the
    /// C exports are `#[cfg(target_os = "linux")]`, so non-Linux builds of
    /// the same capability legitimately lack them. `None` + a module that
    /// exports `JNI_OnLoad` is a hard bring-up error, never a silent skip.
    pub(super) exec_load: Option<unsafe extern "C" fn(*const c_char) -> *mut c_void>,
    pub(super) exec_symbol: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    /// Reference `so_symbol_all` (dynsym, then the module file's `.symtab`),
    /// resolved tolerantly; backs the Host API `so_symbol_all` slot.
    pub(super) exec_symbol_all: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    /// Ticket 84: image base of an exec mapping — the single source of the
    /// guest address identity (event base == execution base).
    pub(super) exec_load_base: Option<unsafe extern "C" fn(*const c_void) -> usize>,
}

pub(super) unsafe fn resolve_api(library: &Library) -> Result<ElfLoadApi, GuestError> {
    let from_path: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_void> = library
        .get(symbols::elf_load::LOAD_FROM_PATH.as_bytes())
        .map_err(|e| GuestError::Symbol {
            capability: "android_elf_load",
            reason: e.to_string(),
        })?;
    let load_base: libloading::Symbol<unsafe extern "C" fn(*const c_void) -> usize> = library
        .get(symbols::elf_load::LOAD_BASE.as_bytes())
        .map_err(|e| GuestError::Symbol {
            capability: "android_elf_load",
            reason: e.to_string(),
        })?;
    let symbol: libloading::Symbol<unsafe extern "C" fn(*const c_void, *const c_char) -> usize> =
        library
            .get(symbols::elf_load::SYMBOL.as_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: "android_elf_load",
                reason: e.to_string(),
            })?;
    let destroy: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> = library
        .get(symbols::elf_load::DESTROY.as_bytes())
        .map_err(|e| GuestError::Symbol {
            capability: "android_elf_load",
            reason: e.to_string(),
        })?;
    // Ticket 65 channel, required since ticket 72: the same-build rule
    // (ticket 57) means this library's descriptor already passed
    // `check_same_build` before bring-up, so a missing channel proves a
    // foreign/stale kit member. It fails on the same `GuestError::Symbol`
    // channel as the other C API entries instead of degrading bring-up to
    // the generic load-failure reason (the R2 Standards-4 silent-degrade
    // gap).
    let last_error: libloading::Symbol<unsafe extern "C" fn(*mut c_char, usize) -> usize> = library
        .get(symbols::elf_load::LAST_ERROR.as_bytes())
        .map_err(|e| GuestError::Symbol {
            capability: "android_elf_load",
            reason: e.to_string(),
        })?;
    // Ticket 49 exec backend, resolved tolerantly: the C exports are
    // Linux-only in the capability, so a missing pair is a legitimate
    // build there — but a guest module that exports JNI_OnLoad then fails
    // bring-up with `JniExecBackendMissing` instead of skipping silently.
    let exec_load: Option<libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_void>> =
        library.get(symbols::elf_exec::EXEC_LOAD.as_bytes()).ok();
    let exec_symbol: Option<
        libloading::Symbol<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    > = library.get(symbols::elf_exec::EXEC_SYMBOL.as_bytes()).ok();
    // Ticket 84, resolved tolerantly like the other exec exports (the same
    // Linux-only cfg lives in the capability): a kit member without it
    // predates the single-image contract and cannot report an exec base.
    let exec_load_base: Option<libloading::Symbol<unsafe extern "C" fn(*const c_void) -> usize>> =
        library
            .get(symbols::elf_exec::EXEC_LOAD_BASE.as_bytes())
            .ok();
    let exec_symbol_all: Option<
        libloading::Symbol<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    > = library
        .get(symbols::elf_exec::EXEC_SYMBOL_ALL.as_bytes())
        .ok();
    Ok(ElfLoadApi {
        from_path: *from_path,
        load_base: *load_base,
        symbol: *symbol,
        destroy: *destroy,
        exec_load_base: exec_load_base.map(|s| *s),

        last_error: *last_error,
        exec_load: exec_load.map(|s| *s),
        exec_symbol: exec_symbol.map(|s| *s),
        exec_symbol_all: exec_symbol_all.map(|s| *s),
    })
}

/// Fetch the capability's thread-local diagnostic for the just-failed call
/// (ticket 65). Two-phase: query the needed length, then fetch exactly that.
/// SAFETY: `last_error` points into the pinned capability library.
pub(super) unsafe fn fetch_last_error(api: &ElfLoadApi) -> Option<String> {
    // SAFETY: the pinned provider implements the length-query/copy contract.
    unsafe { rudroid_common::native_error::read(api.last_error) }
}
