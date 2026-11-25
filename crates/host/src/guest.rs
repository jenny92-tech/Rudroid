//! INPUT:  Host Config guest paths, dynamic ELF/JNI/graphics/Bionic/media/zlib/events capability APIs
//! OUTPUT:  guest module events, JNI_OnLoad and scoped native-source registration
//! POS:    thin bring-up orchestrator; shared services and typed cleanup stay in capabilities
//! Guest SO bring-up from Host Config paths (ticket 32).
//!
//! Mirrors the reference guest load sequence relative to `[paths].game_files`:
//!
//! 0. `[paths].guest_preload` support libraries — explicit, ordered, required
//!    dependency preloads. Each exec load runs its constructors before the
//!    next preload; these libraries fire no module event and receive no
//!    loader-owned `JNI_OnLoad` call.
//! 1. Ordered guest entries contributed by the registered `EngineProfile`
//!    (`build_guest_plan`), with optional `EntryGate` preconditions (for
//!    example addon modules gated on earlier `ModuleKind` events or usable
//!    FMOD `JNI_OnLoad` status) and optional reserved paths.
//!
//! Each mapped kinded module fires its module-loaded event through the
//! ticket-45 C ABI, so a deployed addon observes real provider loads (its
//! own `Mapped` state) instead of a bool argument (ticket 56).
//!
//! After each kinded module's loaded event, the reference JNI_OnLoad call
//! runs (ticket 52): the module's `JNI_OnLoad` export — if any — is invoked
//! with the shared session's `JavaVM*` from the runtime registry, on a
//! separate executable mapping from the `android_elf_load` exec backend
//! (ticket 49). A module exporting `JNI_OnLoad` without a registered
//! session (or without the Linux-only exec backend) fails bring-up
//! explicitly — a guest VM entry point never runs sessionless and a skipped
//! one never passes as called.
//!
//! Mapping goes through the deployed `android_elf_load` capability's C API —
//! the loader never statically links guest-loading implementation crates
//! (ticket 07 contract). Without gamedata the plan fails with an actionable
//! error naming the missing file and its config key; `RUDROID_SKIP_GUEST=1`
//! exists only for compile-level smoke on machines without gamedata and can
//! never satisfy the Silksong acceptance (see crates/docs/ACCEPTANCE.md).

use std::ffi::{c_char, c_int, c_void};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use libloading::Library;
use rudroid_common::{symbols, ModuleKind};

use super::LoadedCapability;

#[path = "guest_plan.rs"]
mod plan;
pub use plan::{EntryGate, GuestEntry, GuestPlan};
use plan::{configured_preload_path, gates_satisfied, valid_preload_path};

#[derive(Debug)]
pub enum GuestError {
    MissingConfigKey(String),
    ElfLoadCapabilityMissing,
    Symbol {
        capability: &'static str,
        reason: String,
    },
    LoadFailed {
        path: PathBuf,
        required: bool,
        reason: String,
    },
    GameFilesMissing {
        dir: PathBuf,
        key: &'static str,
    },
    /// The guest module exports `JNI_OnLoad` but the runtime registry has
    /// no JNI session (ticket 52): calling it without a `JavaVM*` would be
    /// a fake bring-up, so this fails instead.
    JniSessionMissing {
        module: String,
    },
    /// The guest module exports `JNI_OnLoad` but the deployed
    /// `android_elf_load` has no executable-mapping backend (Linux only,
    /// ticket 49): its code cannot run, so the call cannot be made.
    JniExecBackendMissing {
        module: &'static str,
    },
    /// A selected capability's post-Main-JNI hook rejected the bring-up
    /// point after Main `JNI_OnLoad`. Nothing later (including il2cpp) runs.
    AfterMainJniFailed {
        code: c_int,
    },
}

