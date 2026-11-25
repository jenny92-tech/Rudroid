//! INPUT:  installed Host Config, verified host display geometry and process-pinned capabilities
//! OUTPUT:  authoritative Host API config/log/registry entries and one display publication
//! POS:    serves shared configuration and actual display geometry across capability libraries
//! Loader side of Host API v0 (ticket 11).
//!
//! Fills the `HostApiV0` table from the process-wide Host Config and a log
//! sink, then hands it to every capability exporting `rudroid_plugin_init`.
//! The table functions are static: they read a process-global config/sink the
//! loader installs once, mirroring the reference's static `api_config_*`
//! implementations over its global `config` table.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rudroid_common::host_api::{HostApiV0, LogFn};
use rudroid_common::registry::HostStr;
use rudroid_common::runtime_policy::MemoryPolicy;
use rudroid_host_config::HostConfig;
use libloading::Library;

type LogSink = dyn Fn(&str, &str) + Send + Sync;

struct InstalledConfig {
    document: Option<HostConfig>,
    toml_cstr: Option<CString>,
    memory: MemoryPolicy,
    resources: crate::resources::Service,
}

impl InstalledConfig {
    fn new(document: Option<HostConfig>) -> Result<Self, &'static str> {
        let toml_cstr = match document.as_ref() {
            Some(config) => {
                let text = config
                    .to_toml_string()
                    .map_err(|_| "failed to serialize Host Config to TOML")?;
                let cstr = CString::new(text)
                    .map_err(|_| "serialized Host Config contains interior NUL")?;
                Some(cstr)
            }
            None => None,
        };
        let memory = document.as_ref().map(HostConfig::memory_policy).transpose()?.unwrap_or_default();
        let resources = crate::resources::Service::new(document.as_ref())?;
        Ok(Self { document, toml_cstr, memory, resources })
    }

}

static CONFIG: OnceLock<InstalledConfig> = OnceLock::new();
static LOG_SINK: OnceLock<Box<LogSink>> = OnceLock::new();
static CAPABILITY_LOG_SINK: OnceLock<LogFn> = OnceLock::new();
static ACTIVE_CAPABILITIES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static TABLE: OnceLock<HostApiV0> = OnceLock::new();
static DISPLAY: OnceLock<rudroid_common::display_dimensions::RenderDimensions> = OnceLock::new();
/// Named process-pinned services: name → (abi_version, pointer bits).
static SERVICE_REGISTRY: OnceLock<Mutex<HashMap<String, (u32, usize)>>> = OnceLock::new();
/// Exact canonical source path → NUL-terminated replacement path.
static FILE_OVERLAYS: OnceLock<Mutex<HashMap<PathBuf, CString>>> = OnceLock::new();
/// Runtime string overrides published through config_set_string (e.g. boot cmdline).
static CONFIG_STRING_OVERRIDES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
/// Fast path: skip canonicalize when no overlays are registered.
static HAS_FILE_OVERLAYS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

extern "C" fn host_config_toml_utf8() -> HostStr<'static> {
    CONFIG
        .get()
        .and_then(|holder| holder.toml_cstr.as_deref())
        .map(HostStr::new)
        .unwrap_or_else(HostStr::null)
}

static HOST_CONFIG_TOML_SERVICE: rudroid_common::host_api::HostConfigTomlV1 =
    rudroid_common::host_api::HostConfigTomlV1 {
        abi_version: rudroid_common::host_api::HOST_CONFIG_TOML_SERVICE_ABI,
        toml_utf8: host_config_toml_utf8,
    };

fn insert_host_services(registry: &mut HashMap<String, (u32, usize)>) {
    registry.insert(
        rudroid_common::host_api::HOST_CONFIG_TOML_SERVICE_NAME
            .to_str()
            .expect("ASCII service name")
            .to_owned(),
        (
            rudroid_common::host_api::HOST_CONFIG_TOML_SERVICE_ABI,
            std::ptr::from_ref(&HOST_CONFIG_TOML_SERVICE) as usize,
        ),
    );
}

fn service_registry() -> &'static Mutex<HashMap<String, (u32, usize)>> {
    SERVICE_REGISTRY.get_or_init(|| {
        let mut map = HashMap::new();
        insert_host_services(&mut map);
        Mutex::new(map)
    })
}

fn file_overlays() -> &'static Mutex<HashMap<PathBuf, CString>> {
    FILE_OVERLAYS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn config_string_overrides() -> &'static Mutex<HashMap<String, String>> {
    CONFIG_STRING_OVERRIDES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Publish the validated drawable/render pair before guest preparation. One
