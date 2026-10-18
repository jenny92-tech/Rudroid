//! INPUT:  one `GuestModule` description, the loader Host API table and Unity/own module events
//! OUTPUT:  `plugin_init`, the observed `MappingOutcome` and its test reset hook
//! POS:    the single post-Unity guest-library chain shared by the Burst, Unity Helpers and XLua addons
//!
//! Named after the reference `AfterUnityGuestModule`: a game `.so` that is
//! mapped after `libunity.so`. Rust counterpart of the read-only
//! `sdk_unity_burst` / `sdk_unity_helpers` / `sdk_xlua` plugins (tickets
//! 20–22, real load chain ticket 56). Each
//! addon cdylib links its own copy of this library, so the statics below
//! are per addon; the addons differ only in their [`GuestModule`] data.
//!
//! 1. `plugin_init` subscribes to the Unity module event (the reference
//!    `AfterUnityGuestModule::on_unity_loaded` trigger) and to the addon's
//!    own module event (the kit loads the library through the
//!    `android_elf_load` ELF provider and fires it with the real image base).
//! 2. On the Unity event the addon checks the guest file under the Host
//!    Config `paths.game_files`: absent means an explicit log and
//!    `SkippedMissing` (never reported as loaded).
//! 3. On its own event the addon verifies the load identity (real base and
//!    planned path) and only then reports `Mapped`; repeated events are
//!    idempotent (the reference `attempted_` flag).
//!
//! A guest file that is present but broken fails the loader bring-up
//! explicitly through the ELF provider's diagnostics (the addon stays
//! `Pending` and the run never reaches a fake success).
#![cfg_attr(not(test), forbid(unsafe_code))]

use std::ffi::{c_int, c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rudroid_common::handles::HandleTable;
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::ModuleKind;

/// What one single-library addon maps after the Unity module loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestModule {
    /// Library path relative to `paths.game_files` (also the module event name).
    pub library: &'static str,
    /// Host-log tag (the C++ plugin's tag).
    pub log_tag: &'static str,
    /// The addon's own module event kind.
    pub kind: ModuleKind,
    /// Name used for the own subscription in the rejection log line.
    pub label: &'static str,
}

/// Outcome of the post-Unity mapping (ticket 56: every value reflects an
/// observed loader event or a real file check, never a bool argument).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingOutcome {
    /// The Unity module event has not been observed yet.
    Pending,
    /// Unity loaded but the guest library file is absent under
    /// `paths.game_files`: explicitly logged and skipped — the honest
    /// non-loaded state (C++ AfterUnityGuestModule logs the missing module).
    SkippedMissing,
    /// The addon's own module event fired with a real ELF-provider load
    /// (non-zero image base, planned relative path).
    Mapped,
    /// The module event fired but the load contract failed; the reason names
    /// the broken invariant.
    Failed { reason: &'static str },
}

/// Handler state behind the subscription `user` handle; kept in [`STATES`]
/// for the process like the reference plugin handle.
pub(crate) struct AddonState {
    pub(crate) api: HostApiV0,
    pub(crate) module: &'static GuestModule,
}

/// Every armed state; entries are never removed.
pub(crate) static STATES: HandleTable<AddonState> = HandleTable::new();

/// First Unity handling happened (idempotency across repeated Unity events).
static UNITY_SEEN: AtomicBool = AtomicBool::new(false);
/// First own-module handling happened (the reference `attempted_` flag).
static MAPPED_ATTEMPTED: AtomicBool = AtomicBool::new(false);
/// The observed outcome, readable from tests and diagnostics.
static OUTCOME: Mutex<Option<MappingOutcome>> = Mutex::new(None);

fn set_outcome(outcome: MappingOutcome) {
    *OUTCOME.lock().unwrap() = Some(outcome);
}

/// The outcome this addon last observed (`None` before plugin_init).
pub fn last_outcome() -> Option<MappingOutcome> {
    OUTCOME.lock().unwrap().clone()
}

/// Test isolation hook: clears the process-local state so an in-process
/// suite can drive the handlers repeatedly (the loader initializes a cdylib
/// exactly once in production).
#[doc(hidden)]
pub fn reset_state_for_tests() {
    UNITY_SEEN.store(false, Ordering::SeqCst);
    MAPPED_ATTEMPTED.store(false, Ordering::SeqCst);
    *OUTCOME.lock().unwrap() = None;
}

