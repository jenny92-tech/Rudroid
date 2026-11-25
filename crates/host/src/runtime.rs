//! INPUT:  common registry/Host API contracts and process-pinned plugin callbacks
//! OUTPUT:  shared registrations, module/lifecycle events, guest symbol lookup and shutdown
//! POS:    owns the kit's authoritative services and invokes callbacks outside registry locks
//! Process-wide runtime registry (ticket 45).
//!
//! The host owns ONE registry; capabilities register providers, profiles,
//! policies and module subscriptions through the Host API table, and guest
//! bring-up fires module events back through it. This is the shared state of
//! the whole kit — capability-local statics could never share it (each
//! cdylib has its own copies), which is exactly what the C ABI table is for.
//!
//! Locking rules: every registration and query takes the registry mutex
//! briefly; callbacks are collected under the lock but INVOKED OUTSIDE it, so
//! a callback may re-enter the registry (query, subscribe) without deadlocks.
//! Guest symbol lookup likewise copies its process-pinned backend/handle
//! under the module-table lock and calls the backend after releasing it.
//! Subscriptions added while a fire is running take effect from the next
//! fire. `shutdown` clears everything — the bring-up path calls it when a
//! phase fails so no registered callback can fire after a failed init.
//!
//! Provider lifetime (tickets 61/66): a catalog query leases the provider
//! under the lock (in-flight count), calls it outside the lock, and releases
//! the lease afterwards. `shutdown` marks the slot draining; the destroy only
//! runs when the last in-flight query has returned — either inside `shutdown`
//! itself (no query active) or in the last query's epilogue. `shutdown` never
//! waits, so a callback may call it; queries arriving after shutdown miss
//! cleanly. The slot is an explicit state machine (`Open`/`Draining`/
//! `Destroying`/`Vacant`): while the old generation drains or its `destroy`
//! callback has not returned yet, a new provider registration is
//! deterministically rejected (`REGISTRY_REJECTED`, the caller keeps
//! ownership) — the destroy runs outside the registry lock (it may re-enter)
//! and only its RETURN reopens the slot for the next generation.
//!
//! Provider threading contract (ticket 70, decision A): query callbacks are
//! invoked OUTSIDE the registry lock and therefore CONCURRENTLY — two
//! threads can be inside the same provider callback at once, and the
//! documented `CatalogProviderV0` ABI requires providers to tolerate that.
//! The lease handoff also orders memory: the destroy runs only after every
//! in-flight query callback returned, and the registry lock makes those
//! callbacks' writes visible to it. No per-provider serialization exists on
//! the host side; a provider needing exclusivity must bring its own
//! (re-entrant-safe) synchronization.

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::{Arc, Mutex};
use std::sync::OnceLock;

use rudroid_common::jni_env::JavaVm;

use rudroid_common::registry::{
    RudroidAfterMainJniCallback, RudroidAfterMainJniContext, RudroidAndroidHooks,
    RudroidAudioPolicy, RudroidModuleEvent, RudroidModuleLoadedCallback,
    RudroidEngineProfile, CatalogProviderV0, HostStr,
};

const REGISTRY_REJECTED: c_int = 1;
const NULL_ARG: c_int = 2;

#[derive(Default)]
struct RegistryState {
    module_subscriptions: Arc<Vec<ModuleSubscription>>,
    after_main_jni: Arc<Vec<AfterMainSubscription>>,
    profile: Option<OwnedProfile>,
    hooks: Option<RudroidAndroidHooks>,
    audio_policy: Option<RudroidAudioPolicy>,
    catalog: CatalogSlot,
    shared_vm: Option<JavaVm>,
}

/// A profile copy; strings stay as pointers into the pinned registrant.
struct OwnedProfile {
    inner: RudroidEngineProfile,
}

/// One module-event subscription. The registrant keeps `callback`/`user`
/// valid for process life (registration contract); the host only uses the
/// pair to fire events outside the registry lock — possibly from several
/// firing threads at once, as with catalog providers.
#[derive(Clone, Copy)]
struct ModuleSubscription {
    /// Event discriminant matched against fired events; written once at
    /// registration, read under the registry lock.
    kind: u32,
    /// Plain C function pointer (fn pointers are `Send` by definition).
    callback: RudroidModuleLoadedCallback,
    /// Opaque registrant state passed back verbatim; never dereferenced or
    /// freed by the host.
    user: *mut c_void,
}

// SAFETY (per field, ticket 70): `kind` is a plain integer; `callback` is a
// C function pointer (`Send` by definition); `user` is opaque registrant
// state the host never dereferences or frees — it only calls
// `callback(event, user)` outside all registry locks, per the module-event
// contract whose registrants must tolerate concurrent fires (as with
// catalog providers). All host-side table access is serialized by the
// registry mutex; `Send` is what the process-global `Mutex<RegistryState>`
// needs to be shareable across threads.
unsafe impl Send for ModuleSubscription {}

