//! INPUT:  legacy patch/HUD configuration, pinned Host API and initialized IL2CPP metadata
//! OUTPUT:  il2cpp_patch descriptor, checked configured native patches and Il2cppReady lifecycle
//! POS:    installs common's rooted generic patches/HUD/TouchManager adapters in the pre-game window
//!
//! Up to eight legacy [[il2cpp_patch]] entries execute after successful il2cpp_init.
//! Named [il2cpp_hooks] remain a library mechanism supplied with addresses by callers.
//! Method overloads, every parameter/return type, field metadata and exact Unity
//! GC-handle ABI are checked before installation; unsupported shapes are rejected.
//! Common's AAPCS64 bridge preserves mixed scalar/reference arguments and unwind
//! frames. No arbitrary eight-integer function cast or raw configured field write
//! is used. Backing code/metadata stays process-pinned; target entries must remain
//! idle through the startup installation window. UnityStopping cancels extra work.
//! il2cpp_init remains redirected after its one-shot ready broadcast; no runtime
//! prologue restore occurs while guest threads can execute code.

use rudroid_common::arm64_detour::{self as arm64, Original};

/// Redirect shape of an installed detour (re-exported for diagnostics/tests).
pub use arm64::HookShape;

use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void, CString};

use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::Kind;
use rudroid_common::ModuleKind;
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_host_config::HostConfig;

mod configured;
mod layout_config;

rudroid_common::declare_descriptor!("il2cpp_patch", Kind::Runtime);

/// Patch cap, matching the reference plugin's `MAX_PATCHES` (eight fixed
/// hook slots); further configured entries never load.
pub const MAX_PATCHES: usize = 8;

/// Config key of the legacy table (reference plugin reads the same name).
const PATCH_KEY: &str = "il2cpp_patch";

/// Reference patch mode (`parse_mode` in `il2cpp_patch.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    Scale,
    Freeze,
    ForceReturn,
    Trace,
    Bypass,
}

impl PatchMode {
    /// Named modes only; anything else — including a missing or unreadable
    /// key — stays `Scale`, exactly like the reference default.
    pub fn parse(mode: &str) -> PatchMode {
        match mode {
            "freeze" => PatchMode::Freeze,
            "force_return" => PatchMode::ForceReturn,
            "trace" => PatchMode::Trace,
            "bypass" => PatchMode::Bypass,
            _ => PatchMode::Scale,
        }
    }
}

/// Raw `il2cpp_patch.<index>.*` leaf values as read from a config source,
/// before the validity filter. Defaults are the reference getter fallbacks.
#[derive(Debug, Clone, PartialEq)]
struct PatchFields {
    assembly: String,
    namespace: String,
    class: String,
    method: String,
    argument_count: i64,
    object_argument: i64,
    field: i64,
    multiplier: f64,
    mode: String,
    source_field: i64,
    value: f64,
    integer_field: bool,
    enabled: bool,
}

impl PatchFields {
    /// Read one entry through the `HostConfig` object (in-process callers).
    fn from_config(config: &HostConfig, index: usize) -> PatchFields {
        let key = |field: &str| format!("{PATCH_KEY}.{index}.{field}");
        let string =
            |name: &str, fallback: &str| -> String {
                config
                    .get_string(name, fallback)
                    .unwrap_or_else(|| fallback.to_owned())
            };
        PatchFields {
            assembly: string(&key("assembly"), "Assembly-CSharp"),
            namespace: string(&key("namespace"), ""),
            class: string(&key("class"), ""),
            method: string(&key("method"), ""),
            argument_count: config.get_i64(&key("argc"), 0),
            object_argument: config.get_i64(&key("arg"), 0),
            field: config.get_i64(&key("field"), -1),
            multiplier: config.get_f64(&key("mult"), 1.0),
            mode: string(&key("mode"), "scale"),
            source_field: config.get_i64(&key("src_field"), -1),
            value: config.get_f64(&key("setval"), 0.0),
            integer_field: string(&key("type"), "f32") == "i32",
            enabled: config.get_bool(&key("enabled"), true),
        }
    }

