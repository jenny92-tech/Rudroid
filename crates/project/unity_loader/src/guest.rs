//! INPUT:  Host Config guest paths, dynamic ELF/JNI/graphics/Bionic/media/zlib/events capability APIs
//! OUTPUT:  guest module events, JNI_OnLoad and scoped native-source registration
//! POS:    thin bring-up orchestrator; shared services and typed cleanup stay in capabilities
//! Guest SO bring-up from Host Config paths (ticket 32).
//!
//! Mirrors the reference `main.cpp` guest load sequence, relative to the
//! `[paths].game_files` directory:
//!
//! 1. `lib/arm64-v8a/libc++_shared.so` — optional (absence only warns),
//! 2. `lib/arm64-v8a/libmain.so` — required, fires `ModuleKind::Main`,
//! 3. `lib/arm64-v8a/libil2cpp.so` — required, fires `ModuleKind::Il2cpp`
//!    (the mono-branch fallback `libmonobdwgc-2.0.so` + helpers is a
//!    documented follow-up; Silksong is an il2cpp Title),
//! 4. `lib/arm64-v8a/libunity.so` — optional in the synthetic kit (ticket 56;
//!    the reference treats it as required and fires `ModuleKind::Unity`),
//!    then the Android addon guest libraries with the reference plugin
//!    gating: burst / helpers / xlua / fmod only after the Unity module
//!    fired (`AfterUnityGuestModule` + `sdk_fmod` on_unity_loaded), and
//!    `libfmodstudio.so` only after fmod loaded with a usable `JNI_OnLoad`
//!    outcome (oracle: "skipping libfmodstudio after JNI_OnLoad failure").
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
use std::path::PathBuf;
use std::sync::Arc;

use rudroid_common::{symbols, ModuleKind};
use libloading::Library;

use super::LoadedCapability;

/// One guest module the bring-up maps, in reference order.
#[derive(Debug, Clone)]
pub struct GuestEntry {
    pub kind: Option<ModuleKind>,
    /// Path relative to the game_files dir.
    pub relative_path: &'static str,
    pub required: bool,
    /// Ticket 56: reference plugin gating. In the C++ stack the addon
    /// plugins load these libraries themselves inside their Unity
    /// module-loaded callback; the kit realizes the same load+notify outcome
    /// through this plan, so the gate keeps the reference order observable.
    /// An entry with unsatisfied gates is never attempted (logged).
    pub gates: &'static [EntryGate],
}

/// Reference-order gate for a plan entry (ticket 56).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryGate {
    /// Only after this kind's loaded event fired (the reference
    /// `AfterUnityGuestModule` acts solely on the Unity callback).
    KindLoaded(ModuleKind),
    /// Only when the fmod module loaded AND its `JNI_OnLoad` outcome is
    /// acceptable — absent export or a reference-supported version
    /// (sdk_fmod `supported_jni_version`).
    FmodJniOk,
}

impl EntryGate {
    fn name(self) -> &'static str {
        match self {
            EntryGate::KindLoaded(kind) => match kind {
                ModuleKind::Unity => "unity loaded",
                ModuleKind::Fmod => "fmod loaded",
                other => other.name(),
            },
            EntryGate::FmodJniOk => "fmod JNI_OnLoad version usable",
        }
    }
}

/// `true` when every gate of the entry is satisfied by the events fired so
/// far (the plan processes entries in reference order, so earlier events are
/// already recorded).
fn gates_satisfied(gates: &[EntryGate], events: &[ModuleEvent]) -> bool {
    gates.iter().all(|gate| match gate {
        EntryGate::KindLoaded(kind) => events.iter().any(|event| event.kind == *kind),
        EntryGate::FmodJniOk => events
            .iter()
            .any(|event| event.kind == ModuleKind::Fmod && fmod_jni_usable(event.jni_on_load)),
    })
}

/// The oracle `supported_jni_version` set (sdk_fmod.cpp): 1.1/1.2/1.4/1.6/1.8.
/// A module without the export is usable (the oracle leaves `g_fmod_jni_ok`
/// true when the address is missing).
fn fmod_jni_usable(jni_on_load: Option<i32>) -> bool {
    match jni_on_load {
        None => true,
        Some(version) => matches!(
            version,
            0x0001_0001 | 0x0001_0002 | 0x0001_0004 | 0x0001_0006 | 0x0001_0008
        ),
    }
}