/// One opt-in post-Main-JNI subscription. The registrant pins the callback
/// and `user` for process life; the host only passes them back verbatim.
#[derive(Clone, Copy)]
struct AfterMainSubscription {
    callback: RudroidAfterMainJniCallback,
    user: *mut c_void,
}

// SAFETY: the callback is a plain C function pointer and `user` is opaque
// registrant state the host never dereferences or frees. The Arc snapshot is
// copied under the registry lock; invocation happens outside it.
unsafe impl Send for AfterMainSubscription {}

// SAFETY (per field, ticket 70): every field is touched only under the
// registry mutex, and raw pointers are used exactly as the documented
// contracts allow:
// - `module_subscriptions`: entries are registered/cleared and Arc snapshots copied under the
//   lock; `callback`/`user` are only invoked per the module-event contract
//   (see `ModuleSubscription`).
// - `profile`/`hooks`/`audio_policy`: POD copies whose string pointers stay
//   process-lifetime inside the pinned registrant; the host never frees
//   them and the accessors hand out plain copies under the lock.
// - `catalog`: `CatalogSlot` holds the provider copy and lease count;
//   callbacks follow the `CatalogProviderV0` threading contract (concurrent
//   queries allowed, destroy only after the last query callback returned).
// - `shared_vm`: opaque pointer stored/read only under the lock, never
//   dereferenced by the host.
//
// `Send` is required for the `OnceLock<Mutex<RegistryState>>` static to be
// shareable. There is deliberately NO `Sync` impl: nothing ever obtains a
// shared reference to `RegistryState` without the mutex, and not having the
// impl keeps that impossible by construction instead of by justification.
unsafe impl Send for RegistryState {}

static REGISTRY: OnceLock<Mutex<RegistryState>> = OnceLock::new();
static HAS_FRAME_SUBSCRIBERS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn registry() -> &'static Mutex<RegistryState> {
    REGISTRY.get_or_init(|| Mutex::new(RegistryState {
        shared_vm: None,
        ..Default::default()
    }))
}

// ---------------------------------------------------------------- module events

/// Register a module-loaded callback for one `rudroid_common::ModuleKind`
/// discriminant. Returns 0 on success.
pub fn register_module_loaded(
    kind: u32,
    callback: RudroidModuleLoadedCallback,
    user: *mut c_void,
) -> c_int {
    let mut state = registry().lock().unwrap();
    Arc::make_mut(&mut state.module_subscriptions).push(ModuleSubscription {
        kind,
        callback,
        user,
    });
    if kind == rudroid_common::ModuleKind::UnityFrame as u32 {
        HAS_FRAME_SUBSCRIBERS.store(true, std::sync::atomic::Ordering::Release);
    }
    0
}

/// Register a package-selected capability's post-Main-JNI hook. Multiple
/// registrations run in order; the return is the first non-zero result.
pub fn register_after_main_jni(
    callback: RudroidAfterMainJniCallback,
    user: *mut c_void,
) -> c_int {
    let mut state = registry().lock().unwrap();
    Arc::make_mut(&mut state.after_main_jni).push(AfterMainSubscription { callback, user });
    0
}

/// Invoke the post-Main-JNI hooks outside every registry lock. Zero means no
/// hook rejected the point; a non-zero callback result aborts guest bring-up.
pub fn invoke_after_main_jni(context: &RudroidAfterMainJniContext) -> c_int {
    let subscriptions = {
        let state = registry().lock().unwrap();
        state.after_main_jni.clone()
    };
    for subscription in subscriptions.iter() {
        let result = (subscription.callback)(Some(context), subscription.user);
        if result != 0 {
            return result;
        }
    }
    0
}

/// Fire a module event to every subscription matching `event.kind`. The
/// registry lock is released before any callback runs.
pub fn fire_module_event(event: &RudroidModuleEvent) {
    let targets = {
        let state = registry().lock().unwrap();
        Arc::clone(&state.module_subscriptions)
    };
    for sub in targets.iter().filter(|sub| sub.kind == event.kind) {
        (sub.callback)(Some(event), sub.user);
    }
}