    /// Read one entry through the Host API v0 getters — the exact
    /// consumption path of the reference plugin, which also only sees
    /// dotted-key getters (`config_string`/`config_i32`/... helpers with a
    /// 512-byte scratch buffer).
    fn from_host_api(api: &HostApiV0, index: usize) -> PatchFields {
        let key = |field: &str| format!("{PATCH_KEY}.{index}.{field}");
        fn read_string(api: &HostApiV0, key: &str, fallback: &str) -> String {
            // Keys are generated from fixed field names, fallbacks are
            // literals: neither contains NUL.
            let Ok(key) = CString::new(key) else {
                return fallback.to_owned();
            };
            // An absent key reads as the fallback, as the getter's own copy did.
            api.config_string(&key, 512)
                .unwrap_or_else(|| fallback.to_owned())
        }
        fn read_i64(api: &HostApiV0, key: &str, fallback: i64) -> i64 {
            keyed(key, |key| api.config_i64(key, fallback))
        }
        fn read_f64(api: &HostApiV0, key: &str, fallback: f64) -> f64 {
            keyed(key, |key| api.config_f64(key, fallback))
        }
        fn read_bool(api: &HostApiV0, key: &str, fallback: bool) -> bool {
            keyed(key, |key| api.config_bool(key, fallback))
        }
        fn keyed<T>(key: &str, call: impl FnOnce(&CString) -> T) -> T {
            match CString::new(key) {
                Ok(key) => call(&key),
                // Generated keys are always NUL-free; the arm only keeps the
                // helper total.
                Err(_) => call(&CString::default()),
            }
        }
        PatchFields {
            assembly: read_string(api, &key("assembly"), "Assembly-CSharp"),
            namespace: read_string(api, &key("namespace"), ""),
            class: read_string(api, &key("class"), ""),
            method: read_string(api, &key("method"), ""),
            argument_count: read_i64(api, &key("argc"), 0),
            object_argument: read_i64(api, &key("arg"), 0),
            field: read_i64(api, &key("field"), -1),
            multiplier: read_f64(api, &key("mult"), 1.0),
            mode: read_string(api, &key("mode"), "scale"),
            source_field: read_i64(api, &key("src_field"), -1),
            value: read_f64(api, &key("setval"), 0.0),
            integer_field: read_string(api, &key("type"), "f32") == "i32",
            enabled: read_bool(api, &key("enabled"), true),
        }
    }
}

/// One validated `[[il2cpp_patch]]` request: the reference `Patch` config
/// face. Resolution (assembly image → class → method) needs the guest IL2CPP
/// runtime and happens later in the bring-up flow.
#[derive(Debug, Clone, PartialEq)]
pub struct PatchRequest {
    /// Stable id: the config index it was read from (`il2cpp_patch.<n>`).
    pub id: String,
    pub assembly: String,
    pub namespace: String,
    pub class: String,
    pub method: String,
    pub argument_count: i64,
    /// Which argument register carries the object (`arg`, −1 = this).
    pub object_argument: i64,
    pub field: i64,
    pub multiplier: f64,
    pub mode: PatchMode,
    pub source_field: i64,
    pub value: f64,
    /// `type = "i32"` writes integer fields instead of floats.
    pub integer_field: bool,
}

impl PatchRequest {
    /// Validity filter, exactly the reference `load_patches` continue chain:
    /// disabled entries, empty class/method, out-of-range `argc`/`arg`,
    /// scale/freeze without a field offset and no-op scale multipliers all
    /// drop silently (the reference `continue`s, it does not fail).
    fn from_fields(id: String, fields: PatchFields) -> Option<PatchRequest> {
        let mode = PatchMode::parse(&fields.mode);
        let field_required =
            matches!(mode, PatchMode::Scale | PatchMode::Freeze);
        if !fields.enabled
            || fields.class.is_empty()
            || fields.method.is_empty()
            || fields.argument_count < 0
            || fields.argument_count > 6
            || (field_required && fields.field < 0)
            || fields.object_argument < -1
            || fields.object_argument > 6
            || (mode == PatchMode::Scale && fields.multiplier == 1.0)
        {
            return None;
        }
        Some(PatchRequest {
            id,
            assembly: fields.assembly,
            namespace: fields.namespace,
            class: fields.class,
            method: fields.method,
            argument_count: fields.argument_count,
            object_argument: fields.object_argument,
            field: fields.field,
            multiplier: fields.multiplier,
            mode,
            source_field: fields.source_field,
            value: fields.value,
            integer_field: fields.integer_field,
        })
    }
}