/// immutable value keeps all capability copies on the same geometry.
pub(crate) fn install_display_dimensions(
    size: rudroid_common::display_dimensions::RenderDimensions,
) -> Result<(), &'static str> {
    let validated = rudroid_common::display_dimensions::RenderDimensions::resolve(
        i64::from(size.output_width), i64::from(size.output_height),
        i64::from(size.width), i64::from(size.height), 100, size.sharp,
    )?;
    if validated != size { return Err("invalid resolved display geometry"); }
    match DISPLAY.set(size) {
        Ok(()) => Ok(()),
        Err(_) if DISPLAY.get() == Some(&size) => Ok(()),
        Err(_) => Err("display geometry was already published with another size"),
    }
}

/// Install the config the config getters read. Later installs are ignored.
pub fn install_config(config: Option<HostConfig>) -> Result<(), &'static str> {
    // Validate even a repeated install, but never create another boot owner
    // after publication. Concurrent cold contenders publish config/resources
    // together; a losing unlinked-file owner closes through normal Drop.
    if let Some(config) = &config { config.memory_policy()?; }
    if CONFIG.get().is_some() { return Ok(()); }
    let _ = CONFIG.set(InstalledConfig::new(config)?);
    Ok(())
}

/// Install the log sink the `log` table entry calls. Default: stderr.
pub fn install_log_sink(sink: Box<LogSink>) {
    let _ = LOG_SINK.set(sink);
}

fn with_config<R>(f: impl FnOnce(Option<&HostConfig>) -> R) -> R {
    f(CONFIG.get().and_then(|holder| holder.document.as_ref()))
}

/// All capabilities read one immutable policy through the existing Host API.
pub fn memory_policy() -> MemoryPolicy {
    CONFIG.get().map(|c| c.memory).unwrap_or_default()
}

pub(crate) fn resource_service() -> Option<&'static crate::resources::Service> {
    CONFIG.get().map(|c| &c.resources)
}

/// Publish the validated capability IDs before capability phases execute.
pub(crate) fn set_active_capabilities(ids: Vec<String>) {
    *ACTIVE_CAPABILITIES
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = ids;
}

extern "C" fn api_register_log_sink(sink: Option<LogFn>) -> c_int {
    let Some(sink) = sink else {
        return -1;
    };
    if CAPABILITY_LOG_SINK.set(sink).is_ok() {
        0
    } else {
        -1
    }
}

extern "C" fn api_has_capability(id: HostStr<'_>) -> c_int {
    let Some(id) = key(id) else {
        return 0;
    };
    let active = ACTIVE_CAPABILITIES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    c_int::from(active.iter().any(|entry| entry == id))
}

extern "C" fn api_register_service(
    name: HostStr<'static>,
    service: *const c_void,
    abi_version: u32,
) -> c_int {
    let Some(name) = key(name) else {
        return -1;
    };
    if service.is_null() || name.is_empty() {
        return -1;
    }
    let mut registry = service_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if registry.contains_key(name) {
        return -1;
    }
    registry.insert(name.to_owned(), (abi_version, service as usize));
    0
}

extern "C" fn api_query_service(name: HostStr<'_>, expected_abi_version: u32) -> *const c_void {
    let Some(name) = key(name) else {
        return std::ptr::null();
    };
    let registry = service_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match registry.get(name) {
        Some(&(abi, ptr)) if abi == expected_abi_version => ptr as *const c_void,
        _ => std::ptr::null(),
    }
}

extern "C" fn api_register_file_overlay(
    canonical_source: HostStr<'_>,
    replacement_path: HostStr<'_>,
) -> c_int {
    let Some(source) = canonical_source.to_cstr() else {
        return -1;
    };
    let Some(replacement) = replacement_path.to_cstr() else {
        return -1;
    };
    if source.to_bytes().is_empty() || replacement.to_bytes().is_empty() {
        return -1;
    }
    let path = Path::new(OsStr::from_bytes(source.to_bytes()));
    let Ok(canonical) = path.canonicalize() else {
        return -1;
    };
    let mut overlays = file_overlays().lock().unwrap_or_else(|e| e.into_inner());
    if overlays.contains_key(&canonical) {
        return -1;
    }
    let Ok(stored) = CString::new(replacement.to_bytes()) else {
        return -1;
    };
    overlays.insert(canonical, stored);
    HAS_FILE_OVERLAYS.store(true, std::sync::atomic::Ordering::Release);
    0
}