impl std::fmt::Display for GuestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuestError::MissingConfigKey(key) => write!(
                f,
                "host config misses `{key}`; point it at the guest package directory"
            ),
            GuestError::ElfLoadCapabilityMissing => write!(
                f,
                "deploy dir has no android_elf_load capability; guest bring-up cannot map SOs"
            ),
            GuestError::Symbol { capability, reason } => {
                write!(f, "capability {capability} misses its C API: {reason}")
            }
            GuestError::LoadFailed {
                path,
                required: true,
                reason,
            } => write!(
                f,
                "required guest module {} cannot load: {reason}; \
                 verify the gamedata layout (lib/arm64-v8a/...)",
                path.display()
            ),
            GuestError::LoadFailed {
                path,
                required: false,
                reason,
            } => write!(
                f,
                "optional guest module {} is present but failed to load: {reason}",
                path.display()
            ),
            GuestError::GameFilesMissing { dir, key } => write!(
                f,
                "gamedata directory {} (from `{key}`) does not exist; \
                 deploy the guest package first",
                dir.display()
            ),
            GuestError::JniSessionMissing { module } => write!(
                f,
                "guest module {module} exports JNI_OnLoad but no JNI session is \
                 registered (deploy android_jni_vm, ticket 52); a guest VM entry \
                 point must never run without its JavaVM"
            ),
            GuestError::JniExecBackendMissing { module } => write!(
                f,
                "guest module {module} exports JNI_OnLoad but the deployed \
                 android_elf_load has no executable backend (ticket 49, Linux \
                 only); the entry point cannot run"
            ),
            GuestError::AfterMainJniFailed { code } => write!(
                f,
                "a selected post-Main-JNI bootstrap hook returned {code}; \
                 guest bring-up stopped before loading il2cpp"
            ),
        }
    }
}

impl std::error::Error for GuestError {}

/// A successfully mapped guest module: kind, its load base, the planned
/// relative path (the verifiable identity C subscribers receive) and —
/// since ticket 52 — the value `JNI_OnLoad` returned when the module
/// exported one and the loader called it with the shared session's
/// `JavaVM*` (`None`: no export, or the exec backend could not run it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEvent {
    pub kind: ModuleKind,
    pub base: usize,
    /// Path relative to `game_files`, e.g. `lib/arm64-v8a/libmain.so`.
    pub name: &'static str,
    pub jni_on_load: Option<i32>,
}

/// Module-loaded callbacks (reference `register_module_loaded` analogue).
type ModuleCallback = Arc<dyn Fn(&ModuleEvent) + Send + Sync>;

static CALLBACKS: std::sync::Mutex<Vec<(ModuleKind, ModuleCallback)>> =
    std::sync::Mutex::new(Vec::new());

/// Subscribe for the current runtime session.
pub fn register_module_loaded(kind: ModuleKind, callback: ModuleCallback) {
    CALLBACKS.lock().unwrap().push((kind, callback));
}

pub(super) fn clear_callbacks() {
    let callbacks = std::mem::take(&mut *CALLBACKS.lock().unwrap());
    drop(callbacks);
}

fn fire(event: &ModuleEvent) {
    let callbacks = CALLBACKS.lock().unwrap().clone();
    for (kind, callback) in callbacks.iter() {
        if *kind == event.kind {
            callback(event);
        }
    }
    // Cross the C ABI boundary too: subscribers in deployed capability
    // libraries receive the same event with its identity (ticket 45). The
    // name is leaked once per event kind — events fire once per module.
    let c_name = rudroid_common::descriptor::leak_cstr(event.name);
    let c_event = rudroid_common::registry::RudroidModuleEvent {
        kind: event.kind as u32,
        base: event.base,
        name: rudroid_common::registry::HostStr::new(c_name),
    };
    super::runtime::fire_module_event(&c_event);
}

#[path = "guest_elf_api.rs"]
mod elf_api;
use elf_api::{ElfLoadApi, resolve_api, fetch_last_error};

