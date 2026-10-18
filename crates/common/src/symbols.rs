//! INPUT:  (none)
//! OUTPUT:  capability C export names, source callbacks and metadata import visitor ABI
//! POS:    pure same-kit contracts shared without linking capability implementations
//! Exact C symbol names resolved with `dlsym`/`libloading` on every capability
//! dynamic library. These mirror the C++ reference `plugin_api.h` /
//! `plugin_manifest.cpp` entry points so both kits stay symbol-compatible.

/// Returns `const RudroidPluginDescriptor*` (C view of [`crate::descriptor`]).
pub const DESCRIPTOR: &str = "rudroid_plugin_descriptor";

/// Optional early phase, runs before any other capability is initialized.
pub const EARLY_INIT: &str = "rudroid_plugin_early_init";

/// Optional JNI registration phase, runs after the guest VM is up.
pub const REGISTER_JNI: &str = "rudroid_plugin_register_jni";

/// Optional main init phase, runs after JNI registration.
pub const INIT: &str = "rudroid_plugin_init";

/// All phase entry points a capability may export, in invocation order.
pub const PHASE_SYMBOLS: [&str; 3] = [EARLY_INIT, REGISTER_JNI, INIT];

/// Reply Catalog cdylib C API symbols (ticket 04). The catalog is loaded with
/// `libloading`/`dlsym` like any other kit member; these names are the
/// contract between the loader/jni_vm and the catalog library.
pub mod reply_catalog {
    /// `struct RudroidReplyCatalog* rudroid_reply_catalog_from_bytes(const uint8_t*, uintptr_t)`
    pub const FROM_BYTES: &str = "rudroid_reply_catalog_from_bytes";
    /// `struct RudroidReplyCatalog* rudroid_reply_catalog_from_path(const char*)`
    pub const FROM_PATH: &str = "rudroid_reply_catalog_from_path";
    /// `int rudroid_reply_catalog_query_int(handle, class, method, sig, int64_t* out)` — 1 on hit
    pub const QUERY_INT: &str = "rudroid_reply_catalog_query_int";
    /// `int rudroid_reply_catalog_query_bool(handle, class, method, sig, int32_t* out)` — 1 on hit
    pub const QUERY_BOOL: &str = "rudroid_reply_catalog_query_bool";
    /// `void rudroid_reply_catalog_destroy(handle)`
    pub const DESTROY: &str = "rudroid_reply_catalog_destroy";

    /// All catalog symbols a consumer may dlsym.
    pub const ALL: [&str; 5] = [FROM_BYTES, FROM_PATH, QUERY_INT, QUERY_BOOL, DESTROY];
}

/// Guest ELF load capability C API (ticket 12). The loader/jni_vm dlsym these
/// on `android_elf_load` (or call the Rust API directly when linked in-tree).
pub mod elf_load {
    /// Borrowed NUL-terminated name/library/version; the latter pair is null
    /// for bare imports. Flags: bit 0 weak, bit 1 STT_TLS. No guest execution.
    pub type ImportVisitor = unsafe extern "C" fn(
        *const std::ffi::c_char, *const std::ffi::c_char,
        *const std::ffi::c_char, u32, *mut std::ffi::c_void,
    );
    /// `int rudroid_elf_visit_imports(path, visitor, user)` — metadata only;
    /// 0 success, -1 failure on LAST_ERROR, no mapping/relocation/constructors.
    pub const VISIT_IMPORTS: &str = "rudroid_elf_visit_imports";
    /// `struct RudroidGuestModule* rudroid_elf_load_from_path(const char*)`
    pub const LOAD_FROM_PATH: &str = "rudroid_elf_load_from_path";
    /// `struct RudroidGuestModule* rudroid_elf_load_from_bytes(const uint8_t*, uintptr_t)`
    pub const LOAD_FROM_BYTES: &str = "rudroid_elf_load_from_bytes";
    /// `uintptr_t rudroid_elf_load_base(handle)` — the image base / load
    /// bias, distinct from the opaque handle (ticket 48).
    pub const LOAD_BASE: &str = "rudroid_elf_load_base";
    /// `uintptr_t rudroid_elf_symbol(handle, const char*)` — 0 on miss
    pub const SYMBOL: &str = "rudroid_elf_symbol";
    /// `void rudroid_elf_destroy(handle)`
    pub const DESTROY: &str = "rudroid_elf_destroy";
    /// `uintptr_t rudroid_elf_last_error(char* buffer, uintptr_t capacity)`
    /// — length (including NUL) of the calling thread's last C-API failure
    /// message, `0` when there is none; copies a truncated NUL-terminated
    /// prefix when `buffer`/`capacity` allow (ticket 65). Shared by the
    /// buffer and executable backends — both live in the same cdylib.
    pub const LAST_ERROR: &str = "rudroid_elf_last_error";

