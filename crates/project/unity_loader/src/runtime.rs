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
    RudroidAndroidHooks, RudroidAudioPolicy, RudroidModuleEvent, RudroidModuleLoadedCallback, RudroidUnityProfile,
    CatalogProviderV0, HostStr,
};

const REGISTRY_REJECTED: c_int = 1;
const NULL_ARG: c_int = 2;

/// Lifecycle of the catalog provider slot (tickets 61/66). The states make
/// illegal interleavings unrepresentable: an in-flight counter only exists
/// while its generation can legally hold leases, and a destroy that has not
/// returned yet is visible as [`CatalogSlot::Destroying`], so no new
/// generation can slip in behind it.
#[derive(Default)]
enum CatalogSlot {
    /// Open for queries: `provider` answers, `in_flight` query callbacks are
    /// currently running outside the lock.
    Open {
        provider: CatalogProviderV0,
        in_flight: u32,
    },
    /// `shutdown` marked this generation: new queries miss and new providers
    /// are rejected; the last in-flight query's epilogue performs the destroy.
    Draining {
        provider: CatalogProviderV0,
        in_flight: u32,
    },
    /// The provider copy has been taken for destruction, which runs OUTSIDE
    /// the registry lock (the callback may re-enter the registry). This is
    /// the ticket-66 generation barrier: registrations are rejected until the
    /// destroy callback returns and the slot becomes [`CatalogSlot::Vacant`].
    Destroying,
    /// No provider; a fresh registration starts a new generation.
    #[default]
    Vacant,
}

#[derive(Default)]
struct RegistryState {
    module_subscriptions: Arc<Vec<ModuleSubscription>>,
    profile: Option<OwnedProfile>,
    hooks: Option<RudroidAndroidHooks>,
    audio_policy: Option<RudroidAudioPolicy>,
    catalog: CatalogSlot,
    shared_vm: Option<JavaVm>,
}