fn api_log(state: &AddonState, message: &str) {
    log_with(&state.api, state.module.log_tag, message);
}

fn log_with(api: &HostApiV0, tag: &str, message: &str) {
    let Ok(tag) = CString::new(tag) else {
        return;
    };
    api.log_line(&tag, message);
}

/// The Unity module event (the reference `on_unity_loaded` trigger): check
/// the guest file's presence under `paths.game_files` and log the honest
/// outcome. Repeated Unity events are ignored (the oracle `attempted_`
/// model — the load attempt happens once).
pub(crate) extern "C" fn on_unity_loaded(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(state)) = (event, STATES.get(user)) else {
        return;
    };
    let state = &*state;
    if event.kind != ModuleKind::Unity as u32 {
        return;
    }
    if UNITY_SEEN.swap(true, Ordering::SeqCst) {
        api_log(state, "repeated unity module event; already attempted (idempotent)");
        return;
    }
    let Some(game_files) = state.api.config_string(c"paths.game_files", 512).filter(|value| !value.is_empty()) else {
        api_log(
            state,
            "paths.game_files not configured; guest library presence unknown \
             (staying pending)",
        );
        return;
    };
    let library = state.module.library;
    if PathBuf::from(&game_files).join(library).is_file() {
        api_log(state, &format!("guest library present, awaiting its module event: {library}"));
    } else {
        set_outcome(MappingOutcome::SkippedMissing);
        api_log(state, &format!("guest module is missing: {library} (logged and skipped, non-fatal)"));
    }
}

/// The addon's own module event: the loader mapped the library through the
/// ELF provider and fires the event with the real image base. Only this
/// observation — never a bool — turns the state `Mapped`.
pub(crate) extern "C" fn on_module_loaded(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(state)) = (event, STATES.get(user)) else {
        return;
    };
    let state = &*state;
    if event.kind != state.module.kind as u32 {
        return;
    }
    if MAPPED_ATTEMPTED.swap(true, Ordering::SeqCst) {
        api_log(state, "repeated module event; already initialized (idempotent, oracle attempted_)");
        return;
    }
    if event.base == 0 {
        set_outcome(MappingOutcome::Failed { reason: "module event reported no load base" });
        api_log(state, "FAILED: module event reported no load base");
        return;
    }
    let reported = event.name.to_cstr().map(|name| name.to_string_lossy().into_owned());
    let library = state.module.library;
    if reported.as_deref() != Some(library) {
        set_outcome(MappingOutcome::Failed { reason: "module event named an unexpected path" });
        api_log(state, &format!("FAILED: module event named {reported:?}, expected {library:?}"));
        return;
    }
    set_outcome(MappingOutcome::Mapped);
    api_log(
        state,
        &format!("module loaded through the ELF provider: {library} base={:#x}", event.base),
    );
}

/// Arm `module`: subscribe to the Unity event and the addon's own event.
pub fn plugin_init(api: &HostApiV0, module: &'static GuestModule) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let user = STATES.insert(AddonState { api: *api, module });
    // Logged as 0 (accepted) / 1 (rejected): the host's own code is not surfaced.
    let code = |accepted: bool| c_int::from(!accepted);
    let registered = code(api.on_module_loaded(ModuleKind::Unity, on_unity_loaded, user));
    let registered_own = code(api.on_module_loaded(module.kind, on_module_loaded, user));
    if registered != 0 || registered_own != 0 {
        log_with(
            api,
            module.log_tag,
            &format!(
                "module subscription rejected (unity={registered}, {}={registered_own}) \
                 — not armed",
                module.label
            ),
        );
        return -3;
    }
    log_with(api, module.log_tag, "armed: waiting for the unity module event (ticket 56 real-load chain)");
    0
}

/// `rudroid_plugin_init` body for an addon: -1 for a null table.
pub fn plugin_init_raw(api: Option<&HostApiV0>, module: &'static GuestModule) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    plugin_init(api, module)
}

/// Serialize the process-global-state tests (the loader drives one instance
/// per process; the in-process suite resets around them).
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