/// One configured named hook: id plus the guest symbol it targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRequest {
    pub id: String,
    /// Symbol name to resolve in the guest il2cpp module.
    pub symbol: String,
    /// Resolved guest address (0 = unresolved).
    pub address: usize,
}

/// Registry of hook requests: legacy `[[il2cpp_patch]]` patches plus the
/// `[il2cpp_hooks]` named-symbol table. BTreeMap keeps the named-hook
/// iteration order stable for deterministic install sequences.
#[derive(Debug, Default, Clone)]
pub struct HookRegistry {
    entries: BTreeMap<String, HookRequest>,
    patches: Vec<PatchRequest>,
    /// Total `[[il2cpp_patch]]` entries in the config, including ones the
    /// filter dropped or the cap truncated (reference logs the difference).
    configured_patches: usize,
}

impl HookRegistry {
    /// Registry from a parsed Host Config. Both dialects are read and both
    /// take effect; an absent table simply contributes nothing (a config
    /// with no IL2CPP hooks at all stays a legal, empty registry).
    pub fn from_config(config: &HostConfig) -> HookRegistry {
        let mut registry = HookRegistry::default();
        // Deterministic order: TOML tables preserve insertion order (toml
        // crate keeps it); sort anyway to stay stable across readers.
        if let Some(section) =
            config.table().get("il2cpp_hooks").and_then(|v| v.as_table())
        {
            let mut keys: Vec<&String> = section.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(symbol) = section.get(key).and_then(|v| v.as_str()) {
                    registry.register_request(key, symbol);
                }
            }
        }
        let configured = config.array_size(PATCH_KEY);
        registry.configured_patches = configured;
        for index in 0..configured {
            // The reference loop stops once its eight slots are full, so the
            // cap counts VALID patches; overflow is reported by the caller.
            if registry.patches.len() == MAX_PATCHES {
                break;
            }
            let fields = PatchFields::from_config(config, index);
            if let Some(request) = PatchRequest::from_fields(
                format!("{PATCH_KEY}.{index}"),
                fields,
            ) {
                registry.patches.push(request);
            }
        }
        registry
    }

    /// Registry read through the Host API v0 config getters themselves —
    /// the reference plugin's consumption path. Only the legacy array table
    /// is visible here: the FFI surface has no table enumeration, so the
    /// named `[il2cpp_hooks]` dialect is a HostConfig-object concern.
    pub fn from_host_api(api: &HostApiV0) -> HookRegistry {
        let mut registry = HookRegistry::default();
        let configured = api.config_len(c"il2cpp_patch") as usize;
        registry.configured_patches = configured;
        for index in 0..configured {
            if registry.patches.len() == MAX_PATCHES {
                break;
            }
            let fields = PatchFields::from_host_api(api, index);
            if let Some(request) = PatchRequest::from_fields(
                format!("{PATCH_KEY}.{index}"),
                fields,
            ) {
                registry.patches.push(request);
            }
        }
        registry
    }

    pub fn register_request(&mut self, id: &str, symbol: &str) {
        self.entries.insert(
            id.to_owned(),
            HookRequest {
                id: id.to_owned(),
                symbol: symbol.to_owned(),
                address: 0,
            },
        );
    }

    /// Resolve a named hook id to a guest address (runtime callback).
    /// Returns the previous address when the hook was already resolved.
    pub fn resolve(&mut self, id: &str, address: usize) -> Result<usize, HookError> {
        let entry = self
            .entries
            .get_mut(id)
            .ok_or_else(|| HookError::UnknownHook(id.to_owned()))?;
        let previous = entry.address;
        entry.address = address;
        Ok(previous)
    }

    pub fn get(&self, id: &str) -> Option<&HookRequest> {
        self.entries.get(id)
    }

    /// Number of named `[il2cpp_hooks]` requests (legacy patches are
    /// counted by `patches`).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.patches.is_empty()
    }

    /// Validated legacy patch requests, in config order.
    pub fn patches(&self) -> &[PatchRequest] {
        &self.patches
    }

    /// Total `[[il2cpp_patch]]` entries seen in the config.
    pub fn configured_patches(&self) -> usize {
        self.configured_patches
    }

    /// Install a REAL inline-code detour at the named hook's resolved
    /// address (ticket 51): the entry word is overwritten with a real branch
    /// — straight to `destination` when it sits within ±128 MiB, otherwise
    /// into a near trampoline page carrying the absolute veneer — and the
    /// covered original word moves into a trampoline, so `Detour::original`
    /// is the callable original function.
    /// Refuses unresolved hooks (mechanism, not offsets: the caller
    /// resolves) and unsupported prologues with an explicit error, leaving
    /// the target intact. Legacy patches are NOT applied here — their
    /// targets come from the guest IL2CPP runtime.
    ///
    /// Contract: `address` must be a 4-byte-aligned guest code entry in pages
    /// this process owns, and no other thread may execute the entry word
    /// itself during the install (the C++ oracle's contract). The patch
    /// window keeps execute permission (`RWX`, then sealed `RX`), so other
    /// threads running code on the same page are unaffected.
    ///
    /// # Safety
    /// See the contract above; patching foreign or concurrently-executing
    /// code crashes the process, like any hook installer.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub unsafe fn apply_detour(
        &self,
        id: &str,
        destination: usize,
    ) -> Result<Detour, HookError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| HookError::UnknownHook(id.to_owned()))?;
        if entry.address == 0 {
            return Err(HookError::Unresolved(id.to_owned()));
        }
        // SAFETY: contract above — caller supplies a live code entry.
        let installed = unsafe { arm64::install(entry.address, destination) }.map_err(
            |error| match error {
                arm64::DetourError::UnsupportedPrologue {
                    instruction,
                    pc_offset,
                } => HookError::UnsupportedPrologue {
                    hook_id: id.to_owned(),
                    instruction,
                    pc_offset,
                },
                other => HookError::Detour {
                    hook_id: id.to_owned(),
                    reason: other.to_string(),
                },
            },
        )?;
        Ok(Detour {
            hook_id: id.to_owned(),
            address: entry.address,
            original: installed.trampoline,
            replacement: destination,
            shape: installed.shape,
            covered_bytes: installed.covered_bytes,
            saved_word: installed.saved_word,
        })
    }

    /// Non-production targets: no inline detour support — explicit failure
    /// instead of a fake success (the pre-51 pointer-slot behavior is gone).
    #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
    pub unsafe fn apply_detour(
        &self,
        id: &str,
        destination: usize,
    ) -> Result<Detour, HookError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| HookError::UnknownHook(id.to_owned()))?;
        if entry.address == 0 {
            return Err(HookError::Unresolved(id.to_owned()));
        }
        let _ = destination;
        Err(HookError::Detour {
            hook_id: id.to_owned(),
            reason: "inline detours require an aarch64 linux target".to_owned(),
        })
    }

    /// Restore the saved prologue over an installed detour (same
    /// execute-preserving patch window). The trampoline page stays mapped:
    /// calling the original keeps working — only the entry stops
    /// redirecting.
    ///
    /// Ticket 82 rework contract: the saved word is an arbitrary prologue
    /// instruction, so no thread may be executing the target entry while it
    /// goes back. The production `Il2cppReady` lifecycle therefore never
    /// restores at runtime (the entry stays redirected; the trampoline keeps
    /// the original callable); this method remains for pre-guest machinery
    /// and tests.
    ///
    /// # Safety
    /// Same contract as [`HookRegistry::apply_detour`], and the target entry
    /// must not be executing during the restore.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub unsafe fn restore_detour(&self, detour: &Detour) -> Result<(), HookError> {
        // SAFETY: the detour was produced by apply_detour over live code.
        let installed = arm64::InstalledDetour {
            target: detour.address,
            destination: detour.replacement,
            trampoline: detour.original,
            shape: detour.shape,
            covered_bytes: detour.covered_bytes,
            saved_word: detour.saved_word,
        };
        // SAFETY: contract as apply_detour.
        unsafe { arm64::restore(&installed) }.map_err(|error| HookError::Detour {
            hook_id: detour.hook_id.clone(),
            reason: error.to_string(),
        })
    }

    /// Non-production targets: nothing was installed, nothing to restore.
    /// The ticket 82 rework contract on the aarch64 variant applies equally:
    /// the production Il2cppReady lifecycle never restores at runtime.
    #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
    pub unsafe fn restore_detour(&self, detour: &Detour) -> Result<(), HookError> {
        Err(HookError::Detour {
            hook_id: detour.hook_id.clone(),
            reason: "inline detours require an aarch64 linux target".to_owned(),
        })
    }
}

