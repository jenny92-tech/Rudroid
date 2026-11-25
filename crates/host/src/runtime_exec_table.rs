//! INPUT: pinned ELF lookup callbacks and module mappings
//! OUTPUT: export resolution by guest load base
//! POS: loader executable image registry

use super::*;

/// Guest exec-module table (ticket 53): the load base a module-loaded event
/// reports → the exec-mapping handle guest bring-up created for the same
/// file, plus the mapping capability's `exec_symbol` entry. The Host API
/// `so_symbol` slot resolves profile lifecycle dispatches through this
/// table; entries are process-pinned like every live guest mapping
/// (ticket 46 — handles are never destroyed).
struct GuestExecTable {
    exec_symbol: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    /// The capability's `so_symbol_all` analogue (dynsym, then `.symtab`);
    /// optional — a kit member without it leaves the slot answering 0.
    exec_symbol_all: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    /// (load base, exec handle) per mapped kinded guest module.
    modules: Vec<(usize, *mut c_void)>,
}

// SAFETY (per field, ticket 70 style): `exec_symbol` is a C function pointer
// into the pinned elf_load capability; the handles are opaque mapping
// pointers the host never dereferences — they only flow back into the
// capability's own `exec_symbol`. All table access is serialized by the
// mutex; `Send` is what the process-global OnceLock<Mutex<…>> needs.
unsafe impl Send for GuestExecTable {}

static GUEST_EXEC: OnceLock<Mutex<GuestExecTable>> = OnceLock::new();

fn guest_exec() -> &'static Mutex<GuestExecTable> {
    GUEST_EXEC.get_or_init(|| {
        Mutex::new(GuestExecTable {
            exec_symbol: None,
            exec_symbol_all: None,
            modules: Vec::new(),
        })
    })
}

/// Register the exec backend's symbol-lookup entry (once per bring-up; the
/// C API entry lives in the pinned android_elf_load capability).
/// # Safety
/// The backend follows the ELF symbol-query ABI, accepts registered handles
/// and remains callable for the process.
pub unsafe fn register_guest_exec_backend(
    exec_symbol: unsafe extern "C" fn(*const c_void, *const c_char) -> usize,
) {
    let mut table = guest_exec().lock().unwrap();
    if table.exec_symbol.is_none() {
        table.exec_symbol = Some(exec_symbol);
    }
}

/// Register the exec backend's `.symtab`-capable lookup entry
/// (`rudroid_elf_exec_symbol_all`, reference `so_symbol_all`).
/// # Safety
/// As `register_guest_exec_backend`, for the all-symbols query ABI.
pub unsafe fn register_guest_exec_symbol_all(
    exec_symbol_all: unsafe extern "C" fn(*const c_void, *const c_char) -> usize,
) {
    let mut table = guest_exec().lock().unwrap();
    if table.exec_symbol_all.is_none() {
        table.exec_symbol_all = Some(exec_symbol_all);
    }
}

/// Register one mapped kinded guest module: its reported load base and the
/// exec mapping handle created from the same file.
/// # Safety
/// `exec_handle` belongs to the installed backend, reports `base` and remains
/// live for the process, including queries already in flight when withdrawn.
pub unsafe fn register_guest_exec_module(base: usize, exec_handle: *mut c_void) {
    let mut table = guest_exec().lock().unwrap();
    table.modules.push((base, exec_handle));
}

/// Registered exec image bases (ticket 84 test/diagnostic surface): lets
/// integration tests assert exactly which images survived a bring-up
/// without guessing at addresses.
pub fn guest_exec_bases() -> Vec<usize> {
    let table = guest_exec().lock().unwrap();
    table.modules.iter().map(|(base, _)| *base).collect()
}

/// Ticket 84: drop one base→handle binding after a failed bring-up step
/// (JNI_OnLoad failure) so nothing new can bind to a module whose
/// initialization failed. The mapping itself stays pinned for the process
/// (ownership contract) — this only withdraws the lookup identity.
pub fn unregister_guest_exec_module(base: usize) {
    let mut table = guest_exec().lock().unwrap();
    table
        .modules
        .retain(|(module_base, _)| *module_base != base);
}

/// Resolve a guest module export by load base and name (the `so_symbol`
/// contract: 0 on any miss). `None` when the base is unknown, the exec
/// backend is absent, or the symbol is not an export.
pub fn guest_exec_symbol(base: usize, name: &str) -> Option<usize> {
    let name = CString::new(name.replace('\0', "\\0")).ok()?;
    lookup_guest_symbol(base, &name, false)
}

pub fn guest_exec_symbol_all(base: usize, name: &str) -> Option<usize> {
    let name = CString::new(name.replace('\0', "\\0")).ok()?;
    lookup_guest_symbol(base, &name, true)
}

pub(crate) fn lookup_guest_symbol(base: usize, name: &std::ffi::CStr, all: bool) -> Option<usize> {
    let (lookup, handle) = {
        let table = guest_exec().lock().unwrap();
        let lookup = if all {
            table.exec_symbol_all
        } else {
            table.exec_symbol
        }?;
        let handle = table.modules.iter().find(|(module, _)| *module == base)?.1;
        (lookup, handle)
    };
    // SAFETY: the mapping and lookup backend remain pinned after unlocking.
    let address = unsafe { lookup(handle, name.as_ptr()) };
    (address != 0).then_some(address)
}

pub(super) fn clear() {
    let mut table = guest_exec().lock().unwrap();
    table.modules.clear();
    table.exec_symbol = None;
    table.exec_symbol_all = None;
}
