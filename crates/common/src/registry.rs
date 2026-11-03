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
//! - `RudroidUnityProfile`, `RudroidAndroidHooks` and `RudroidAudioPolicy` are copied
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

/// Unity profile phases. `session` is the profile's opaque value from
/// `prepare`; the host only passes it back.
pub type RudroidUnityPrepareCallback =
    extern "C" fn(api: Option<&'static HostApiV0>) -> *mut c_void;
pub type RudroidUnityBootstrapCallback =
    extern "C" fn(session: *mut c_void, native_library_dir: HostStr<'_>) -> c_int;
pub type RudroidUnityRunCallback = extern "C" fn(session: *mut c_void) -> c_int;

/// The engine requested stop and preferences were synchronously saved. The
/// caller must end this process immediately, without native/service teardown:
/// their state and code remain pinned until the kernel reclaims the process.
/// Zero still means full teardown completed; negative values are failures.
pub const RUDROID_UNITY_RUN_PROCESS_EXIT: c_int = 1;

/// Reference `RudroidUnityProfileFlags`.
pub const RUDROID_UNITY_PROFILE_NONE: u32 = 0;
pub const RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG: u32 = 1 << 0;

/// Unity engine profile (reference `RudroidUnityProfile`). The host copies the
/// struct; the profile lifecycle body itself is ticket 53's work.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RudroidUnityProfile {
    pub id: HostStr<'static>,
    pub player_class: HostStr<'static>,
    pub player_constructor_signature: HostStr<'static>,
    pub init_jni_signature: HostStr<'static>,
    pub flags: u32,
    pub init_jni_int_arg: i32,
    pub prepare: Option<RudroidUnityPrepareCallback>,
    pub bootstrap: Option<RudroidUnityBootstrapCallback>,
    pub run: Option<RudroidUnityRunCallback>,
}

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