/// Brief overlay lookup used by the resource authority after resolver replies.
pub(crate) fn file_overlay_redirect(candidate: &CStr) -> Option<Vec<u8>> {
    if !HAS_FILE_OVERLAYS.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    let path = Path::new(OsStr::from_bytes(candidate.to_bytes()));
    let canonical = path.canonicalize().ok()?;
    let overlays = file_overlays().lock().unwrap_or_else(|e| e.into_inner());
    overlays
        .get(&canonical)
        .map(|replacement| replacement.as_bytes_with_nul().to_vec())
}

/// Clear version-7 registries on failed bring-up; BootView owners retain their fds.
pub(crate) fn clear_service_and_overlay_registries() {
    {
        let mut registry = service_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.clear();
        insert_host_services(&mut registry);
    }
    file_overlays()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    HAS_FILE_OVERLAYS.store(false, std::sync::atomic::Ordering::Release);
    config_string_overrides()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

fn log_to_sink(tag: &str, message: &str) {
    if let Some(sink) = LOG_SINK.get() {
        sink(tag, message);
        return;
    }
    if let Some(sink) = CAPABILITY_LOG_SINK.get() {
        let c_tag = CString::new(tag.replace('\0', "\\0")).unwrap_or_else(|_| c"PLUGIN".into());
        rudroid_common::host_api::log_message(*sink, &c_tag, message);
        return;
    }
    if rudroid_common::logging::is_error_tag(tag) || rudroid_common::logging::diagnostics_enabled() {
        rudroid_common::logging::stderr(format_args!("[{tag}] {message}"));
    }
}

/// Loader-side log through the same sink the capabilities' `api.log` uses,
/// so loader-stage lines ([PROFILE] runner stages) and capability lines are
/// observable in one channel (the integration tests assert on the order).
pub fn loader_log(tag: &str, message: &str) {
    log_to_sink(tag, message);
}

fn key(text: HostStr<'_>) -> Option<&str> {
    text.to_cstr()?.to_str().ok()
}

/// getenv result cache: name → NUL-terminated process-lifetime C string.
///
/// Lifetime and duplicate-read policy (ticket 46): every returned pointer is
/// a valid, NUL-terminated C string (including empty values, which return a
/// pointer to `""`). The same variable always yields the SAME pointer —
/// values are snapshotted at first query, mirroring the reference, whose
/// config table does not change after startup. The cache only grows with the
/// number of distinct queried names (bounded by the capability set), not
/// with the number of calls.
static ENV_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, &'static CString>>> =
    std::sync::OnceLock::new();

extern "C" fn api_getenv(name: HostStr<'_>) -> *const c_char {
    let Some(name) = key(name) else {
        return std::ptr::null();
    };
    // Only snapshot what capabilities actually query — never the whole
    // user environment.
    let cache = ENV_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    if let Some(cached) = cache.get(name) {
        return cached.as_ptr();
    }
    let Some(value) = std::env::var(name).ok() else {
        return std::ptr::null();
    };
    let Ok(c_string) = CString::new(value) else {
        return std::ptr::null();
    };
    let leaked: &'static CString = Box::leak(Box::new(c_string));
    cache.insert(name.to_owned(), leaked);
    leaked.as_ptr()
}

extern "C" fn api_config_get_bool(key_ptr: HostStr<'_>, fallback: c_int) -> c_int {
    let Some(name) = key(key_ptr) else {
        return fallback;
    };
    if name == "gpu.renderScaleSharp" {
        if let Some(size) = DISPLAY.get() {
            return c_int::from(size.sharp);
        }
    }
    if name == "runtime.preload_audio" {
        return memory_policy().preload_audio as c_int;
    }
    with_config(|config| match config {
        Some(config) => config.get_bool(name, fallback != 0) as c_int,
        None => fallback,
    })
}

extern "C" fn api_config_get_i64(key_ptr: HostStr<'_>, fallback: i64) -> i64 {
    let Some(name) = key(key_ptr) else {
        return fallback;
    };
    if let Some(size) = DISPLAY.get() {
        match name {
            "device.displayWidth" => return i64::from(size.output_width),
            "device.displayHeight" => return i64::from(size.output_height),
            "gpu.renderWidth" => return i64::from(size.width),
            "gpu.renderHeight" => return i64::from(size.height),
            _ => {},
        }
    }
    match name {
        "runtime.stream_buffer_bytes" => return memory_policy().stream_buffer_bytes as i64,
        "runtime.stream_buffer_budget_mib" => return memory_policy().stream_buffer_budget_mib as i64,
        _ => {},
    }
    with_config(|config| match config {
        Some(config) => config.get_i64(name, fallback),
        None => fallback,
    })
}

extern "C" fn api_config_get_f64(key_ptr: HostStr<'_>, fallback: f64) -> f64 {
    let Some(name) = key(key_ptr) else {
        return fallback;
    };
    with_config(|config| match config {
        Some(config) => config.get_f64(name, fallback),
        None => fallback,
    })
}

/// # Safety
/// `out` is null or writable for `out_size` bytes.
unsafe extern "C" fn api_config_get_string(
    key_ptr: HostStr<'_>,
    fallback: HostStr<'_>,
    out: *mut c_char,
    out_size: u32,
) -> c_int {
    if out.is_null() || out_size == 0 {
        return 0;
    }
    let Some(name) = key(key_ptr) else {
        return 0;
    };
    let fallback = key(fallback).unwrap_or("");
    let value = {
        let overrides = config_string_overrides()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        overrides
            .get(name)
            .cloned()
            .or_else(|| with_config(|config| config.and_then(|c| c.get_string(name, fallback))))
    };
    match value {
        Some(value) => {
            // SAFETY: caller contract — `out` holds `out_size` bytes.
            unsafe { copy_into(out, out_size, &value) };
            1
        }
        None => {
            // SAFETY: as above.
            unsafe { copy_into(out, out_size, fallback) };
            0
        }
    }
}

/// Copy `value` plus NUL into `out`, truncating when it does not fit
/// (reference parity: snprintf semantics).
unsafe fn copy_into(out: *mut c_char, out_size: u32, value: &str) {
    let bytes = value.as_bytes();
    let usable = (out_size as usize - 1).min(bytes.len());
    // SAFETY: caller contract — `out` holds `out_size > usable` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, usable);
        *out.add(usable) = 0;
    }
}