/// An installed detour record (guest diagnostics keep these for restore).
/// `original` is the trampoline entry — the callable original function
/// (oracle `orig_out` semantics) — not the overwritten bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detour {
    pub hook_id: String,
    /// Patched entry address.
    pub address: usize,
    /// Trampoline that replays the relocated entry word and resumes the
    /// original.
    pub original: usize,
    pub replacement: usize,
    /// Redirect shape chosen by the range check (direct branch to the
    /// destination vs branch to a near absolute veneer).
    pub shape: HookShape,
    /// Entry bytes the redirect overwrote (always 4: one instruction word).
    pub covered_bytes: usize,
    /// The saved entry word, written back by restore.
    pub saved_word: u32,
}

#[derive(Debug)]
pub enum HookError {
    UnknownHook(String),
    Unresolved(String),
    /// The target prologue cannot be relocated; the target was left intact.
    UnsupportedPrologue {
        hook_id: String,
        instruction: u32,
        pc_offset: usize,
    },
    /// Any other install/restore failure (mapping, protection, alignment).
    Detour { hook_id: String, reason: String },
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::UnknownHook(id) => write!(f, "hook `{id}` is not registered"),
            HookError::Unresolved(id) => write!(
                f,
                "hook `{id}` has no resolved guest address; load the guest il2cpp module first"
            ),
            HookError::UnsupportedPrologue {
                hook_id,
                instruction,
                pc_offset,
            } => write!(
                f,
                "hook `{hook_id}` cannot be detoured: unsupported prologue instruction \
                 {instruction:#010x} at {pc_offset:#x} (target left intact)"
            ),
            HookError::Detour { hook_id, reason } => {
                write!(f, "hook `{hook_id}` detour failed: {reason}")
            }
        }
    }
}