/// Capability-sourced lifecycle broadcast (ticket 78): the only kinds a
/// capability may fire are the runtime pseudo-events — guest module-load
/// events stay loader-only, so a capability cannot spoof a mapping. The
/// event identity is a fixed literal; consumers carry the module binding
/// they stored at the Mapped event, not a base from here.
pub fn fire_runtime_event(kind: u32) -> c_int {
    use rudroid_common::ModuleKind;
    let name = match ModuleKind::from_u32(kind) {
        Some(ModuleKind::Il2cppReady) => c"runtime/il2cpp_ready",
        Some(ModuleKind::UnityMaintenance) => c"runtime/unity_maintenance",
        Some(ModuleKind::UnityStopping) => c"runtime/unity_stopping",
        Some(ModuleKind::UnityFrame) => {
            // No lock, snapshot allocation or callback work without a listener.
            if !HAS_FRAME_SUBSCRIBERS.load(std::sync::atomic::Ordering::Acquire) {
                return 0;
            }
            c"runtime/unity_frame"
        }
        _ => return REGISTRY_REJECTED,
    };
    let event = RudroidModuleEvent {
        kind,
        base: 0,
        name: rudroid_common::registry::HostStr::new(name),
    };
    fire_module_event(&event);
    0
}

// ---------------------------------------------------------------- singular registrations

/// Copy a caller-built POD (its strings are `HostStr<'static>` into the
/// pinned library) into a first-write-wins slot.
fn register_pod<T: Copy>(slot: &mut Option<T>, value: Option<&T>) -> c_int {
    let Some(value) = value else {
        return NULL_ARG;
    };
    if slot.is_some() {
        return REGISTRY_REJECTED;
    }
    *slot = Some(*value);
    0
}

pub fn register_engine_profile(profile: Option<&RudroidEngineProfile>) -> c_int {
    let mut state = registry().lock().unwrap();
    let Some(profile) = profile else {
        return NULL_ARG;
    };
    if state.profile.is_some() {
        return REGISTRY_REJECTED;
    }
    state.profile = Some(OwnedProfile { inner: *profile });
    0
}

pub fn register_android_hooks(hooks: Option<&RudroidAndroidHooks>) -> c_int {
    let mut state = registry().lock().unwrap();
    register_pod(&mut state.hooks, hooks)
}

pub fn register_audio_policy(policy: Option<&RudroidAudioPolicy>) -> c_int {
    let mut state = registry().lock().unwrap();
    register_pod(&mut state.audio_policy, policy)
}

pub fn register_shared_vm(vm: JavaVm) -> c_int {
    let mut state = registry().lock().unwrap();
    if state.shared_vm.is_some() {
        return REGISTRY_REJECTED;
    }
    state.shared_vm = Some(vm);
    0
}

pub fn shared_vm() -> *mut c_void {
    registry().lock().unwrap().shared_vm.map_or(std::ptr::null_mut(), JavaVm::as_raw)
}

// ---------------------------------------------------------------- catalog queries

// ---------------------------------------------------------------- accessors + shutdown

/// Read access for the loader-side consumers (profile runner, audio backend).
pub fn registered_profile() -> Option<RudroidEngineProfile> {
    registry().lock().unwrap().profile.as_ref().map(|p| p.inner)
}

pub fn registered_audio_policy() -> Option<RudroidAudioPolicy> {
    registry().lock().unwrap().audio_policy
}

pub fn registered_android_hooks() -> Option<RudroidAndroidHooks> {
    registry().lock().unwrap().hooks
}

/// Clear every registration. Called on the bring-up failure path so no
/// callback can fire after a failed init: the subscription table is empty
/// and the provider slot is released. Callbacks already running are not
/// interrupted; in production the process exits right after.
///
/// Provider teardown (tickets 61/66): the slot is marked draining and the
/// catalog provider is destroyed immediately only when no query is in
/// flight; otherwise the last in-flight query's epilogue destroys it
/// (queries arriving from now on miss). `shutdown` never waits, so it is
/// safe to call from inside a query callback or from inside a destroy
/// callback. The provider is destroyed exactly once, the destroy runs
/// outside the registry lock, and until it RETURNS any new provider
/// registration is rejected (`REGISTRY_REJECTED`, caller keeps ownership) —
/// a provider registered after the drain is never touched by this drain.
pub fn shutdown() {
    super::host_api::clear_service_and_overlay_registries();
    super::guest::clear_callbacks();
    let to_destroy = {
        let mut state = registry().lock().unwrap();
        state.module_subscriptions = Arc::new(Vec::new());
        state.after_main_jni = Arc::new(Vec::new());
        HAS_FRAME_SUBSCRIBERS.store(false, std::sync::atomic::Ordering::Release);
        state.profile = None;
        state.hooks = None;
        state.audio_policy = None;
        state.shared_vm = None;
        begin_shutdown(&mut state.catalog)
    };
    if let Some(provider) = to_destroy {
        // Destroy outside the registry lock, mirroring query epilogues; the
        // slot reopens for the next generation only after it returned.
        destroy_provider_and_reopen(provider);
    }
    // Ticket 53: a failed bring-up leaves no prepared session and no guest
    // exec table behind, so a later bring-up in the same process starts
    // from a clean slate.
    profile::clear();
    exec_table::clear();
}