extern "C" fn api_config_array_size(key_ptr: HostStr<'_>) -> u32 {
    let Some(name) = key(key_ptr) else {
        return 0;
    };
    with_config(|config| match config {
        Some(config) => config.array_size(name) as u32,
        None => 0,
    })
}

extern "C" fn api_config_set_string(key_ptr: HostStr<'_>, value_ptr: HostStr<'_>) -> c_int {
    let Some(name) = key(key_ptr) else {
        return 0;
    };
    // HostConfig stays immutable for user configuration. Runtime overrides are
    // scoped to first-write-wins `package.mainIntentBundle.<key>` projections
    // (e.g. Unity startup command-line flags published after BootView).
    let Some(leaf) = name.strip_prefix("package.mainIntentBundle.") else {
        return 0;
    };
    if leaf.is_empty() || leaf.contains('.') {
        return 0;
    }
    let Some(value) = value_ptr.to_cstr().map(|v| v.to_string_lossy().into_owned()) else {
        return 0;
    };
    let mut overrides = config_string_overrides()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if overrides.contains_key(name) {
        return 0;
    }
    overrides.insert(name.to_owned(), value);
    1
}

extern "C" fn api_so_symbol(module: *mut c_void, name: HostStr<'_>) -> usize {
    // Ticket 53: guest symbol lookup is live. `module` is the load base a
    // module-loaded event reported (the verifiable identity, never the
    // opaque handle); the resolution goes through the exec-mapping table
    // guest bring-up filled, so the profile lifecycle dispatches run real
    // guest code (ticket 49 mappings). 0 on any miss — callers treat it as
    // a missing required function and fail explicitly.
    let Some(name) = name.to_cstr() else {
        return 0;
    };
    crate::runtime::lookup_guest_symbol(module as usize, name, false).unwrap_or(0)
}

extern "C" fn api_so_symbol_all(module: *mut c_void, name: HostStr<'_>) -> usize {
    // Reference `so_symbol_all`: the exec mapping's dynamic symbols, then
    // the module file's `.symtab`; same base identity and 0-on-miss
    // contract as `api_so_symbol`.
    let Some(name) = name.to_cstr() else {
        return 0;
    };
    crate::runtime::lookup_guest_symbol(module as usize, name, true).unwrap_or(0)
}

extern "C" fn api_log(tag: HostStr<'_>, message: HostStr<'_>) {
    if LOG_SINK.get().is_none() {
        if let Some(sink) = CAPABILITY_LOG_SINK.get() {
            sink(tag, message);
            return;
        }
    }
    let tag = tag.to_cstr().map(|v| v.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed("PLUGIN"));
    let message = message.to_cstr().map(|v| v.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed(""));
    log_to_sink(&tag, &message);
}