impl std::error::Error for HookError {}

/// Init phase: read the legacy `[[il2cpp_patch]]` table through the Host
/// API v0 getters and report what will be installed — the same surface the
/// reference plugin init uses (`config_array_size` + dotted reads, then a
/// count log). Requests are retained and installed after successful IL2CPP
/// initialization through common's checked managed gateway; init itself does
/// not access managed objects. Absent table logs the reference's
/// "no il2cpp_patch configuration" and succeeds.
///
/// Beyond the config half, init arms the ticket-78 producer: the capability
/// subscribes to the il2cpp module event, detours the guest's `il2cpp_init`
/// export and broadcasts `Il2cppReady` once it returns successfully — the
/// event-model translation of the reference `il2cpp_post_init` broker.
#[no_mangle]
/// # Safety
/// The host table and callbacks remain mapped for the process; guest symbol
/// lookup (including the int-returning il2cpp_init) and module events obey the
/// same-kit native ABI and quiescent hook
/// installation contract.
unsafe extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let registry = HookRegistry::from_host_api(api);
    configured::configure(registry.patches().to_vec(),api);
    let tag = c"IL2PATCH";
    let log = |message: &str| {
        // NUL rejection preserved (skip, never escape) for this call site.
        if message.contains('\0') {
            return;
        }
        api.log_line(tag, message);
    };
    let configured = registry.configured_patches();
    if configured == 0 {
        log("no il2cpp_patch configuration");
    } else {
        let loaded = registry.patches().len();
        if configured > loaded {
            log(&format!("only the first {loaded} patches can be loaded"));
        }
        log(&format!(
            "patches={loaded} configured={configured} (validated requests; installation follows successful IL2CPP initialization)"
        ));
    }
    arm_il2cpp_ready_producer(api, &log)
}