/// The guest load plan derived from the Host Config.
#[derive(Debug, Clone)]
pub struct GuestPlan {
    /// `[paths].game_files` — the reference chdirs here before loading.
    pub game_files: PathBuf,
    pub entries: Vec<GuestEntry>,
}

impl GuestPlan {
    pub fn from_config(config: &rudroid_host_config::HostConfig) -> Result<GuestPlan, GuestError> {
        let paths = config.paths();
        let Some(game_files) = paths.game_files else {
            return Err(GuestError::MissingConfigKey(
                "paths.game_files".to_owned(),
            ));
        };
        use ModuleKind::{Burst, Fmod, FmodStudio, Il2cpp, Main, Unity, UnityHelpers, Xlua};
        const UNITY_ONLY: &[EntryGate] = &[EntryGate::KindLoaded(Unity)];
        const FMOD_STUDIO: &[EntryGate] = &[
            EntryGate::KindLoaded(Unity),
            EntryGate::KindLoaded(Fmod),
            EntryGate::FmodJniOk,
        ];
        Ok(GuestPlan {
            game_files: PathBuf::from(game_files),
            entries: vec![
                GuestEntry {
                    kind: None,
                    relative_path: "lib/arm64-v8a/libc++_shared.so",
                    required: false,
                    gates: &[],
                },
                GuestEntry {
                    kind: Some(Main),
                    relative_path: "lib/arm64-v8a/libmain.so",
                    required: true,
                    gates: &[],
                },
                GuestEntry {
                    kind: Some(Il2cpp),
                    relative_path: "lib/arm64-v8a/libil2cpp.so",
                    required: true,
                    gates: &[],
                },
                // Ticket 56: the reference main.cpp requires libunity.so and
                // fires RUDROID_MODULE_UNITY for it. Synthetic kits may lack the
                // file (documented divergence: optional here), and the addon
                // entries keep the reference gating on the fired event.
                GuestEntry {
                    kind: Some(Unity),
                    relative_path: "lib/arm64-v8a/libunity.so",
                    required: false,
                    gates: &[],
                },
                GuestEntry {
                    kind: Some(Burst),
                    relative_path: "lib/arm64-v8a/lib_burst_generated.so",
                    required: false,
                    gates: UNITY_ONLY,
                },
                GuestEntry {
                    kind: Some(UnityHelpers),
                    relative_path: "lib/arm64-v8a/libUnityHelpers_Android.so",
                    required: false,
                    gates: UNITY_ONLY,
                },
                GuestEntry {
                    kind: Some(Xlua),
                    relative_path: "lib/arm64-v8a/libxlua.so",
                    required: false,
                    gates: UNITY_ONLY,
                },
                GuestEntry {
                    kind: Some(Fmod),
                    relative_path: "lib/arm64-v8a/libfmod.so",
                    required: false,
                    gates: UNITY_ONLY,
                },
                GuestEntry {
                    kind: Some(FmodStudio),
                    relative_path: "lib/arm64-v8a/libfmodstudio.so",
                    required: false,
                    gates: FMOD_STUDIO,
                },
            ],
        })
    }
}