/// A profile copy; strings stay as pointers into the pinned registrant.
struct OwnedProfile {
    inner: RudroidUnityProfile,
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

pub fn register_unity_profile(profile: Option<&RudroidUnityProfile>) -> c_int {
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

pub fn register_catalog_provider(provider: Option<&CatalogProviderV0>) -> c_int {
    let Some(provider) = provider else {
        return NULL_ARG;
    };
    let mut state = registry().lock().unwrap();
    // Ticket 66 generation barrier: a new provider registers only into a
    // `Vacant` slot. While the old generation still drains (in-flight
    // queries) or its destroy callback has not returned yet, registration is
    // deterministically rejected and the caller keeps ownership.
    if !matches!(state.catalog, CatalogSlot::Vacant) {
        return REGISTRY_REJECTED;
    }
    state.catalog = CatalogSlot::Open {
        provider: *provider,
        in_flight: 0,
    };
    0
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

/// Release one catalog lease (the query-epilogue half of the slot state
/// machine, ticket 66). Returns the provider to destroy when this was the
/// last in-flight user of a draining slot. Illegal states panic: an
/// in-flight underflow is a state-machine bug and must never be smoothed
/// over with `saturating_sub`.
fn release_lease(slot: &mut CatalogSlot) -> Option<CatalogProviderV0> {
    match slot {
        CatalogSlot::Open { in_flight, .. } => {
            assert!(*in_flight > 0, "catalog lease underflow on an open slot");
            *in_flight -= 1;
            None
        }
        CatalogSlot::Draining { provider, in_flight } => {
            assert!(*in_flight > 0, "catalog lease underflow on a draining slot");
            *in_flight -= 1;
            if *in_flight == 0 {
                // Exclusive with `shutdown`: the first side to reach zero
                // in-flight moves the slot to `Destroying` and gets the
                // provider copy (exactly-once destroy).
                let provider = *provider;
                *slot = CatalogSlot::Destroying;
                Some(provider)
            } else {
                None
            }
        }
        CatalogSlot::Destroying | CatalogSlot::Vacant => {
            panic!("catalog query epilogue without an active lease");
        }
    }
}

fn release_catalog_lease() -> Option<CatalogProviderV0> {
    let mut state = registry().lock().unwrap();
    release_lease(&mut state.catalog)
}

/// Run the destroy callback OUTSIDE the registry lock (it may re-enter the
/// registry) and only reopen the slot for the next generation after the
/// callback returned — the ticket-66 barrier.
fn destroy_provider_and_reopen(provider: CatalogProviderV0) {
    if let Some(destroy) = provider.destroy {
        // This generation's provider copy was handed to exactly one drainer;
        // the registry lock is not held here and every registry entry point
        // the callback might call observes `Destroying`.
        destroy(provider.user);
    }
    let mut state = registry().lock().unwrap();
    assert!(
        matches!(state.catalog, CatalogSlot::Destroying),
        "destroy epilogue on a slot that is not being destroyed"
    );
    state.catalog = CatalogSlot::Vacant;
}

fn query_with<R>(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut R>,
    call: impl Fn(&CatalogProviderV0, *mut c_void, HostStr<'_>, HostStr<'_>, HostStr<'_>, Option<&mut R>) -> c_int,
) -> c_int {
    // Lease the provider for this query (ticket 61): the copy plus an
    // in-flight bump happen under the lock, so shutdown cannot destroy the
    // provider while the callback is running. Only an `Open` slot leases;
    // draining, destroying and vacant slots miss cleanly.
    let provider = {
        let mut state = registry().lock().unwrap();
        match &mut state.catalog {
            CatalogSlot::Open { provider, in_flight } => {
                // Exhaustion must miss rather than wrap and let shutdown
                // destroy a provider with outstanding callbacks.
                let Some(next) = in_flight.checked_add(1) else { return 0; };
                *in_flight = next;
                *provider
            }
            CatalogSlot::Draining { .. } | CatalogSlot::Destroying | CatalogSlot::Vacant => {
                return 0
            }
        }
    };
    // The callback runs outside the registry lock and may re-enter.
    let result = (call)(&provider, provider.user, class, method, sig, out);
    // Release the lease; the last in-flight user of a draining slot takes
    // the provider for destruction (exactly once — the transition to
    // `Destroying` is exclusive with shutdown) and the destroy stays outside
    // the lock, opening the generation barrier only when it returns.
    if let Some(provider) = release_catalog_lease() {
        destroy_provider_and_reopen(provider);
    }
    result
}

pub fn catalog_query_int(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int {
    query_with(class, method, sig, out, |provider, user, c, m, s, out| {
        provider.query_int.map_or(0, |query| query(user, c, m, s, out))
    })
}

pub fn catalog_query_bool(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut c_int>,
) -> c_int {
    query_with(class, method, sig, out, |provider, user, c, m, s, out| {
        provider.query_bool.map_or(0, |query| query(user, c, m, s, out))
    })
}

// ---------------------------------------------------------------- accessors + shutdown

/// Read access for the loader-side consumers (profile runner, audio backend).
pub fn registered_profile() -> Option<RudroidUnityProfile> {
    registry().lock().unwrap().profile.as_ref().map(|p| p.inner)
}

pub fn registered_audio_policy() -> Option<RudroidAudioPolicy> {
    registry().lock().unwrap().audio_policy
}

pub fn registered_android_hooks() -> Option<RudroidAndroidHooks> {
    registry().lock().unwrap().hooks
}

/// `shutdown` half of the slot state machine (tickets 61/66): stop the slot
/// from leasing and decide who performs the destroy. Returns the provider
/// when the caller must destroy it right away (no query in flight); `None`
/// when an in-flight query's epilogue will, when a destroy is already in
/// progress (never wait for it, never destroy twice), or when nothing is
/// registered.
fn begin_shutdown(slot: &mut CatalogSlot) -> Option<CatalogProviderV0> {
    match slot {
        CatalogSlot::Open { provider, in_flight: 0 } => {
            let provider = *provider;
            *slot = CatalogSlot::Destroying;
            Some(provider)
        }
        CatalogSlot::Open { provider, in_flight } => {
            *slot = CatalogSlot::Draining {
                provider: *provider,
                in_flight: *in_flight,
            };
            None
        }
        CatalogSlot::Draining { .. } | CatalogSlot::Destroying | CatalogSlot::Vacant => None,
    }
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
    let to_destroy = {
        let mut state = registry().lock().unwrap();
        state.module_subscriptions = Arc::new(Vec::new());
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
    *PROFILE_SESSION.lock().unwrap() = None;
    {
        let mut table = guest_exec().lock().unwrap();
        table.modules.clear();
        table.exec_symbol = None;
        table.exec_symbol_all = None;
    }
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
            eprintln!(
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
        None => eprintln!(
            "[AUDIO] no audio policy provider deployed; the selected output backend uses its default policy"
        ),
    }
}

// ---------------------------------------------------------------- unity profile runner (ticket 53)

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
    table.modules.retain(|(module_base, _)| *module_base != base);
}

/// Resolve a guest module export by load base and name (the `so_symbol`
/// contract: 0 on any miss). `None` when the base is unknown, the exec
/// backend is absent, or the symbol is not an export.
pub fn guest_exec_symbol(base: usize, name: &str) -> Option<usize> {
    let (exec_symbol, handle) = {
        let table = guest_exec().lock().unwrap();
        let exec_symbol = table.exec_symbol?;
        let handle = table
            .modules
            .iter()
            .find(|(module_base, _)| *module_base == base)
            .map(|(_, handle)| *handle)?;
        (exec_symbol, handle)
    };
    let c_name = CString::new(name.replace('\0', "\\0")).ok()?;
    // SAFETY: the backend and mapping stay process-pinned even if shutdown
    // or a reentrant callback withdraws this binding after the lock drops.
    // The entry follows the documented C contract.
    let address = unsafe { exec_symbol(handle, c_name.as_ptr()) };
    (address != 0).then_some(address)
}

/// [`guest_exec_symbol`] through the `.symtab`-capable entry (the
/// `so_symbol_all` contract: 0 on any miss). `None` additionally when the
/// mapping capability does not export the entry.
pub fn guest_exec_symbol_all(base: usize, name: &str) -> Option<usize> {
    let (exec_symbol_all, handle) = {
        let table = guest_exec().lock().unwrap();
        let exec_symbol_all = table.exec_symbol_all?;
        let handle = table
            .modules
            .iter()
            .find(|(module_base, _)| *module_base == base)
            .map(|(_, handle)| *handle)?;
        (exec_symbol_all, handle)
    };
    let c_name = CString::new(name.replace('\0', "\\0")).ok()?;
    // SAFETY: as guest_exec_symbol — live pinned mapping, documented C API.
    let address = unsafe { exec_symbol_all(handle, c_name.as_ptr()) };
    (address != 0).then_some(address)
}

/// The prepared profile session (opaque pointer owned by the profile
/// capability): `Some` between a successful prepare and the run hand-off.
/// Cleared by [`shutdown`] so a failed init never leaves a live session
/// behind.
static PROFILE_SESSION: Mutex<Option<usize>> = Mutex::new(None);

/// Cross-check one registered profile against the deployed capability and
/// the reference registration rules (`api_register_unity_profile` parity:
/// identity strings plus all three lifecycle callbacks are required).
fn validate_registered_profile(
    profile: &RudroidUnityProfile,
    capability_id: &str,
) -> Result<(), super::LoaderError> {
    let read = |text: HostStr<'static>| -> Option<String> {
        let text = text.to_cstr()?.to_string_lossy().into_owned();
        (!text.is_empty()).then_some(text)
    };
    let id = read(profile.id);
    if id.as_deref() != Some(capability_id) {
        return Err(super::LoaderError::ProfileLifecycle {
            id: capability_id.to_owned(),
            stage: "register",
            reason: format!(
                "the registered profile id {id:?} does not match the deployed \
                 capability id {capability_id:?}; the kit is inconsistent"
            ),
        });
    }
    for (field, value) in [
        ("player_class", read(profile.player_class)),
        (
            "player_constructor_signature",
            read(profile.player_constructor_signature),
        ),
        ("init_jni_signature", read(profile.init_jni_signature)),
    ] {
        if value.is_none() {
            return Err(super::LoaderError::ProfileLifecycle {
                id: capability_id.to_owned(),
                stage: "register",
                reason: format!("the registered profile misses `{field}`"),
            });
        }
    }
    // The three callbacks have distinct signatures; presence checks stay
    // per field (reference `api_register_unity_profile` rejects null ones).
    if profile.prepare.is_none() || profile.bootstrap.is_none() || profile.run.is_none() {
        let missing = [
            ("prepare", profile.prepare.is_none()),
            ("bootstrap", profile.bootstrap.is_none()),
            ("run", profile.run.is_none()),
        ]
        .into_iter()
        .filter(|(_, absent)| *absent)
        .map(|(field, _)| field)
        .collect::<Vec<_>>()
        .join(", ");
        return Err(super::LoaderError::ProfileLifecycle {
            id: capability_id.to_owned(),
            stage: "register",
            reason: format!("the registered profile misses its `{missing}` callback"),
        });
    }
    Ok(())
}

/// The deployed Unity profile capability plus its registered callbacks and
/// the prepared session (bootstrap/run precondition).
fn prepared_profile(
    capabilities: &[super::LoadedCapability],
    stage: &'static str,
) -> Result<(String, RudroidUnityProfile, *mut c_void), super::LoaderError> {
    let capability = capabilities
        .iter()
        .find(|c| c.descriptor.kind == rudroid_common::Kind::UnityProfile)
        .ok_or(super::LoaderError::NoUnityProfile)?;
    let id = capability.descriptor.id.clone();
    let profile = registered_profile().ok_or_else(|| super::LoaderError::ProfileLifecycle {
        id: id.clone(),
        stage: "register",
        reason: "the profile capability did not register its RudroidUnityProfile \
                 during the register-JNI phase; redeploy a consistent kit"
            .to_owned(),
    })?;
    validate_registered_profile(&profile, &id)?;
    let session = PROFILE_SESSION
        .lock()
        .unwrap()
        .ok_or_else(|| super::LoaderError::ProfileLifecycle {
            id: id.clone(),
            stage,
            reason: "called before a successful prepare; check bring-up order"
                .to_owned(),
        })?;
    Ok((id, profile, session as *mut c_void))
}

/// Host-side half of the reference `prepare_unity_runtime` (ticket 53): the
/// deployed profile must have registered exactly one engine profile during
/// the register-JNI phase, the Android runtime hooks must already be
/// registered (reference: "android_base plugin is missing or failed"), and
/// the profile's own `prepare` must return a session. Runs BEFORE the guest
/// modules are mapped (reference order).
pub fn prepare_unity_runtime(
    capabilities: &[super::LoadedCapability],
) -> Result<(), super::LoaderError> {
    {
        let session = PROFILE_SESSION.lock().unwrap();
        if session.is_some() {
            let id = capabilities
                .iter()
                .find(|c| c.descriptor.kind == rudroid_common::Kind::UnityProfile)
                .map(|c| c.descriptor.id.clone())
                .unwrap_or_default();
            return Err(super::LoaderError::ProfileLifecycle {
                id,
                stage: "prepare",
                reason: "the Unity engine session is already prepared".to_owned(),
            });
        }
    }
    let capability = capabilities
        .iter()
        .find(|c| c.descriptor.kind == rudroid_common::Kind::UnityProfile)
        .ok_or(super::LoaderError::NoUnityProfile)?;
    let id = capability.descriptor.id.clone();
    let profile = registered_profile().ok_or_else(|| super::LoaderError::ProfileLifecycle {
        id: id.clone(),
        stage: "register",
        reason: "the profile capability did not register its RudroidUnityProfile \
                 during the register-JNI phase; redeploy a consistent kit"
            .to_owned(),
    })?;
    validate_registered_profile(&profile, &id)?;
    // Reference gate: prepare proceeds only with both runtime hooks
    // registered (the android_base role).
    let hooks = registered_android_hooks();
    let hooks_ok = matches!(&hooks, Some(hooks) if hooks.flush_preferences.is_some() && hooks.signal_vsync.is_some());
    if !hooks_ok {
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "prepare",
            reason: "the android_base plugin is missing or failed: flush_preferences \
                     and signal_vsync must be registered before the engine prepares"
                .to_owned(),
        });
    }
    let Some(prepare) = profile.prepare else {
        // Validated above; the explicit arm keeps the call total.
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "register",
            reason: "the registered profile misses its `prepare` callback".to_owned(),
        });
    };
    // The loader's table is a process static.
    let session = prepare(Some(crate::host_api::table()));
    if session.is_null() {
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "prepare",
            reason: "the profile's prepare returned no session (see its UNITY \
                     diagnostics above)"
                .to_owned(),
        });
    }
    *PROFILE_SESSION.lock().unwrap() = Some(session as usize);
    crate::host_api::loader_log("PROFILE", &format!("unity profile prepared: {id}"));
    Ok(())
}

