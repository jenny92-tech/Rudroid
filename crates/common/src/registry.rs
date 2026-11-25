//! INPUT: cross-capability callbacks, handles and service descriptions
//! OUTPUT: shared registration and query ABI types
//! POS: runtime service ABI used by the loader and its members
//! Registration PODs crossing the kit C ABI (ticket 45).
//!
//! Mirrors the read-only reference (`plugin_api.h`, `audio_policy.h`): the
//! host owns ONE process-wide registry; capabilities register providers,
//! profiles, policies and module subscriptions through the Host API table,
//! and the host fires events back across the same boundary. Rust kits are
//! same-build, so struct layout is guaranteed kit-wide; the plain-data rule
//! (POD in/out-params only) keeps every field C-compatible.
//!
//! Lifetime rules (documented contract):
//! - Strings inside registered PODs are `HostStr<'static>` (literals or
//!   `rudroid_common::descriptor::leak_cstr`), and the registering library
//!   stays pinned by the loader, so the host may keep the copied struct
//!   forever. Every type here is ABI-identical to the C reference: `HostStr`
//!   and `Option<&T>` are nullable pointers, callbacks are plain fn pointers.
//! - `RudroidEngineProfile`, `RudroidAndroidHooks` and `RudroidAudioPolicy` are copied
//!   by the host during registration (reference parity: "copied by the host
//!   during init"). The first registration wins; duplicates are rejected
//!   with a non-zero return.
//! - `CatalogProviderV0` is copied during registration: its function pointers
//!   and `user` refer to the pinned provider library. Its threading contract
//!   (ticket 70) is documented on the struct: the ABI allows CONCURRENT
//!   queries, and the host destroys a provider only after every in-flight
//!   query callback returned.
//! - Module callbacks are invoked outside all registry locks, so a callback
//!   may query the registry or subscribe again; different fires may also run
//!   concurrently on different threads (same rule as catalog providers).
//!   Subscriptions added during a fire take effect from the next fire on.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::marker::PhantomData;

use crate::host_api::HostApiV0;

/// A NUL-terminated string the host lends for `'a` (`#[repr(transparent)]`
/// over `const char*`). Module names live for the process; a callback
/// argument only for the call, which the higher-ranked `'a` enforces. Built
/// from a `&'a CStr`, or from a raw pointer only through `unsafe`, so reading
/// it back is safe.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostStr<'a>(*const c_char, PhantomData<&'a CStr>);

impl<'a> HostStr<'a> {
    pub const fn new(text: &'a CStr) -> Self {
        Self(text.as_ptr(), PhantomData)
    }

    pub const fn null() -> Self {
        Self(std::ptr::null(), PhantomData)
    }

    /// # Safety
    /// `text` is null or NUL-terminated, valid and unchanged for `'a`.
    pub const unsafe fn from_ptr(text: *const c_char) -> Self {
        Self(text, PhantomData)
    }

    pub fn as_ptr(self) -> *const c_char {
        self.0
    }

    /// The string, `None` for null.
    pub fn to_cstr(self) -> Option<&'a CStr> {
        // SAFETY: built from a `&'a CStr` or under the `from_ptr` contract.
        (!self.0.is_null()).then(|| unsafe { CStr::from_ptr(self.0) })
    }
}

/// Module-loaded event (reference `RudroidModuleLoadedCallback` payload).
/// `name` is the planned module path relative to `game_files` — the
/// verifiable identity alongside kind and base.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RudroidModuleEvent {
    pub kind: u32,
    /// Opaque base as reported by the mapping capability (ticket 48 corrects
    /// the base semantics; identity here is "what the mapper reported").
    pub base: usize,
    /// Process-lifetime module identity (e.g. `lib/arm64-v8a/libmain.so`).
    pub name: HostStr<'static>,
}

/// Module-loaded callback. Callbacks are plain safe `extern "C" fn`s: the
/// event arrives as `Option<&RudroidModuleEvent>` (ABI-identical to the nullable
/// `const RudroidModuleEvent*` of the C reference) and `user` is the registrant's
/// opaque value, which the host never dereferences.
pub type RudroidModuleLoadedCallback =
    extern "C" fn(event: Option<&RudroidModuleEvent>, user: *mut c_void);