/// Map every planned guest module through the elf_load capability.
/// Returns the fired events in load order (optional modules that were absent
/// produce no event).
///
/// # Safety
/// Planned files are trusted guest code with imports matching the registered
/// native ABIs and JNI callbacks. Backing files stay unmodified while mapped;
/// returned code and any constructor-created threads remain process-pinned.
pub unsafe fn bring_up(
    deployed: &[LoadedCapability],
    plan: &GuestPlan,
) -> Result<Vec<ModuleEvent>, GuestError> {
    // SAFETY: same trusted deployment and lifetime requirements as this entry.
    unsafe { bring_up_observed(deployed, plan, || {}) }
}

/// Notify the CLI immediately before an operation can first execute guest code.
///
/// # Safety
/// The trusted image, callback ABI and mapping requirements of `bring_up` apply.
pub unsafe fn bring_up_observed(
    deployed: &[LoadedCapability],
    plan: &GuestPlan,
    mut entering: impl FnMut(),
) -> Result<Vec<ModuleEvent>, GuestError> {
    // Data prerequisites first: a wrong gamedata directory is the config
    // contract failing, which is more actionable than a kit composition
    // error (ticket 44).
    if !plan.game_files.is_dir() {
        return Err(GuestError::GameFilesMissing {
            dir: plan.game_files.clone(),
            key: "paths.game_files",
        });
    }
    let canonical_game_files =
        std::fs::canonicalize(&plan.game_files).map_err(|_| GuestError::GameFilesMissing {
            dir: plan.game_files.clone(),
            key: "paths.game_files",
        })?;
    // Validate every configured preload before resolving or calling any
    // capability. A directly constructed plan therefore cannot bypass the
    // relative-path/symlink checks through `from_config`.
    let preloads = plan
        .preload
        .iter()
        .map(|relative| {
            configured_preload_path(&canonical_game_files, relative)
                .map(|path| (relative.as_str(), path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // The exec mapper intentionally creates a new image on every call. Reject
    // aliases/duplicates before the first constructor: tracking mapped_paths
    // afterward cannot undo repeated initialization or split global state.
    let mut preload_paths = HashMap::new();
    for (relative, path) in &preloads {
        if let Some(previous) = preload_paths.insert(path.clone(), *relative) {
            return Err(GuestError::LoadFailed {
                path: path.clone(),
                required: true,
                reason: format!("configured guest preload duplicates {previous}: {relative}"),
            });
        }
    }
    for entry in &plan.entries {
        if let Ok(path) = canonical_game_files.join(entry.relative_path).canonicalize() {
            if let Some(relative) = preload_paths.get(&path) {
                return Err(GuestError::LoadFailed {
                    path,
                    required: true,
                    reason: format!("configured guest preload {relative} overlaps the normal bring-up plan: {}", entry.relative_path),
                });
            }
        }
    }
    for path in &plan.reserved_paths {
        if let Some(relative) = preload_paths.get(path) {
            return Err(GuestError::LoadFailed {
                path: path.clone(),
                required: true,
                reason: format!(
                    "configured guest preload {relative} overlaps a reserved Mono/engine path"
                ),
            });
        }
    }

    let capability = deployed
        .iter()
        .find(|c| {
            // SAFETY: pinned capability handle; probing ELF loader entry export only.
            unsafe {
                c.library()
                    .get::<*const c_void>(symbols::elf_load::LOAD_FROM_PATH.as_bytes())
                    .is_ok()
            }
        })
        .ok_or(GuestError::ElfLoadCapabilityMissing)?;

    // SAFETY: the capability handle is pinned by the loader; the C API
    // symbols follow the documented contract in rudroid_common::symbols.
    let api = unsafe { resolve_api(capability.library()) }?;
    let mut bionic_source = install_bionic_source(deployed, capability.library())?;
    let mut media_source = install_native_source(
        deployed,
        capability.library(),
        "android_media_fallback",
        c"rudroid_media_fallback_symbol",
        c"ndk-media-unavailable",
    )?;
    let mut events_source = install_native_source(
        deployed,
        capability.library(),
        "android_native_events",
        c"rudroid_native_events_symbol",
        c"ndk-native-events",
    )?;
    let mut zlib_source = install_native_source(
        deployed,
        capability.library(),
        "android_zlib",
        c"rudroid_zlib_symbol",
        c"guest-zlib",
    )?;
    let mut assets_source = install_native_source(
        deployed,
        capability.library(),
        "android_assets",
        c"rudroid_assets_symbol",
        c"ndk-assets",
    )?;

    let mut events = Vec::new();
    // Reserve canonical identities before any post-Main callback can request them.
    let planned = plan.entries.iter().filter_map(|entry|
        canonical_game_files.join(entry.relative_path).canonicalize().ok());
    let reserved = planned
        .chain(plan.reserved_paths.iter().cloned())
        .chain(preloads.iter().map(|(_, path)| path.clone()))
        .map(|path| (path, ExplicitImageState::ReservedByBringUp))
        .collect();
    let bridge = LoadBridge {
        api: &api,
        game_files: &canonical_game_files,
        images: std::sync::Mutex::new(reserved),
    };
    // Dependencies first, in configuration order. Each exec load publishes
    // the image before running its constructors, so the next preload and the
    // normal plan see the previous library's guest-visible exports. Preloads
    // are support libraries: no module event and no JNI_OnLoad call happens
    // here, even if the library exports JNI_OnLoad for a later Java load.
    for (relative, path) in &preloads {
        let c_path =
            std::ffi::CString::new(path.to_str().ok_or_else(|| GuestError::LoadFailed {
                path: path.clone(),
                required: true,
                reason: "path is not valid UTF-8".to_owned(),
            })?)
            .map_err(|error| GuestError::LoadFailed {
                path: path.clone(),
                required: true,
                reason: error.to_string(),
            })?;
        if let (Some(exec_load), Some(exec_load_base)) = (api.exec_load, api.exec_load_base) {
            entering();
            // SAFETY: pinned capability entries; c_path is NUL-free.
            let handle = unsafe { exec_load(c_path.as_ptr()) };
            if handle.is_null() {
                return Err(GuestError::LoadFailed {
                    path: path.clone(),
                    required: true,
                    reason: unsafe { fetch_last_error(&api) }.unwrap_or_else(|| {
                        "android_elf_load exec backend cannot map the configured preload".to_owned()
                    }),
                });
            }
            let base = unsafe { exec_load_base(handle) };
            rudroid_common::diagnostic!(
                "[GUEST] exec-mapped configured preload {} (base={base:#x}; constructor order preserved; no event/JNI_OnLoad)",
                relative
            );
        } else {
            let handle = unsafe { (api.from_path)(c_path.as_ptr()) };
            if handle.is_null() {
                return Err(GuestError::LoadFailed {
                    path: path.clone(),
                    required: true,
                    reason: unsafe { fetch_last_error(&api) }.unwrap_or_else(|| {
                        "android_elf_load reported a configured-preload load failure".to_owned()
                    }),
                });
            }
            rudroid_common::diagnostic!(
                "[GUEST] data-mapped configured preload {} (no executable backend; no constructor/event/JNI_OnLoad)",
                relative
            );
        }
    }
    // Engine profiles may defer selected ModuleKind JNI_OnLoad to NativeLoader.load.
    for entry in &plan.entries {
        // Ticket 56: reference-order gating (the addon plugins act only on
        // the Unity module-loaded callback; fmodstudio needs a usable fmod
        // JNI_OnLoad). A gated entry is not attempted and says so.
        if !gates_satisfied(&entry.gates, &events) {
            let gates = entry
                .gates
                .iter()
                .map(|gate| gate.name())
                .collect::<Vec<_>>()
                .join(", ");
            rudroid_common::diagnostic!(
                "[GUEST] optional module {} not attempted: gate unsatisfied ({gates}); \
                 skipping (reference plugin order)",
                entry.relative_path
            );
            continue;
        }
        let path = plan.game_files.join(entry.relative_path);
        if !path.is_file() {
            if entry.required {
                return Err(GuestError::LoadFailed {
                    path,
                    required: true,
                    reason: "file not found".to_owned(),
                });
            }
            // Reference behavior: absent optional libs only warn.
            rudroid_common::diagnostic!(
                "[GUEST] optional module {} absent; skipping (non-fatal)",
                path.display()
            );
            continue;
        }
        let c_path =
            std::ffi::CString::new(path.to_str().ok_or_else(|| GuestError::LoadFailed {
                path: path.clone(),
                required: entry.required,
                reason: "path is not valid UTF-8".to_owned(),
            })?)
            .map_err(|e| GuestError::LoadFailed {
                path: path.clone(),
                required: entry.required,
                reason: e.to_string(),
            })?;
        // Ticket 84: ONE load per module. When the exec backend exists it is
        // THE loader for every guest module — kinded or support — and the
        // event base, symbol addresses, constructors and JNI_OnLoad all
        // refer to that single image (address identity). The buffer backend
        // stays as the non-Linux fallback and the in-process test/parse
        // tool; it no longer accompanies an execution load (the old
        // from_path-then-exec_load pair read and relocated every file
        // twice).
        match (api.exec_load, api.exec_symbol, api.exec_load_base) {
            (Some(exec_load), Some(exec_symbol), Some(exec_load_base)) => {
                rudroid_common::diagnostic!(
                    "[GUEST] exec-load begin {} (required={})",
                    entry.relative_path,
                    entry.required
                );
                entering();
                // SAFETY: pinned capability entries; c_path is NUL-free.
                let exec_handle = unsafe { (exec_load)(c_path.as_ptr()) };
                if exec_handle.is_null() {
                    return Err(GuestError::LoadFailed {
                        path,
                        required: entry.required,
                        reason: unsafe { fetch_last_error(&api) }.unwrap_or_else(|| {
                            "android_elf_load exec backend cannot map the module".to_owned()
                        }),
                    });
                }
                let base = unsafe { (exec_load_base)(exec_handle) };
                rudroid_common::diagnostic!(
                    "[GUEST] exec-load returned {} (base={base:#x})",
                    entry.relative_path
                );
                let Some(kind) = entry.kind else {
                    // Support library: participates in the exec image world
                    // (and, from ticket 86, the symbol scope) but fires no
                    // event and has no JNI stage.
                    rudroid_common::diagnostic!(
                        "[GUEST] exec-mapped support library {} (base={base:#x}; no kind — no event)",
                        entry.relative_path
                    );
                    continue;
                };
                super::runtime::register_guest_exec_backend(exec_symbol);
                if let Some(exec_symbol_all) = api.exec_symbol_all {
                    super::runtime::register_guest_exec_symbol_all(exec_symbol_all);
                }
                super::runtime::register_guest_exec_module(base, exec_handle);
                rudroid_common::diagnostic!(
                    "[GUEST] exec-mapped {} (base={base:#x}) single-image (ticket 84)",
                    entry.relative_path
                );
                let event = ModuleEvent {
                    kind,
                    base,
                    name: entry.relative_path,
                    jni_on_load: None,
                };
                fire(&event);
                // Reference sequence: after the module-loaded notification
                // the loader calls JNI_OnLoad with the shared VM — on this
                // same image now (ticket 84).
                let jni_on_load = unsafe {
                    if plan.defer_jni_on_load.contains(&kind) {
                        rudroid_common::diagnostic!("[GUEST] JNI_OnLoad for {} deferred to NativeLoader.load (single initialization)", entry.relative_path);
                        Ok(None)
                    } else {
                        call_jni_on_load_exec(exec_symbol, exec_handle, entry.relative_path)
                    }
                };
                let jni_on_load = match jni_on_load {
                    Ok(jni_on_load) => jni_on_load,
                    Err(error) => {
                        // Failed initialization: drop the base→handle
                        // binding so nothing new can bind to a module whose
                        // JNI_OnLoad failed; the mapping itself stays pinned
                        // (ownership contract, ticket 46).
                        super::runtime::unregister_guest_exec_module(base);
                        return Err(error);
                    }
                };
                if kind == ModuleKind::Main {
                    // Generic fallible seam: Main and the shared VM are live,
                    // while il2cpp has not been mapped or initialized yet.
                    let context = rudroid_common::registry::RudroidAfterMainJniContext {
                        main: rudroid_common::registry::RudroidModuleEvent {
                            kind: kind as u32,
                            base,
                            name: rudroid_common::registry::HostStr::new(
                                rudroid_common::descriptor::leak_cstr(entry.relative_path),
                            ),
                        },
                        loader_user: std::ptr::from_ref(&bridge).cast_mut().cast(),
                        load_and_jni_on_load: Some(load_bridge_jni_on_load),
                    };
                    let hook = super::runtime::invoke_after_main_jni(&context);
                    if hook != 0 {
                        super::runtime::unregister_guest_exec_module(base);
                        return Err(GuestError::AfterMainJniFailed { code: hook });
                    }
                }
                let event = ModuleEvent {
                    jni_on_load,
                    ..event
                };
                events.push(event);
            }
            _ => {
                // No exec backend (non-Linux builds of the capability):
                // data-only fallback with the previous semantics. Kinded
                // modules that export JNI_OnLoad fail explicitly at the
                // call stage instead of mapping a guest that cannot run.
                let handle = unsafe { (api.from_path)(c_path.as_ptr()) };
                if handle.is_null() {
                    return Err(GuestError::LoadFailed {
                        path,
                        required: entry.required,
                        reason: unsafe { fetch_last_error(&api) }.unwrap_or_else(|| {
                            "android_elf_load reported a load failure".to_owned()
                        }),
                    });
                }
                let Some(kind) = entry.kind else { continue };
                // The real image base — the handle itself is an opaque
                // pointer and must never be reported as the base (ticket 48).
                let base = unsafe { (api.load_base)(handle) };
                rudroid_common::diagnostic!(
                    "[GUEST] no executable backend (ticket 49, Linux only): {} \
                     is mapped for data only and guest code cannot run",
                    entry.relative_path
                );
                let event = ModuleEvent {
                    kind,
                    base,
                    name: entry.relative_path,
                    jni_on_load: None,
                };
                fire(&event);
                // SAFETY: pinned data mapping used only to probe for the JNI export.
                let jni_on_load =
                    unsafe { call_jni_on_load(&api, handle, entry.relative_path)? };
                let event = ModuleEvent {
                    jni_on_load,
                    ..event
                };
                events.push(event);
            }
        }
    }
    // Ownership is documented on ElfLoadApi: dropping here pins the handles
    // for the process life instead of destroying them (ticket 46).
    if let Some(source) = &mut bionic_source {
        source.id = -1;
    }
    if let Some(source) = &mut media_source {
        source.id = -1;
    }
    if let Some(source) = &mut zlib_source {
        source.id = -1;
    }
    if let Some(source) = &mut events_source {
        source.id = -1;
    }
    if let Some(source) = &mut assets_source {
        source.id = -1;
    }
    Ok(events)
}

/// Optional native compatibility stays in its own capability. Register once
/// for this bring-up before any relocation; all returned C entry points and
/// both libraries obey the loader's existing process-pinned lifetime.
struct PendingNativeSource {
    id: c_int,
    unregister: unsafe extern "C" fn(c_int) -> c_int,
}
impl Drop for PendingNativeSource {
    fn drop(&mut self) {
        if self.id >= 0 {
            // SAFETY: own registration; the ELF library stays process-pinned.
            unsafe {
                (self.unregister)(self.id);
            }
        }
    }
}

fn install_bionic_source(
    deployed: &[LoadedCapability],
    elf: &Library,
) -> Result<Option<PendingNativeSource>, GuestError> {
    let Some(bionic) = deployed.iter().find(|c| {
        // SAFETY: pinned capability handle; probing Bionic provider export only.
        unsafe { c.library().get::<*const c_void>(b"rudroid_bionic_symbol\0").is_ok() }
    }) else {
        return Ok(None);
    };
    use rudroid_common::symbols::elf_exec::{RangeFinalizer, VersionedSymbolSource};
    // SAFETY: same-build libraries are pinned; the exported signatures are
    // the common ABI. The stateless source accepts concurrent queries.
    unsafe {
        let source: libloading::Symbol<VersionedSymbolSource> = bionic
            .library()
            .get(b"rudroid_bionic_symbol")
            .map_err(|e| GuestError::Symbol {
                capability: "android_bionic",
                reason: e.to_string(),
            })?;
        let finalizer: libloading::Symbol<RangeFinalizer> = bionic
            .library()
            .get(b"rudroid_bionic_finalize_range")
            .map_err(|e| GuestError::Symbol {
                capability: "android_bionic",
                reason: e.to_string(),
            })?;
        let register: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                Option<VersionedSymbolSource>,
                *mut c_void,
                Option<RangeFinalizer>,
            ) -> c_int,
        > = elf
            .get(symbols::elf_exec::SOURCE_REGISTER_FINALIZED.as_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: "android_elf_load",
                reason: e.to_string(),
            })?;
        let unregister: libloading::Symbol<unsafe extern "C" fn(c_int) -> c_int> = elf
            .get(symbols::elf_exec::SOURCE_UNREGISTER.as_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: "android_elf_load",
                reason: e.to_string(),
            })?;
        let id = register(
            c"bionic-native".as_ptr(),
            Some(*source),
            std::ptr::null_mut(),
            Some(*finalizer),
        );
        if id <= 0 {
            return Err(GuestError::Symbol {
                capability: "android_elf_load",
                reason: "rejected the version-aware Bionic source".into(),
            });
        }
        rudroid_common::diagnostic!(
            "[BIONIC] version-aware native ABI source registered before guest mapping"
        );
        Ok(Some(PendingNativeSource {
            id,
            unregister: *unregister,
        }))
    }
}

fn install_native_source(
    deployed: &[LoadedCapability],
    elf: &Library,
    capability_name: &'static str,
    provider_name: &std::ffi::CStr,
    source_name: &std::ffi::CStr,
) -> Result<Option<PendingNativeSource>, GuestError> {
    let Some(capability) = deployed.iter().find(|c| {
        // SAFETY: pinned capability handle; probing native source export only.
        unsafe {
            c.library()
                .get::<*const c_void>(provider_name.to_bytes_with_nul())
                .is_ok()
        }
    }) else {
        return Ok(None);
    };
    use rudroid_common::symbols::elf_exec::VersionedSymbolSource;
    // SAFETY: same-kit libraries and callbacks are process-pinned. The source
    // only accepts its explicit ABI names and retains no user pointer.
    unsafe {
        let source: libloading::Symbol<VersionedSymbolSource> = capability
            .library()
            .get(provider_name.to_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: capability_name,
                reason: e.to_string(),
            })?;
        let register: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                Option<VersionedSymbolSource>,
                *mut c_void,
            ) -> c_int,
        > = elf
            .get(symbols::elf_exec::SOURCE_REGISTER_VERSIONED.as_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: "android_elf_load",
                reason: e.to_string(),
            })?;
        let unregister: libloading::Symbol<unsafe extern "C" fn(c_int) -> c_int> = elf
            .get(symbols::elf_exec::SOURCE_UNREGISTER.as_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: "android_elf_load",
                reason: e.to_string(),
            })?;
        let id = register(source_name.as_ptr(), Some(*source), std::ptr::null_mut());
        if id <= 0 {
            return Err(GuestError::Symbol {
                capability: "android_elf_load",
                reason: format!("rejected native ABI source {capability_name}"),
            });
        }
        rudroid_common::diagnostic!(
            "[NATIVE] {capability_name} ABI source registered before guest mapping"
        );
        Ok(Some(PendingNativeSource {
            id,
            unregister: *unregister,
        }))
    }
}