#[derive(Debug)]
pub enum GuestError {
    MissingConfigKey(String),
    ElfLoadCapabilityMissing,
    Symbol { capability: &'static str, reason: String },
    LoadFailed { path: PathBuf, required: bool, reason: String },
    GameFilesMissing { dir: PathBuf, key: &'static str },
    /// The guest module exports `JNI_OnLoad` but the runtime registry has
    /// no JNI session (ticket 52): calling it without a `JavaVM*` would be
    /// a fake bring-up, so this fails instead.
    JniSessionMissing { module: &'static str },
    /// The guest module exports `JNI_OnLoad` but the deployed
    /// `android_elf_load` has no executable-mapping backend (Linux only,
    /// ticket 49): its code cannot run, so the call cannot be made.
    JniExecBackendMissing { module: &'static str },
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
            GuestError::Symbol { capability, reason } => write!(
                f,
                "capability {capability} misses its C API: {reason}"
            ),
            GuestError::LoadFailed { path, required: true, reason } => write!(
                f,
                "required guest module {} cannot load: {reason}; \
                 verify the gamedata layout (lib/arm64-v8a/...)",
                path.display()
            ),
            GuestError::LoadFailed { path, required: false, reason } => write!(
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

/// Register a callback for one module kind. Never unregistered — the loader
/// pins everything for process life.
pub fn register_module_loaded(kind: ModuleKind, callback: ModuleCallback) {
    CALLBACKS.lock().unwrap().push((kind, callback));
}

fn fire(event: &ModuleEvent) {
    let callbacks = CALLBACKS.lock().unwrap();
    for (kind, callback) in callbacks.iter() {
        if *kind == event.kind {
            callback(event);
        }
    }
    drop(callbacks);
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

/// C API of the android_elf_load capability, resolved once per bring-up.
///
/// Ownership (ticket 46): mapped guest modules stay mapped for the process
/// lifetime — guest code may already have been entered by the time this
/// struct is dropped, so `Drop` intentionally never calls the capability's
/// destroy entry. The loader pins the owning capability library for the
/// process life, so the stored function pointers can never dangle.
struct ElfLoadApi {
    from_path: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    /// Image base / load bias of a mapped module (ticket 48): module events
    /// carry this, never the opaque handle address.
    load_base: unsafe extern "C" fn(*const c_void) -> usize,
    symbol: unsafe extern "C" fn(*const c_void, *const c_char) -> usize,
    // Used by the hook/JNI_OnLoad wiring on the guest bring-up path
    // (crates/docs/ACCEPTANCE.md); not exercised by every test build yet.
    #[allow(dead_code)]
    destroy: unsafe extern "C" fn(*mut c_void),
    /// Ticket 65 error channel (`rudroid_elf_last_error`), required since
    /// ticket 72. Kits are same-build only (ticket 57): this library already
    /// passed the loader's `runtime_build_id` check, so a member without the
    /// channel cannot be a legitimate deployment — resolving it like every
    /// other C API entry keeps ticket 65 diagnostics from silently degrading
    /// exactly on the broken kits where they matter most.
    last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
    /// Ticket 49 executable backend (Linux only), resolved tolerantly: the
    /// C exports are `#[cfg(target_os = "linux")]`, so non-Linux builds of
    /// the same capability legitimately lack them. `None` + a module that
    /// exports `JNI_OnLoad` is a hard bring-up error, never a silent skip.
    exec_load: Option<unsafe extern "C" fn(*const c_char) -> *mut c_void>,
    exec_symbol: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    /// Reference `so_symbol_all` (dynsym, then the module file's `.symtab`),
    /// resolved tolerantly; backs the Host API `so_symbol_all` slot.
    exec_symbol_all: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    /// Ticket 84: image base of an exec mapping — the single source of the
    /// guest address identity (event base == execution base).
    exec_load_base: Option<unsafe extern "C" fn(*const c_void) -> usize>,
    /// Buffer-backend fallback handles (non-Linux / capability without the
    /// exec backend). On the exec path nothing is loaded here: kinded
    /// modules and support libraries get exactly ONE mapping (ticket 84).
    handles: Vec<*mut c_void>,
    /// Exec handles pinned for the process (also tracked base→handle in the
    /// runtime table; the pin here covers the window before registration).
    exec_handles: Vec<*mut c_void>,
}

impl Drop for ElfLoadApi {
    fn drop(&mut self) {
        // Intentional no-op: see the type docs. Handles leak by design;
        // the OS reclaims the mappings at process exit.
    }
}

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
    // Data prerequisites first: a wrong gamedata directory is the config
    // contract failing, which is more actionable than a kit composition
    // error (ticket 44).
    if !plan.game_files.is_dir() {
        return Err(GuestError::GameFilesMissing {
            dir: plan.game_files.clone(),
            key: "paths.game_files",
        });
    }

    let capability = deployed
        .iter()
        .find(|c| c.descriptor.id == "android_elf_load")
        .ok_or(GuestError::ElfLoadCapabilityMissing)?;

    // SAFETY: the capability handle is pinned by the loader; the C API
    // symbols follow the documented contract in rudroid_common::symbols.
    let mut api = unsafe { resolve_api(capability.library()) }?;

    let mut bionic_source = install_bionic_source(deployed, capability.library())?;
    let mut media_source = install_native_source(deployed, capability.library(), "android_media_fallback",
        c"rudroid_media_fallback_symbol", c"ndk-media-unavailable")?;
    let mut events_source = install_native_source(deployed, capability.library(), "android_native_events",
        c"rudroid_native_events_symbol", c"ndk-native-events")?;
    let mut zlib_source = install_native_source(deployed, capability.library(), "android_zlib",
        c"rudroid_zlib_symbol", c"guest-zlib")?;
    let mut assets_source = install_native_source(deployed, capability.library(), "android_assets",
        c"rudroid_assets_symbol", c"ndk-assets")?;

    let mut events = Vec::new();
    // A real Unity libmain's NativeLoader.load owns engine JNI initialization.
    // Map/publish the libraries here, but let that native loader invoke their
    // JNI_OnLoad once, in its own order. Synthetic kits without libunity retain
    // direct initialization; addon JNI entry points remain loader-owned.
    let native_loader_owns_engine_jni = plan.game_files.join("lib/arm64-v8a/libunity.so").is_file();
    for entry in &plan.entries {
        // Ticket 56: reference-order gating (the addon plugins act only on
        // the Unity module-loaded callback; fmodstudio needs a usable fmod
        // JNI_OnLoad). A gated entry is not attempted and says so.
        if !gates_satisfied(entry.gates, &events) {
            let gates = entry
                .gates
                .iter()
                .map(|gate| gate.name())
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
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
            eprintln!(
                "[GUEST] optional module {} absent; skipping (non-fatal)",
                path.display()
            );
            continue;
        }
        let c_path = std::ffi::CString::new(
            path.to_str().ok_or_else(|| GuestError::LoadFailed {
                path: path.clone(),
                required: entry.required,
                reason: "path is not valid UTF-8".to_owned(),
            })?,
        )
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
                // SAFETY: pinned capability entries; c_path is NUL-free.
                let exec_handle = unsafe { (exec_load)(c_path.as_ptr()) };
                if exec_handle.is_null() {
                    return Err(GuestError::LoadFailed {
                        path,
                        required: entry.required,
                        reason: unsafe { fetch_last_error(&api) }.unwrap_or_else(|| {
                            "android_elf_load exec backend cannot map the module"
                                .to_owned()
                        }),
                    });
                }
                api.exec_handles.push(exec_handle);
                let base = unsafe { (exec_load_base)(exec_handle) };
                let Some(kind) = entry.kind else {
                    // Support library: participates in the exec image world
                    // (and, from ticket 86, the symbol scope) but fires no
                    // event and has no JNI stage.
                    eprintln!(
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
                eprintln!(
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
                    if native_loader_owns_engine_jni && matches!(kind, ModuleKind::Unity | ModuleKind::Il2cpp) {
                        eprintln!("[GUEST] JNI_OnLoad for {} deferred to NativeLoader.load (single initialization)", entry.relative_path);
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
                let event = ModuleEvent { jni_on_load, ..event };
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
                api.handles.push(handle);
                let Some(kind) = entry.kind else { continue };
                // The real image base — the handle itself is an opaque
                // pointer and must never be reported as the base (ticket 48).
                let base = unsafe { (api.load_base)(handle) };
                eprintln!(
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
                let jni_on_load = unsafe {
                    call_jni_on_load(
                        &api,
                        handle,
                        None,
                        None,
                        entry.relative_path,
                    )?
                };
                let event = ModuleEvent { jni_on_load, ..event };
                events.push(event);
            }
        }
    }
    // Ownership is documented on ElfLoadApi: dropping here pins the handles
    // for the process life instead of destroying them (ticket 46).
    drop(api);
    if let Some(source) = &mut bionic_source { source.id = -1; }
    if let Some(source) = &mut media_source { source.id = -1; }
    if let Some(source) = &mut zlib_source { source.id = -1; }
    if let Some(source) = &mut events_source { source.id = -1; }
    if let Some(source) = &mut assets_source { source.id = -1; }
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
            unsafe { (self.unregister)(self.id); }
        }
    }
}

fn install_bionic_source(deployed: &[LoadedCapability], elf: &Library) -> Result<Option<PendingNativeSource>, GuestError> {
    let Some(bionic) = deployed.iter().find(|c| c.descriptor.id == "android_bionic") else {
        return Ok(None);
    };
    use rudroid_common::symbols::elf_exec::{RangeFinalizer, VersionedSymbolSource};
    // SAFETY: same-build libraries are pinned; the exported signatures are
    // the common ABI. The stateless source accepts concurrent queries.
    unsafe {
        let source: libloading::Symbol<VersionedSymbolSource> = bionic.library()
            .get(b"rudroid_bionic_symbol")
            .map_err(|e| GuestError::Symbol { capability: "android_bionic", reason: e.to_string() })?;
        let finalizer: libloading::Symbol<RangeFinalizer> = bionic.library()
            .get(b"rudroid_bionic_finalize_range")
            .map_err(|e| GuestError::Symbol { capability: "android_bionic", reason: e.to_string() })?;
        let register: libloading::Symbol<unsafe extern "C" fn(
            *const c_char, Option<VersionedSymbolSource>, *mut c_void, Option<RangeFinalizer>,
        ) -> c_int> = elf.get(symbols::elf_exec::SOURCE_REGISTER_FINALIZED.as_bytes())
            .map_err(|e| GuestError::Symbol { capability: "android_elf_load", reason: e.to_string() })?;
        let unregister: libloading::Symbol<unsafe extern "C" fn(c_int) -> c_int> = elf
            .get(symbols::elf_exec::SOURCE_UNREGISTER.as_bytes())
            .map_err(|e| GuestError::Symbol { capability: "android_elf_load", reason: e.to_string() })?;
        let id = register(c"bionic-native".as_ptr(), Some(*source), std::ptr::null_mut(), Some(*finalizer));
        if id <= 0 {
            return Err(GuestError::Symbol { capability: "android_elf_load", reason: "rejected the version-aware Bionic source".into() });
        }
        eprintln!("[BIONIC] version-aware native ABI source registered before guest mapping");
        Ok(Some(PendingNativeSource { id, unregister: *unregister }))
    }
}

fn install_native_source(deployed: &[LoadedCapability], elf: &Library, capability_name: &'static str,
    provider_name: &std::ffi::CStr, source_name: &std::ffi::CStr,
) -> Result<Option<PendingNativeSource>, GuestError> {
    let Some(capability) = deployed.iter().find(|c| c.descriptor.id == capability_name) else {
        return Ok(None);
    };
    use rudroid_common::symbols::elf_exec::VersionedSymbolSource;
    // SAFETY: same-kit libraries and callbacks are process-pinned. The source
    // only accepts its explicit ABI names and retains no user pointer.
    unsafe {
        let source: libloading::Symbol<VersionedSymbolSource> = capability.library()
            .get(provider_name.to_bytes())
            .map_err(|e| GuestError::Symbol { capability: capability_name, reason: e.to_string() })?;
        let register: libloading::Symbol<unsafe extern "C" fn(
            *const c_char, Option<VersionedSymbolSource>, *mut c_void,
        ) -> c_int> = elf.get(symbols::elf_exec::SOURCE_REGISTER_VERSIONED.as_bytes())
            .map_err(|e| GuestError::Symbol { capability: "android_elf_load", reason: e.to_string() })?;
        let unregister: libloading::Symbol<unsafe extern "C" fn(c_int) -> c_int> = elf
            .get(symbols::elf_exec::SOURCE_UNREGISTER.as_bytes())
            .map_err(|e| GuestError::Symbol { capability: "android_elf_load", reason: e.to_string() })?;
        let id = register(source_name.as_ptr(), Some(*source), std::ptr::null_mut());
        if id <= 0 {
            return Err(GuestError::Symbol { capability: "android_elf_load", reason: format!("rejected native ABI source {capability_name}") });
        }
        eprintln!("[NATIVE] {capability_name} ABI source registered before guest mapping");
        Ok(Some(PendingNativeSource { id, unregister: *unregister }))
    }
}

unsafe fn resolve_api(library: &Library) -> Result<ElfLoadApi, GuestError> {
    let from_path: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_void> =
        library
            .get(symbols::elf_load::LOAD_FROM_PATH.as_bytes())
            .map_err(|e| GuestError::Symbol {
                capability: "android_elf_load",
                reason: e.to_string(),
            })?;
    let load_base: libloading::Symbol<unsafe extern "C" fn(*const c_void) -> usize> =
        library
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
    let last_error: libloading::Symbol<unsafe extern "C" fn(*mut c_char, usize) -> usize> =
        library
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
    let exec_load_base: Option<
        libloading::Symbol<unsafe extern "C" fn(*const c_void) -> usize>,
    > = library.get(symbols::elf_exec::EXEC_LOAD_BASE.as_bytes()).ok();
    let exec_symbol_all: Option<
        libloading::Symbol<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    > = library.get(symbols::elf_exec::EXEC_SYMBOL_ALL.as_bytes()).ok();
    Ok(ElfLoadApi {
        from_path: *from_path,
        load_base: *load_base,
        symbol: *symbol,
        destroy: *destroy,
        exec_load_base: exec_load_base.map(|s| *s),
        exec_handles: Vec::new(),
        last_error: *last_error,
        exec_load: exec_load.map(|s| *s),
        exec_symbol: exec_symbol.map(|s| *s),
        exec_symbol_all: exec_symbol_all.map(|s| *s),
        handles: Vec::new(),
    })
}

/// Fetch the capability's thread-local diagnostic for the just-failed call
/// (ticket 65). Two-phase: query the needed length, then fetch exactly that.
/// SAFETY: `last_error` points into the pinned capability library.
unsafe fn fetch_last_error(api: &ElfLoadApi) -> Option<String> {
    let last_error = api.last_error;
    let need = unsafe { last_error(std::ptr::null_mut(), 0) };
    if need == 0 {
        return None;
    }
    let mut buffer = vec![0u8; need];
    unsafe { last_error(buffer.as_mut_ptr() as *mut c_char, need) };
    buffer.truncate(need.saturating_sub(1)); // drop the NUL terminator
    String::from_utf8(buffer).ok()
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
    module: &'static str,
) -> Result<Option<i32>, GuestError> {
    let c_name = std::ffi::CString::new("JNI_OnLoad").expect("literal is NUL-free");
    // SAFETY: live exec mapping; NUL-free name.
    let entry = unsafe { exec_symbol(exec_handle, c_name.as_ptr()) };
    if entry == 0 {
        return Ok(None);
    }
    let vm = super::runtime::shared_vm();
    if vm.is_null() {
        return Err(GuestError::JniSessionMissing { module });
    }
    eprintln!(
        "[GUEST] calling JNI_OnLoad from {module} (shared JavaVM registered)"
    );
    // SAFETY: the exec symbol resolved above is the module's JNI_OnLoad.
    let entry: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(entry) };
    let returned = unsafe { entry(vm, std::ptr::null_mut()) };
    eprintln!("[GUEST] JNI_OnLoad from {module} returned {returned:#x}");
    Ok(Some(returned))
}

/// # Safety
/// `handle` must be a live data mapping from `api.from_path` whose code
/// bytes are the guest's; `exec_handle` (when present) maps the same file
/// for execution.
unsafe fn call_jni_on_load(
    api: &ElfLoadApi,
    handle: *mut c_void,
    exec_handle: Option<*mut c_void>,
    exec_symbol: Option<unsafe extern "C" fn(*const c_void, *const c_char) -> usize>,
    module: &'static str,
) -> Result<Option<i32>, GuestError> {
    // Cheap data-view probe first: no export, no exec call needed.
    if unsafe { export_symbol(api, handle, "JNI_OnLoad") }.is_none() {
        return Ok(None);
    }
    let (Some(exec_handle), Some(exec_symbol)) = (exec_handle, exec_symbol) else {
        return Err(GuestError::JniExecBackendMissing { module });
    };
    let vm = super::runtime::shared_vm();
    if vm.is_null() {
        return Err(GuestError::JniSessionMissing { module });
    }
    eprintln!(
        "[GUEST] calling JNI_OnLoad from {module} (shared JavaVM registered)"
    );
    // Process-pinned like every live guest mapping (ticket 46): the entry
    // point may have registered callbacks; the handle is never destroyed.
    let found = unsafe { exec_symbol_fn(exec_symbol, exec_handle, "JNI_OnLoad") };
    let Some(entry) = found else {
        return Err(GuestError::LoadFailed {
            path: PathBuf::from(module),
            required: true,
            reason: "JNI_OnLoad is exported in the data view but not in the \
                     executable mapping"
                .to_owned(),
        });
    };
    let entry: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(entry) };
    let returned = unsafe { entry(vm, std::ptr::null_mut()) };
    eprintln!("[GUEST] JNI_OnLoad from {module} returned {returned:#x}");
    Ok(Some(returned))
}

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

/// Same lookup on the executable mapping, returning the callable address.
///
/// # Safety
/// `handle` must be a live exec mapping from `exec_symbol`'s own capability.
unsafe fn exec_symbol_fn(
    exec_symbol: unsafe extern "C" fn(*const c_void, *const c_char) -> usize,
    handle: *mut c_void,
    name: &str,
) -> Option<usize> {
    let Ok(c_name) = std::ffi::CString::new(name) else {
        return None;
    };
    let address = unsafe { (exec_symbol)(handle, c_name.as_ptr()) };
    (address != 0).then_some(address)
}