/// The process-wide table; built once on first use. Registration entries
/// point into the runtime registry (ticket 45).
pub fn table() -> &'static HostApiV0 {
    TABLE.get_or_init(|| HostApiV0 {
        abi_version: rudroid_common::host_api::HOST_API_VERSION,
        getenv: api_getenv,
        config_get_bool: api_config_get_bool,
        config_get_i64: api_config_get_i64,
        config_get_f64: api_config_get_f64,
        // SAFETY: the static getter bounds writes, NUL-terminates successes and retains no borrows.
        config_get_string: unsafe { rudroid_common::host_api::ConfigGetStringFn::new(api_config_get_string) },
        config_array_size: api_config_array_size,
        config_set_string: api_config_set_string,
        so_symbol: api_so_symbol,
        log: api_log,
        register_module_loaded: crate::runtime::ffi::register_module_loaded_c,
        register_engine_profile: crate::runtime::ffi::register_engine_profile_c,
        register_android_hooks: crate::runtime::ffi::register_android_hooks_c,
        register_audio_policy: crate::runtime::ffi::register_audio_policy_c,
        register_catalog_provider: crate::runtime::ffi::register_catalog_provider_c,
        catalog_query_int: crate::runtime::ffi::catalog_query_int_c,
        catalog_query_bool: crate::runtime::ffi::catalog_query_bool_c,
        register_shared_vm: crate::runtime::ffi::register_shared_vm_c,
        shared_vm: crate::runtime::ffi::shared_vm_c,
        fire_module_event: crate::runtime::ffi::fire_module_event_c,
        so_symbol_all: api_so_symbol_all,
        // SAFETY: the static resource entry copies only within capacity and never retains request/output pointers.
        asset_query: unsafe { rudroid_common::host_api::AssetQueryFn::new(crate::resources::query) },
        enable_asset_packs: crate::resources::enable_packs,
        register_after_main_jni: crate::runtime::ffi::register_after_main_jni_c,
        register_log_sink: api_register_log_sink,
        has_capability: api_has_capability,
        register_service: api_register_service,
        query_service: api_query_service,
        register_file_overlay: api_register_file_overlay,
    })
}

#[derive(Debug)]
pub enum InitError {
    /// Capability does not export init — legal, phases are optional.
    NotPresent,
    /// Capability exported init but returned non-zero.
    Failed { id: String, code: i32 },
    /// Symbol exists but could not be resolved (malformed library).
    Symbol { reason: String },
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::NotPresent => write!(f, "no init export"),
            InitError::Failed { id, code } => {
                write!(f, "capability `{id}` init failed with code {code}")
            }
            InitError::Symbol { reason } => write!(f, "cannot resolve init: {reason}"),
        }
    }
}

impl std::error::Error for InitError {}

/// Resolve and invoke one phase entry point with the v0 table. The phase's
/// failure path loses the capability id here; the caller (phases module)
/// re-attaches it.
///
/// # Safety
/// `library` must be a valid, still-open capability handle.
pub unsafe fn run_phase_entry(
    phase: crate::phases::Phase,
    library: &Library,
) -> Result<(), InitError> {
    let entry: libloading::Symbol<unsafe extern "C" fn(*const HostApiV0) -> c_int> =
        match library.get(phase.symbol().as_bytes()) {
            Ok(entry) => entry,
            // dlsym failure == the optional phase is not exported.
            Err(libloading::Error::DlSym { .. }) | Err(libloading::Error::DlSymUnknown) => {
                return Err(InitError::NotPresent)
            }
            Err(err) => return Err(InitError::Symbol { reason: err.to_string() }),
        };
    let code = entry(table() as *const HostApiV0);
    if code == 0 {
        Ok(())
    } else {
        Err(InitError::Failed {
            id: String::new(),
            code,
        })
    }
}

/// Call a capability's `rudroid_plugin_init` with the v0 table.
///
/// # Safety
/// `library` must be a valid, still-open capability handle.
pub unsafe fn init_capability(
    id: &str,
    library: &Library,
) -> Result<(), InitError> {
    match run_phase_entry(crate::phases::Phase::Init, library) {
        Ok(()) => Ok(()),
        Err(InitError::Failed { code, .. }) => Err(InitError::Failed {
            id: id.to_owned(),
            code,
        }),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
#[path = "../tests/unit/host_api_getenv_tests.rs"]
mod getenv_tests;