// ---------------------------------------------------------------------------
// Ticket 78: the Il2cppReady producer. Mapped → detour `il2cpp_init`;
// init returning nonzero success → broadcast `Il2cppReady` once, restore
// the guest entry. The reference analogue is `il2cpp_post_init.cpp`
// (detour `il2cpp_init`, dispatch the post-init callbacks after it returns).
// ---------------------------------------------------------------------------

/// Pinned producer surface (ticket 82): set once at init and never mutated.
/// `HostApiV0` is plain function pointers (auto `Send`/`Sync`), so the hot
/// detour path reads it lock-free — no producer lock can ever be held across
/// the guest call, the broadcast or a subscriber callback.
static PRODUCER_API: std::sync::OnceLock<HostApiV0> = std::sync::OnceLock::new();
/// Trampoline of the installed `il2cpp_init` detour (0 = not installed).
static INIT_ORIG: Original<Il2cppInitFn> = Original::new();
type Il2cppInitFn = unsafe extern "C-unwind" fn(domain_name: *const c_char) -> c_int;
/// One-shot guard: `Il2cppReady` fires at most once per process.
static READY_FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
// Ticket 82 rework: the old READY_DETOUR restore bookkeeping is gone with
// the runtime restore itself — the guest entry stays redirected for the
// process lifetime, so no state has to outlive the install.

/// Poison-tolerant lock (house rule): a subscriber that panicked must not
/// wedge the next bring-up; the guard still hands out the data. The 82
/// rework removed the last production user (READY_DETOUR); the helper
/// stays for the lifecycle tests.
#[cfg(test)]
fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn producer_log(message: &str) {
    let Some(api) = PRODUCER_API.get() else {
        return;
    };
    api.log_line(c"IL2PATCH", message);
}

/// Field-wise copy of the loader table (ticket 80 will collapse the per-cap
/// copies into a common helper; il2cpp_hook keeps one local call site).
fn copy_host_api(api: &HostApiV0) -> HostApiV0 {
    // Value copy of the pinned table (ticket 80): no lifetime is extended.
    *api
}

/// Arm the producer: pin the host copy once, subscribe to the il2cpp module
/// event. A duplicate subscription is the registry's first-write-wins
/// rejection (returns -3 like every other registration failure).
fn arm_il2cpp_ready_producer(api: &HostApiV0, log: &dyn Fn(&str)) -> c_int {
    if PRODUCER_API.set(copy_host_api(api)).is_err() {
        log("il2cpp-ready producer already armed (first-write-wins)");
        return 0;
    }
    if !api.on_module_loaded(ModuleKind::Il2cpp, on_il2cpp_mapped, std::ptr::null_mut()) {
        log("module subscription rejected — il2cpp-ready producer not armed");
        return -3;
    }
    if !api.on_module_loaded(ModuleKind::UnityStopping, configured::stopping, std::ptr::null_mut()) {
        log("stop subscription rejected — configured hook shutdown unavailable");
        return -3;
    }
    log("il2cpp-ready producer armed: waiting for the il2cpp module (ticket 78)");
    0
}