// Historical callers/tests keep these names; the CLI uses the backend-neutral
// entry. SDL native ABI calls live only in the adapter, outside this registry.
pub use crate::host_platform::{bring_up_host_session as bring_up_sdl_session, HostSessionReport as SdlSessionReport};
pub use crate::platform_sdl::api as sdl_api;

/// Registry lifecycle status of the dynamically registered audio policy
/// (ticket 45 model): the values come from whichever deployed library won
/// the first-write-wins registration, or the explicit no-policy note.
pub fn log_audio_policy_status() {
    match registered_audio_policy() {
        Some(policy) => {
            let id = policy.id.to_cstr().map(|id| id.to_string_lossy().into_owned()).unwrap_or_default();
            rudroid_common::diagnostic!(
                "[AUDIO] policy registered: id={id} default_open_attempts={} \
                 named_open_attempts={} retry_delay_ms={} enumerate_outputs={} \
                 prefer_non_hdmi={} reject_silent_drivers={}",
                policy.default_open_attempts,
                policy.named_open_attempts,
                policy.retry_delay_ms,
                policy.enumerate_outputs,
                policy.prefer_non_hdmi,
                policy.reject_silent_drivers
            );
        }
        None => rudroid_common::diagnostic!(
            "[AUDIO] no audio policy provider deployed; the selected output backend uses its default policy"
        ),
    }
}

// ---------------------------------------------------------------- unity profile runner (ticket 53)

#[path = "runtime_exec_table.rs"]
mod exec_table;
pub use exec_table::{guest_exec_bases, guest_exec_symbol, guest_exec_symbol_all, register_guest_exec_backend,
    register_guest_exec_module, register_guest_exec_symbol_all, unregister_guest_exec_module};
pub(crate) use exec_table::lookup_guest_symbol;

#[path = "engine_profile.rs"]
mod profile;
pub use profile::{
    bootstrap_engine_runtime, prepare_engine_runtime, run_engine_runtime, EngineRunCompletion,
};

// ---------------------------------------------------------------- C table bindings

/// The `extern "C"` implementations wired into the Host API table.
pub mod ffi {
    use super::*;

    pub extern "C" fn register_module_loaded_c(
        kind: u32,
        callback: Option<RudroidModuleLoadedCallback>,
        user: *mut c_void,
    ) -> c_int {
        match callback {
            Some(callback) => super::register_module_loaded(kind, callback, user),
            None => NULL_ARG,
        }
    }

    pub extern "C" fn register_after_main_jni_c(
        callback: Option<RudroidAfterMainJniCallback>,
        user: *mut c_void,
    ) -> c_int {
        match callback {
            Some(callback) => super::register_after_main_jni(callback, user),
            None => NULL_ARG,
        }
    }

    pub extern "C" fn fire_module_event_c(kind: u32) -> c_int {
        super::fire_runtime_event(kind)
    }

    pub extern "C" fn register_engine_profile_c(profile: Option<&RudroidEngineProfile>) -> c_int {
        super::register_engine_profile(profile)
    }

    pub extern "C" fn register_android_hooks_c(hooks: Option<&RudroidAndroidHooks>) -> c_int {
        super::register_android_hooks(hooks)
    }

    pub extern "C" fn register_audio_policy_c(policy: Option<&RudroidAudioPolicy>) -> c_int {
        super::register_audio_policy(policy)
    }

    pub extern "C" fn register_catalog_provider_c(provider: Option<&CatalogProviderV0>) -> c_int {
        super::register_catalog_provider(provider)
    }

    pub extern "C" fn catalog_query_int_c(
        class: HostStr<'_>,
        method: HostStr<'_>,
        sig: HostStr<'_>,
        out: Option<&mut i64>,
    ) -> c_int {
        super::catalog_query_int(class, method, sig, out)
    }

    pub extern "C" fn catalog_query_bool_c(
        class: HostStr<'_>,
        method: HostStr<'_>,
        sig: HostStr<'_>,
        out: Option<&mut c_int>,
    ) -> c_int {
        super::catalog_query_bool(class, method, sig, out)
    }

    /// Nullable typed VM, with the same pointer ABI as JavaVM*.
    pub extern "C" fn register_shared_vm_c(vm: Option<JavaVm>) -> c_int {
        match vm {
            Some(vm) => super::register_shared_vm(vm),
            None => NULL_ARG,
        }
    }

    pub extern "C" fn shared_vm_c() -> Option<JavaVm> {
        super::registry().lock().unwrap().shared_vm
    }
}

#[cfg(test)]
#[path = "../tests/unit/runtime_catalog_state_machine_tests.rs"]
mod catalog_state_machine_tests;

#[path = "runtime_catalog.rs"]
mod catalog;
pub use catalog::{catalog_query_bool, catalog_query_int, register_catalog_provider};
use catalog::{begin_shutdown, destroy_provider_and_reopen, CatalogSlot};
#[cfg(test)]
use catalog::release_lease;