    pub const ALL: [&str; 7] = [
        LOAD_FROM_PATH,
        LOAD_FROM_BYTES,
        LOAD_BASE,
        SYMBOL,
        DESTROY,
        LAST_ERROR,
        VISIT_IMPORTS,
    ];
}

/// Executable mapping and symbol-source C ABI (Linux only). The loader
/// resolves these on the deployed ELF capability before mapping guest code.
pub mod elf_exec {
    pub type SymbolSource = unsafe extern "C" fn(
        *const std::ffi::c_char, *mut std::ffi::c_void,
    ) -> usize;
    /// Borrowed name/library/version; library and version are both null for
    /// an unversioned lookup. Return nonzero only for a supported ABI tuple.
    pub type VersionedSymbolSource = unsafe extern "C" fn(
        *const std::ffi::c_char, *const std::ffi::c_char,
        *const std::ffi::c_char, *mut std::ffi::c_void,
    ) -> usize;
    /// Called before a quiescent guest image is unmapped; range remains live.
    pub type RangeFinalizer = unsafe extern "C" fn(usize, usize);
    /// Concurrent callback/user pairs are process-pinned; revocation changes future lookups.
    pub const SOURCE_REGISTER: &str = "rudroid_elf_symbol_source_register";
    pub const SOURCE_REGISTER_VERSIONED: &str = "rudroid_elf_symbol_source_register_versioned";
    /// Same version-aware source ABI, with an explicit process-pinned
    /// cleanup callback. Plain symbol sources never imply lifecycle hooks.
    pub const SOURCE_REGISTER_FINALIZED: &str = "rudroid_elf_symbol_source_register_finalized";
    pub const SOURCE_UNREGISTER: &str = "rudroid_elf_symbol_source_unregister";
    /// `struct RudroidExecutableModule* rudroid_elf_exec_load(const char*)`
    pub const EXEC_LOAD: &str = "rudroid_elf_exec_load";
    /// `uintptr_t rudroid_elf_exec_symbol(handle, const char*)` — 0 on miss
    pub const EXEC_SYMBOL: &str = "rudroid_elf_exec_symbol";
    /// `void rudroid_elf_exec_destroy(handle)`
    pub const EXEC_DESTROY: &str = "rudroid_elf_exec_destroy";
    /// `usize rudroid_elf_exec_load_base(handle)` — image base of the exec
    /// mapping (ticket 84: the event base IS the execution image base).
    pub const EXEC_LOAD_BASE: &str = "rudroid_elf_exec_load_base";
    /// `uintptr_t rudroid_elf_exec_symbol_all(handle, const char*)` —
    /// reference `so_symbol_all`: dynamic symbols first, then the file's
    /// `.symtab`; 0 on miss.
    pub const EXEC_SYMBOL_ALL: &str = "rudroid_elf_exec_symbol_all";

    pub const ALL: [&str; 9] = [
        EXEC_LOAD,
        EXEC_SYMBOL,
        EXEC_DESTROY,
        EXEC_LOAD_BASE,
        EXEC_SYMBOL_ALL,
        SOURCE_REGISTER,
        SOURCE_REGISTER_VERSIONED,
        SOURCE_REGISTER_FINALIZED,
        SOURCE_UNREGISTER,
    ];
}

#[cfg(test)]
#[path = "../tests/unit/symbols.rs"]
mod tests;