/// Host-side half of the reference `bootstrap_unity_runtime` (ticket 53):
/// hand the prepared session and the native library dir to the profile's
/// registered bootstrap (the `NativeLoader.load` analogue). Non-zero means
/// the guest refused — the run fails with the reason, never a fake success.
pub fn bootstrap_unity_runtime(
    capabilities: &[super::LoadedCapability],
    native_library_dir: &str,
) -> Result<(), super::LoaderError> {
    let (id, profile, session) = prepared_profile(capabilities, "bootstrap")?;
    if native_library_dir.is_empty() {
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "bootstrap",
            reason: "the native library dir is empty".to_owned(),
        });
    }
    let c_dir = CString::new(native_library_dir.replace('\0', "\\0"))
        .expect("NUL bytes are replaced above");
    let Some(bootstrap) = profile.bootstrap else {
        // Validated in prepared_profile; the explicit arm keeps the call total.
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "register",
            reason: "the registered profile misses its `bootstrap` callback".to_owned(),
        });
    };
    // The session is prepare's opaque value; the dir outlives the call.
    let rc = bootstrap(session, rudroid_common::registry::HostStr::new(&c_dir));
    if rc != 0 {
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "bootstrap",
            reason: format!(
                "the profile bootstrap returned {rc} (NativeLoader.load failed — \
                 see its UNITY diagnostics above)"
            ),
        });
    }
    crate::host_api::loader_log(
        "PROFILE",
        &format!("unity profile bootstrapped: {id} native_library_dir={native_library_dir}"),
    );
    Ok(())
}