/// Call `JNI_OnLoad` on a mapped guest module with the shared session's
/// `JavaVM*` (ticket 52; reference `main.cpp` guest sequence). The call runs
/// on the exec mapping guest bring-up created for the module (ticket 49) —
/// module events still carry the data-mapping base because the exec C API
/// has no base accessor yet (documented gap, ticket-52 manifest).
///
/// Returns `None` when the module exports no `JNI_OnLoad`. Missing session
/// or exec backend with an export present: hard `GuestError`, never a
/// silent skip (a guest VM entry point must not run without its session,
/// and a skipped one must not pass for called).
///
/// Call `JNI_OnLoad` on the SINGLE exec image (ticket 84): the exec symbol
/// lookup is both the existence probe and the entry resolution — there is
/// no data view on this path to disagree with it.
///
/// # Safety
/// `exec_handle` must be a live exec mapping from the pinned capability.
unsafe fn call_jni_on_load_exec(
    exec_symbol: unsafe extern "C" fn(*const c_void, *const c_char) -> usize,
    exec_handle: *mut c_void,
    module: &str,
) -> Result<Option<i32>, GuestError> {
    let c_name = std::ffi::CString::new("JNI_OnLoad").expect("literal is NUL-free");
    // SAFETY: live exec mapping; NUL-free name.
    let entry = unsafe { exec_symbol(exec_handle, c_name.as_ptr()) };
    if entry == 0 {
        return Ok(None);
    }
    let vm = super::runtime::shared_vm();
    if vm.is_null() {
        return Err(GuestError::JniSessionMissing { module: module.to_owned() });
    }
    rudroid_common::diagnostic!(
        "[GUEST] calling JNI_OnLoad from {module} (shared JavaVM registered)"
    );
    // SAFETY: the exec symbol resolved above is the module's JNI_OnLoad.
    let entry: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(entry) };
    let returned = unsafe { entry(vm, std::ptr::null_mut()) };
    rudroid_common::diagnostic!("[GUEST] JNI_OnLoad from {module} returned {returned:#x}");
    Ok(Some(returned))
}

/// # Safety
/// `handle` must be a live data mapping from `api.from_path` whose code
/// bytes are the guest's; `exec_handle` (when present) maps the same file
/// for execution.
unsafe fn call_jni_on_load(
    api: &ElfLoadApi,
    handle: *mut c_void,
    module: &'static str,
) -> Result<Option<i32>, GuestError> {
    // Cheap data-view probe first: no export, no exec call needed.
    if unsafe { export_symbol(api, handle, "JNI_OnLoad") }.is_none() {
        return Ok(None);
    }
    Err(GuestError::JniExecBackendMissing { module })
}

#[path = "guest_load_bridge.rs"]
mod load_bridge;
use load_bridge::{load_bridge_jni_on_load, ExplicitImageState, LoadBridge};

/// Look a dynamic symbol up through the capability's C API on the data
/// view; `None` on miss.
///
/// # Safety
/// `handle` must be a live mapping from this `api`.
unsafe fn export_symbol(api: &ElfLoadApi, handle: *mut c_void, name: &str) -> Option<usize> {
    let Ok(c_name) = std::ffi::CString::new(name) else {
        return None;
    };
    let address = unsafe { (api.symbol)(handle, c_name.as_ptr()) };
    (address != 0).then_some(address)
}