/// Loads one relative guest library below the active `game_files` root and
/// calls that image's `JNI_OnLoad` with the shared VM. The loader owns the
/// mapping/handle for process life. Zero reports success and writes the image
/// base when `out_base` is non-null; any non-zero value is a hard failure.
///
/// # Safety
/// `user` is the loader's opaque bring-up value from
/// [`RudroidAfterMainJniContext`]. `relative` is a valid borrowed string for
/// this call only. `out_base`, when non-null, is writable for one `usize`.
pub type RudroidGuestLoadAndJniOnLoadFn = unsafe extern "C" fn(
    user: *mut c_void,
    relative: HostStr<'_>,
    out_base: *mut usize,
) -> c_int;

/// The Main image has completed `JNI_OnLoad`; the next il2cpp load has not
/// started. Capabilities may use the loader-owned operation to reproduce an
/// explicit Java `System.loadLibrary` sequence. Return zero to continue and
/// non-zero to stop bring-up before il2cpp is mapped.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RudroidAfterMainJniContext {
    pub main: RudroidModuleEvent,
    pub loader_user: *mut c_void,
    pub load_and_jni_on_load: Option<RudroidGuestLoadAndJniOnLoadFn>,
}

/// Optional post-Main-JNI hook. The host never dereferences `user`; callbacks
/// run in registration order and a non-zero result aborts bring-up.
pub type RudroidAfterMainJniCallback =
    extern "C" fn(context: Option<&RudroidAfterMainJniContext>, user: *mut c_void) -> c_int;