/// Full teardown, or an explicit handoff to immediate process termination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "ProcessExit requires immediate process termination with the kit still pinned"]
pub enum UnityRunCompletion {
    Stopped,
    ProcessExit,
}

/// Run the engine profile, then flush applied Android preferences. Consume
/// ProcessExit immediately through the CLI terminal boundary with every
/// library/service owner still pinned; Stopped completed full teardown.
pub fn run_unity_runtime(
    capabilities: &[super::LoadedCapability],
) -> Result<UnityRunCompletion, super::LoaderError> {
    let (id, profile, session) = prepared_profile(capabilities, "run")?;
    let Some(run) = profile.run else {
        // Validated in prepared_profile; the explicit arm keeps the call total.
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "register",
            reason: "the registered profile misses its `run` callback".to_owned(),
        });
    };
    let rc = run(session);
    if let Some(flush) = registered_android_hooks().and_then(|hooks| hooks.flush_preferences) {
        // Persist already-applied changes even when the profile reports an
        // ordinary failure. The error below keeps its original outcome.
        flush();
    }
    if rc == rudroid_common::registry::RUDROID_UNITY_RUN_PROCESS_EXIT {
        crate::host_api::loader_log("PROFILE", "engine stop ready for immediate process exit (saved preferences)");
        return Ok(UnityRunCompletion::ProcessExit);
    }
    if rc != 0 {
        return Err(super::LoaderError::ProfileLifecycle {
            id,
            stage: "run",
            reason: format!(
                "the profile run returned {rc} (the engine did not exit cleanly — \
                 see its UNITY diagnostics above)"
            ),
        });
    }
    crate::host_api::loader_log(
        "PROFILE",
        &format!("unity profile run complete: {id} (player stopped)"),
    );
    Ok(UnityRunCompletion::Stopped)
}

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

    pub extern "C" fn fire_module_event_c(kind: u32) -> c_int {
        super::fire_runtime_event(kind)
    }

    pub extern "C" fn register_unity_profile_c(profile: Option<&RudroidUnityProfile>) -> c_int {
        super::register_unity_profile(profile)
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