/// Module-loaded callback: bind the il2cpp module, detour its `il2cpp_init`
/// export. A missing export or a failed detour is a structured diagnostic —
/// the load itself never fails from producer machinery (ticket 78
/// constraint 3). No producer lock is held across any of this (ticket 82).
extern "C" fn on_il2cpp_mapped(event: Option<&RudroidModuleEvent>, _user: *mut c_void) {
    let Some(event) = event else {
        return;
    };
    if event.kind != ModuleKind::Il2cpp as u32 {
        return;
    }
    let Some(api) = PRODUCER_API.get() else {
        return;
    };
    configured::mapped(event.base as usize);
    let Some(entry) = api.guest_symbol(event.base, c"il2cpp_init") else {
        producer_log(
            "il2cpp_init export missing — Il2cppReady will not fire (metadata-dependent hooks stay off)",
        );
        return;
    };
    let mut registry = HookRegistry::default();
    registry.register_request("il2cpp_init", "il2cpp_init");
    if registry.resolve("il2cpp_init", entry).is_err() {
        return;
    }
    // SAFETY: entry is a live guest code page and il2cpp_init has not run
    // yet (the module-event window); the arm64 install keeps execute
    // permission on the page and performs its own cache maintenance.
    match unsafe { registry.apply_detour("il2cpp_init", il2cpp_init_detour as usize) } {
        Ok(detour) => {
            // SAFETY: the trampoline of the il2cpp_init export just detoured
            // with `il2cpp_init_detour`.
            unsafe { INIT_ORIG.set(detour.original) };
            producer_log(
                "il2cpp_init detoured — Il2cppReady will fire once init returns (ticket 78)",
            );
        }
        Err(error) => {
            producer_log(&format!(
                "il2cpp_init detour failed: {error} — Il2cppReady will not fire"
            ));
        }
    }
}

/// The detour body — the process's guest-facing callback.
///
/// Lifecycle (ticket 82 rework): run the original; zero success status broadcasts
/// nothing and keeps the detour installed (the guest may retry init); a
/// nonzero success status broadcasts `Il2cppReady` exactly once and DONE — the
/// entry STAYS REDIRECTED for the process lifetime. The trampoline (replays
/// the saved prologue, jumps back into the original body) stays mapped for
/// the process lifetime (arm64 module contract), so later `il2cpp_init`
/// calls run the original code unchanged, `READY_FIRED` keeps the broadcast
/// one-shot, and NO guest code page is ever re-patched while guest threads
/// run. That was the 2026-09-19 review finding: the previous one-shot
/// restore opened a page-wide RW (execute-removed) window through
/// `mprotect` — a thread executing ANY function on that page faulted mid-
/// window, and a resident trampoline did not fix that window. Guest code
/// pages are now written exactly once, in the pre-guest install window.
///
/// The original and detour use `C-unwind` so guest exceptions can cross
/// this Rust frame. Registry callbacks keep their plain C ABI; they must
/// not unwind. This body uses pinned state and poison-tolerant locks, and
/// every expected failure is a logged diagnostic.
unsafe extern "C-unwind" fn il2cpp_init_detour(domain_name: *const c_char) -> c_int {
    let status = match INIT_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue and resumes the
        // original function — its real signature applies.
        Some(orig) => unsafe { orig(domain_name) },
        None => 0,
    };
    if status == 0 {
        // Retry semantics: keep the detour installed, stay silent.
        return status;
    }
    if READY_FIRED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return status;
    }
    // Broadcast without holding any producer lock: subscribers may log,
    // patch guest code or re-enter producer helpers.
    if let Some(api) = PRODUCER_API.get() {
        configured::apply(api);
        api.fire_event(ModuleKind::Il2cppReady);
    }
    // Ticket 82 rework: no runtime restore — the entry keeps its branch to
    // the trampoline (behaviorally transparent, see the lifecycle comment).
    // There is deliberately no code-page write and no mprotect on this path.
    producer_log(
        "il2cpp_init returned — Il2cppReady fired; entry stays redirected (no runtime page rewrite, ticket 82)",
    );
    status
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;

#[cfg(test)]
#[path="../tests/unit/layout_config.rs"]
mod layout_config_tests;