/// Engine profile phases. `session` is the profile's opaque value from
/// `prepare`; the host only passes it back.
pub type RudroidEnginePrepareCallback =
    extern "C" fn(api: Option<&'static HostApiV0>) -> *mut c_void;
pub type RudroidEngineValidateConfigCallback =
    extern "C" fn(api: Option<&'static HostApiV0>) -> c_int;
pub type RudroidEngineBootstrapCallback =
    extern "C" fn(session: *mut c_void, native_library_dir: HostStr<'_>) -> c_int;
pub type RudroidEngineRunCallback = extern "C" fn(session: *mut c_void) -> c_int;

/// Builder callbacks append guest-plan entries owned by the loader. `role_id`
/// is [`crate::ModuleKind`] as `u32` (`0` = no event). `after_role_id` is a
/// KindLoaded gate (`0` = none). `flags`:
/// [`RUDROID_GUEST_ENTRY_FMOD_JNI_OK`] and [`RUDROID_GUEST_ENTRY_AFTER_FMOD`].
#[repr(C)]
pub struct RudroidGuestPlanBuilder {
    pub ctx: *mut c_void,
    pub add_entry: Option<
        extern "C" fn(
            ctx: *mut c_void,
            relative_path: HostStr<'_>,
            role_id: u32,
            required: c_int,
            after_role_id: u32,
            flags: u32,
        ) -> c_int,
    >,
    pub add_reserved_path:
        Option<extern "C" fn(ctx: *mut c_void, canonical_path: HostStr<'_>) -> c_int>,
    pub set_defer_jni_on_load: Option<extern "C" fn(ctx: *mut c_void, role_id: u32) -> c_int>,
}

/// Fill the loader-owned guest plan. Prefer `api` for config/service queries;
/// session is not required (may be called before prepare).
pub type RudroidEngineBuildGuestPlanCallback = extern "C" fn(
    api: Option<&'static HostApiV0>,
    builder: Option<&RudroidGuestPlanBuilder>,
) -> c_int;

/// The engine requested stop and preferences were synchronously saved. The
/// caller must end this process immediately, without native/service teardown:
/// their state and code remain pinned until the kernel reclaims the process.
/// Zero still means full teardown completed; negative values are failures.
pub const RUDROID_ENGINE_RUN_PROCESS_EXIT: c_int = 1;

/// Engine profile registered through the Host API. The host copies the struct.
/// Player class / initJni shape live on the engine Spec (player_lifecycle).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RudroidEngineProfile {
    pub id: HostStr<'static>,
    pub validate_config: Option<RudroidEngineValidateConfigCallback>,
    pub build_guest_plan: Option<RudroidEngineBuildGuestPlanCallback>,
    pub prepare: Option<RudroidEnginePrepareCallback>,
    pub bootstrap: Option<RudroidEngineBootstrapCallback>,
    pub run: Option<RudroidEngineRunCallback>,
}

/// Guest-plan entry flags for [`RudroidGuestPlanBuilder::add_entry`].
pub const RUDROID_GUEST_ENTRY_FMOD_JNI_OK: u32 = 1 << 0;
/// Also require `KindLoaded(Fmod)` (FmodStudio pattern with `after_role_id = Unity`).
pub const RUDROID_GUEST_ENTRY_AFTER_FMOD: u32 = 1 << 1;

/// Android runtime hooks (reference `RudroidAndroidHooks`).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RudroidAndroidHooks {
    pub flush_preferences: Option<extern "C" fn()>,
    pub signal_vsync: Option<extern "C" fn()>,
}

/// Platform audio policy (reference `RudroidAudioPolicy`, `audio_policy.h`).
/// Registration only in this ticket; the audio backend consumes it (55).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RudroidAudioPolicy {
    pub id: HostStr<'static>,
    pub default_open_attempts: u32,
    pub named_open_attempts: u32,
    pub retry_delay_ms: u32,
    pub enumerate_outputs: i32,
    pub prefer_non_hdmi: i32,
    pub reject_silent_drivers: i32,
}

/// Reply Catalog provider registered by the catalog capability (ticket 45).
/// Query functions return `1` on hit with the out-param written, `0` on
/// miss — the same contract as the catalog cdylib C API.
///
/// # Provider threading contract (ticket 70, decision A)
///
/// The host invokes `query_int`/`query_bool` with `user` from MULTIPLE
/// THREADS CONCURRENTLY: a query leases the provider under the registry lock
/// (in-flight count) but the callback itself runs outside every registry
/// lock, so two queries can be inside the provider at the same time. This is
/// part of the ABI: a provider whose `user` state is not safe for concurrent
/// calls (Rust: shared state must be `Sync` or synchronized; C: immutable or
/// mutex-protected) violates the contract. Pinning only proves the function
/// addresses stay valid; it says nothing about calling them concurrently.
/// The built-in reply_catalog satisfies the contract by state: `user` is a
/// handle to an immutable blob-lookup `Catalog` whose `query` takes `&self`.
///
/// - Memory visibility: the `class`/`method`/`sig`/`out` arguments are valid
///   only for the duration of one call (their types enforce it). The
///   out-param write a provider performs is visible to its caller when the
///   call returns (same thread). Queries that do not overlap are ordered
///   through the registry lock every query takes, so a later query observes
///   an earlier query's provider-side writes; overlapping queries have no
///   ordering between them.
/// - Reentrancy: callbacks run outside all registry locks and may re-enter
///   the registry. From inside a query callback, another `catalog_query_*`
///   answers from the still-open slot and `shutdown` marks the slot draining
///   without waiting. From inside `destroy`, queries miss and new
///   registrations are rejected until the destroy returned (ticket 66).
/// - `user` ownership: `user` stays owned by the registering library (the
///   loader pins it for process life in normal operation). The host never
///   dereferences or frees `user`; it only passes it back opaquely to the
///   callbacks registered alongside it.
/// - `destroy` happens-before: the host calls `destroy` at most once,
///   strictly AFTER every in-flight query callback of the same provider has
///   RETURNED (tickets 61/66: the last lease release and the destroy are
///   ordered through the registry lock, so query-side writes are visible to
///   `destroy`), and never overlaps a query on `user`.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CatalogProviderV0 {
    pub user: *mut c_void,
    pub query_int: Option<
        extern "C" fn(
            user: *mut c_void,
            class: HostStr<'_>,
            method: HostStr<'_>,
            sig: HostStr<'_>,
            out: Option<&mut i64>,
        ) -> c_int,
    >,
    pub query_bool: Option<
        extern "C" fn(
            user: *mut c_void,
            class: HostStr<'_>,
            method: HostStr<'_>,
            sig: HostStr<'_>,
            out: Option<&mut c_int>,
        ) -> c_int,
    >,
    /// Called at most once per registered provider, under the struct-level
    /// threading contract above: never overlapping a query, happening after
    /// every in-flight query callback returned (tickets 61/66/70). It runs
    /// outside the registry lock; re-entrant registry calls are allowed but
    /// the destroyed provider answers nothing from that point on. Providers
    /// are process-pinned in normal operation — with the loader driving
    /// bring-up, teardown happens only on the failed-init path.
    pub destroy: Option<extern "C" fn(user: *mut c_void)>,
}

#[cfg(test)]
#[path = "../tests/unit/registry.rs"]
mod tests;
