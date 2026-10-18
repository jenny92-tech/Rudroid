//! INPUT:  shared registry ABI types and native C scalar/string conventions
//! OUTPUT:  HostApiV0, typed callbacks, API version and log_message
//! POS:    defines the same-build dynamic boundary for shared runtime services
//! Host API table: the C-shaped kit boundary handed to capabilities.
//!
//! Rust analogue of the reference `RudroidPluginApi`: config getters, log, and
//! — since ticket 45 — the shared-runtime registration surface (module
//! subscriptions, Unity profile, Android hooks, audio policy, catalog
//! provider and the shared VM/session slot). Everything not listed here
//! (real JNI VM, guest symbol tables, EGL/texture policies, ...) arrives in
//! later tickets as the corresponding capabilities land.
//!
//! Differences vs the C++ reference, kept on purpose:
//! - `log(tag, message)` takes a preformatted message, not a printf format
//!   string + varargs. Rust capabilities format before calling; this avoids
//!   varargs across the FFI boundary.
//! - `so_symbol(module, name)` resolves guest module exports since ticket
//!   53 (the loader keeps an exec-mapping table keyed by the load base a
//!   module-loaded event reported; before tickets 49/53 it was a v0
//!   placeholder that always returned 0). The loader still refuses a null
//!   module instead of crashing; 0 means "not an export" and callers fail
//!   explicitly on missing required functions.
//! - The shared VM/session slot is accessed through `register_shared_vm` /
//!   `shared_vm` function pointers instead of a plain field: the table is
//!   built once, and the VM arrives later in the bring-up sequence (real VM
//!   in ticket 52).
//! - `so_symbol_all(module, name)` is the reference `so_symbol_all`: the
//!   dynamic symbols first, then the module file's `.symtab` (hidden
//!   functions of an unstripped library, e.g. Swappy). It sits at the tail
//!   of the table, not next to `so_symbol` as in `plugin_api.h`, because the
//!   tail is the only growth direction.
//! - `HOST_API_VERSION` is 4: ticket 78 appended the capability-sourced
//!   lifecycle broadcast (`fire_module_event`, version 2), then
//!   `so_symbol_all` was appended (version 3); same-build kits guarantee the
//!   resource queries/pack enablement append the version-4 tail. Every member has the
//!   exact same struct layout, and all kit members rebuild together on any
//!   ABI change.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

use crate::registry::{
    RudroidAndroidHooks, RudroidAudioPolicy, RudroidModuleLoadedCallback, RudroidUnityProfile,
    CatalogProviderV0, HostStr,
};

pub const HOST_API_VERSION: u32 = 4;

/// Process-pinned bounded asset service. Positive replies report the complete byte length;
/// zero means unhandled, -1 missing, -2 invalid and -3 too large. No request/output is retained.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct AssetQueryFn(unsafe extern "C" fn(Option<&crate::android_assets::AssetRequest<'_>>, *mut u8, u32) -> i32);
impl AssetQueryFn {
    /// # Safety
    /// The callback stays mapped, accepts every valid request and writes only within
    /// the supplied output capacity. It returns the complete response length even
    /// for short buffers, initializing exactly that many bytes when they fit.
    pub const unsafe fn new(callback: unsafe extern "C" fn(Option<&crate::android_assets::AssetRequest<'_>>, *mut u8, u32) -> i32) -> Self { Self(callback) }
}
pub type EnableAssetPacksFn = extern "C" fn() -> i32;

// Entry types are ABI-identical to the C reference (`HostStr`, `Option<&T>`
// and `Option<&mut T>` are nullable pointers). VM entries carry the sealed
// process-pinned `JavaVm`; the raw string getter is held in a private wrapper
// whose unsafe constructor establishes its output-buffer contract once.

pub type LogFn = extern "C" fn(tag: HostStr<'_>, message: HostStr<'_>);

/// Bridge Rust text to the host logger, escaping embedded NULs as `\\0`.
pub fn log_message(log: LogFn, tag: &CStr, message: &str) {
    let message = if message.contains('\0') {
        CString::new(message.replace('\0', "\\0"))
    } else {
        CString::new(message)
    }.expect("message is NUL-free after escaping");
    log(HostStr::new(tag), HostStr::new(&message));
}
pub type GetenvFn = extern "C" fn(name: HostStr<'_>) -> *const c_char;
pub type ConfigGetBoolFn = extern "C" fn(key: HostStr<'_>, fallback: c_int) -> c_int;
pub type ConfigGetI64Fn = extern "C" fn(key: HostStr<'_>, fallback: i64) -> i64;
pub type ConfigGetF64Fn = extern "C" fn(key: HostStr<'_>, fallback: f64) -> f64;
/// A process-pinned getter that writes at most `out_size` bytes, NUL included.
/// The private pointer prevents safe table construction from assigning an
/// arbitrary unsafe callback with a different buffer contract.
///
/// ```compile_fail
/// use rudroid_common::{host_api::HostApiV0, registry::HostStr};
/// use std::ffi::{c_char, c_int};
/// unsafe extern "C" fn arbitrary(_: HostStr<'_>, _: HostStr<'_>, _: *mut c_char, _: u32) -> c_int { 0 }
/// let _api = HostApiV0 { config_get_string: arbitrary, ..HostApiV0::stubbed() };
/// ```
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct ConfigGetStringFn(unsafe extern "C" fn(HostStr<'_>, HostStr<'_>, *mut c_char, u32) -> c_int);
impl ConfigGetStringFn {
    /// # Safety
    /// The callback remains mapped for the process. For any valid key/fallback
    /// and a writable, nonempty `out_size`-byte output, it writes only within that range,
    /// initializes a NUL terminator on success and retains no borrowed pointer.
    pub const unsafe fn new(callback: unsafe extern "C" fn(HostStr<'_>, HostStr<'_>, *mut c_char, u32) -> c_int) -> Self {
        Self(callback)
    }
}
pub type ConfigArraySizeFn = extern "C" fn(key: HostStr<'_>) -> u32;
pub type ConfigSetStringFn = extern "C" fn(key: HostStr<'_>, value: HostStr<'_>) -> c_int;
/// Guest symbol lookup on a mapped module (`module` = the load base a
/// module-loaded event reported, ticket 48 semantics). The loader resolves
/// through its exec-mapping table (ticket 53); 0 on any miss, including an
/// unknown base.
pub type SoSymbolFn = extern "C" fn(module: *mut c_void, name: HostStr<'_>) -> usize;

pub type RegisterModuleLoadedFn = extern "C" fn(
    kind: u32,
    callback: Option<RudroidModuleLoadedCallback>,
    user: *mut c_void,
) -> c_int;
pub type RegisterUnityProfileFn = extern "C" fn(profile: Option<&RudroidUnityProfile>) -> c_int;
pub type RegisterAndroidHooksFn = extern "C" fn(hooks: Option<&RudroidAndroidHooks>) -> c_int;
pub type RegisterAudioPolicyFn = extern "C" fn(policy: Option<&RudroidAudioPolicy>) -> c_int;
/// Registers the process-wide catalog provider. The threading, reentrancy,
/// memory-visibility and `user`-ownership contract of the registered
/// provider is documented on [`CatalogProviderV0`] (ticket 70: concurrent
/// queries are part of the ABI; a duplicate registration is rejected and the
/// caller keeps ownership).
pub type RegisterCatalogProviderFn = extern "C" fn(provider: Option<&CatalogProviderV0>) -> c_int;
/// Callable concurrently from any thread: runs the registered provider's
/// query callback per the [`CatalogProviderV0`] threading contract (leased
/// under the registry lock, invoked outside it). Misses cleanly while the
/// slot is draining, destroying or vacant.
pub type CatalogQueryIntFn = extern "C" fn(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int;
/// Same contract as [`CatalogQueryIntFn`].
pub type CatalogQueryBoolFn = extern "C" fn(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut c_int>,
) -> c_int;
/// First write wins; a second registration is rejected (the session is
/// process-wide, created once by the host).
pub type RegisterSharedVmFn = extern "C" fn(vm: Option<crate::jni_env::JavaVm>) -> c_int;
/// Nullable, process-pinned VM, ABI-identical to `JavaVM*`. A safe table
/// implementation cannot publish an arbitrary integer as a valid session.
pub type SharedVmFn = extern "C" fn() -> Option<crate::jni_env::JavaVm>;
/// Fire a capability-sourced lifecycle event into the shared module-event
/// subscription table (ticket 78). The host allowlists the kinds a
/// capability may fire (Il2cppReady and player-thread UnityMaintenance/
/// UnityStopping/UnityFrame) — guest module-load events stay loader-only, so a
/// capability cannot spoof a mapping. Returns 0 on delivery, non-zero when
/// the kind is not fireable from capabilities.
pub type FireModuleEventFn = extern "C" fn(kind: u32) -> c_int;

/// `Copy` is a plain value copy of the table: function pointers and the
/// version word. It does NOT extend any callback or library lifetime —
/// copies remain bound to the same process-pinned registrant contract.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct HostApiV0 {
    pub abi_version: u32,
    pub getenv: GetenvFn,
    pub config_get_bool: ConfigGetBoolFn,
    pub config_get_i64: ConfigGetI64Fn,
    pub config_get_f64: ConfigGetF64Fn,
    pub config_get_string: ConfigGetStringFn,
    pub config_array_size: ConfigArraySizeFn,
    pub config_set_string: ConfigSetStringFn,
    pub so_symbol: SoSymbolFn,
    pub log: LogFn,
    // Ticket 45: shared-runtime registration surface (reference order where
    // applicable).
    pub register_module_loaded: RegisterModuleLoadedFn,
    pub register_unity_profile: RegisterUnityProfileFn,
    pub register_android_hooks: RegisterAndroidHooksFn,
    pub register_audio_policy: RegisterAudioPolicyFn,
    pub register_catalog_provider: RegisterCatalogProviderFn,
    pub catalog_query_int: CatalogQueryIntFn,
    pub catalog_query_bool: CatalogQueryBoolFn,
    pub register_shared_vm: RegisterSharedVmFn,
    pub shared_vm: SharedVmFn,
    // Ticket 78: capability-sourced lifecycle broadcast (appended last —
    // the tail is the only ABI-stable growth direction).
    pub fire_module_event: FireModuleEventFn,
    /// Reference `so_symbol_all` (appended in version 3): like
    /// `so_symbol`, falling back to the module file's `.symtab`; 0 on miss.
    pub so_symbol_all: SoSymbolFn,
    /// Version 4: one host-owned resource model for Java, NDK and Bionic paths.
    pub asset_query: AssetQueryFn,
    pub enable_asset_packs: EnableAssetPacksFn,
}

impl HostApiV0 {
    pub fn asset_query(&self, request: &crate::android_assets::AssetRequest<'_>) -> Result<Option<Vec<u8>>, crate::android_assets::AssetError> {
        use crate::android_assets::{AssetError, MAX_REPLY};
        let mut bytes = vec![0; 4096];
        for _ in 0..3 {
            // SAFETY: the sealed process-pinned service bounds all writes to this initialized buffer.
            let size = unsafe { (self.asset_query.0)(Some(request), bytes.as_mut_ptr(), bytes.len() as u32) };
            match size {
                0 => return Ok(None),
                -1 => return Err(AssetError::Missing),
                -2 => return Err(AssetError::Invalid),
                -3 => return Err(AssetError::TooLarge),
                n if n > 0 && n as usize <= bytes.len() => { bytes.truncate(n as usize); return Ok(Some(bytes)); },
                n if n > 0 && n as usize <= MAX_REPLY => bytes.resize(n as usize, 0),
                _ => return Err(AssetError::TooLarge),
            }
        }
        Err(AssetError::Changed)
    }
    pub fn asset_path(&self, kind: u32, path: &CStr, pack: Option<&CStr>, index: u32) -> Result<Option<CString>, crate::android_assets::AssetError> {
        self.asset_query(&crate::android_assets::AssetRequest::new(kind,path,pack,index))?
            .map(|bytes| CString::from_vec_with_nul(bytes).map_err(|_|crate::android_assets::AssetError::Invalid)).transpose()
    }
    pub fn enable_asset_packs(&self) -> bool { (self.enable_asset_packs)() == 0 }

    /// A table whose every entry panics when called (aborting at the C
    /// boundary). Tests override what they exercise:
    /// `HostApiV0 { log: capture, ..HostApiV0::stubbed() }`.
    pub fn stubbed() -> Self {
        fn called(entry: &str) -> ! {
            panic!("Host API entry `{entry}` called on a stub table")
        }
        extern "C" fn getenv(_: HostStr<'_>) -> *const c_char { called("getenv") }
        extern "C" fn config_get_bool(_: HostStr<'_>, _: c_int) -> c_int { called("config_get_bool") }
        extern "C" fn config_get_i64(_: HostStr<'_>, _: i64) -> i64 { called("config_get_i64") }
        extern "C" fn config_get_f64(_: HostStr<'_>, _: f64) -> f64 { called("config_get_f64") }
        extern "C" fn config_get_string(_: HostStr<'_>, _: HostStr<'_>, _: *mut c_char, _: u32) -> c_int {
            called("config_get_string")
        }
        extern "C" fn config_array_size(_: HostStr<'_>) -> u32 { called("config_array_size") }
        extern "C" fn config_set_string(_: HostStr<'_>, _: HostStr<'_>) -> c_int { called("config_set_string") }
        extern "C" fn so_symbol(_: *mut c_void, _: HostStr<'_>) -> usize { called("so_symbol") }
        extern "C" fn log(_: HostStr<'_>, _: HostStr<'_>) { called("log") }
        extern "C" fn register_module_loaded(_: u32, _: Option<RudroidModuleLoadedCallback>, _: *mut c_void) -> c_int {
            called("register_module_loaded")
        }
        extern "C" fn register_unity_profile(_: Option<&RudroidUnityProfile>) -> c_int { called("register_unity_profile") }
        extern "C" fn register_android_hooks(_: Option<&RudroidAndroidHooks>) -> c_int { called("register_android_hooks") }
        extern "C" fn register_audio_policy(_: Option<&RudroidAudioPolicy>) -> c_int { called("register_audio_policy") }
        extern "C" fn register_catalog_provider(_: Option<&CatalogProviderV0>) -> c_int {
            called("register_catalog_provider")
        }
        extern "C" fn catalog_query_int(_: HostStr<'_>, _: HostStr<'_>, _: HostStr<'_>, _: Option<&mut i64>) -> c_int {
            called("catalog_query_int")
        }
        extern "C" fn catalog_query_bool(_: HostStr<'_>, _: HostStr<'_>, _: HostStr<'_>, _: Option<&mut c_int>) -> c_int {
            called("catalog_query_bool")
        }
        extern "C" fn register_shared_vm(_: Option<crate::jni_env::JavaVm>) -> c_int { called("register_shared_vm") }
        extern "C" fn shared_vm() -> Option<crate::jni_env::JavaVm> { called("shared_vm") }
        extern "C" fn fire_module_event(_: u32) -> c_int { called("fire_module_event") }
        extern "C" fn so_symbol_all(_: *mut c_void, _: HostStr<'_>) -> usize { called("so_symbol_all") }
        unsafe extern "C" fn asset_query(_: Option<&crate::android_assets::AssetRequest<'_>>, _: *mut u8, _: u32) -> i32 { called("asset_query") }
        extern "C" fn enable_asset_packs() -> i32 { called("enable_asset_packs") }
        Self {
            abi_version: HOST_API_VERSION,
            getenv,
            config_get_bool,
            config_get_i64,
            config_get_f64,
            config_get_string: ConfigGetStringFn(config_get_string),
            config_array_size,
            config_set_string,
            so_symbol,
            log,
            register_module_loaded,
            register_unity_profile,
            register_android_hooks,
            register_audio_policy,
            register_catalog_provider,
            catalog_query_int,
            catalog_query_bool,
            register_shared_vm,
            shared_vm,
            fire_module_event,
            so_symbol_all,
            asset_query: AssetQueryFn(asset_query),
            enable_asset_packs,
        }
    }
}

/// Typed calls for every entry. A table a capability holds is the loader's
/// (or a test's), whose functions accept any `HostStr`, treat unknown module
/// bases as misses and never retain borrowed strings.
impl HostApiV0 {
    /// Boolean config value; `fallback` when absent or not a bool.
    pub fn config_bool(&self, key: &CStr, fallback: bool) -> bool {
        (self.config_get_bool)(HostStr::new(key), c_int::from(fallback)) != 0
    }

    /// Integer config value; `fallback` when absent or not an integer.
    pub fn config_i64(&self, key: &CStr, fallback: i64) -> i64 {
        (self.config_get_i64)(HostStr::new(key), fallback)
    }

    /// Float config value; `fallback` when absent or not a number.
    pub fn config_f64(&self, key: &CStr, fallback: f64) -> f64 {
        (self.config_get_f64)(HostStr::new(key), fallback)
    }

    /// String config value read through a `capacity`-byte buffer (longer
    /// values are truncated, like the reference getter); `None` when absent.
    pub fn config_string(&self, key: &CStr, capacity: usize) -> Option<String> {
        self.config_string_with_fallback(key, c"", capacity)
            .and_then(|(found, value)| found.then_some(value))
    }

    /// String getter plus its presence flag, with a truncated fallback on a
    /// miss. `None` when `capacity` cannot be represented by the C ABI.
    pub fn config_string_with_fallback(&self, key: &CStr, fallback: &CStr, capacity: usize) -> Option<(bool, String)> {
        let capacity = u32::try_from(capacity.max(1)).ok()?;
        let mut buffer = vec![0 as c_char; capacity as usize];
        // SAFETY: the getter writes at most `buffer.len()` bytes including the NUL.
        let found = unsafe {
            (self.config_get_string.0)(HostStr::new(key), HostStr::new(fallback), buffer.as_mut_ptr(), capacity)
        };
        let bytes: Vec<u8> = if found != 0 {
            buffer.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect()
        } else {
            fallback.to_bytes().iter().take(capacity as usize - 1).copied().collect()
        };
        Some((found != 0, String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Element count of the array at `key` (0 when absent).
    pub fn config_len(&self, key: &CStr) -> u32 {
        (self.config_array_size)(HostStr::new(key))
    }

    /// Log one line under `tag` (embedded NULs are escaped).
    pub fn log_line(&self, tag: &CStr, message: &str) {
        log_message(self.log, tag, message)
    }

    /// Export `name` of the guest module whose load base a module event
    /// reported; `None` on any miss.
    pub fn guest_symbol(&self, module_base: usize, name: &CStr) -> Option<usize> {
        let address = (self.so_symbol)(module_base as *mut c_void, HostStr::new(name));
        (address != 0).then_some(address)
    }

    /// Like [`Self::guest_symbol`], falling back to the module file's `.symtab`.
    pub fn guest_symbol_all(&self, module_base: usize, name: &CStr) -> Option<usize> {
        let address = (self.so_symbol_all)(module_base as *mut c_void, HostStr::new(name));
        (address != 0).then_some(address)
    }

    /// The shared session's `JavaVM`, once `android_jni_vm` registered it.
    pub fn java_vm(&self) -> Option<crate::jni_env::JavaVm> {
        (self.shared_vm)()
    }

    /// Broadcast a capability-sourced lifecycle event; 0 on delivery.
    pub fn fire_event(&self, kind: crate::ModuleKind) -> c_int {
        (self.fire_module_event)(kind as u32)
    }

    /// Subscribe `callback` to module events of `kind`. `user` is handed back
    /// verbatim and never dereferenced by the host (use
    /// [`crate::handles::HandleTable`] for per-registration state). False when
    /// the host refuses.
    pub fn on_module_loaded(
        &self,
        kind: crate::ModuleKind,
        callback: RudroidModuleLoadedCallback,
        user: *mut c_void,
    ) -> bool {
        (self.register_module_loaded)(kind as u32, Some(callback), user) == 0
    }

    /// Register the Android runtime hooks (the host copies the struct).
    pub fn register_hooks(&self, hooks: &RudroidAndroidHooks) -> bool {
        (self.register_android_hooks)(Some(hooks)) == 0
    }

    /// Register the Unity engine profile (the host copies the struct).
    pub fn register_profile(&self, profile: &RudroidUnityProfile) -> bool {
        (self.register_unity_profile)(Some(profile)) == 0
    }

    /// Register the platform audio policy (the host copies the struct).
    pub fn register_audio(&self, policy: &RudroidAudioPolicy) -> bool {
        (self.register_audio_policy)(Some(policy)) == 0
    }

    /// Register the Reply Catalog provider (the host copies the struct).
    pub fn register_catalog(&self, provider: &CatalogProviderV0) -> bool {
        (self.register_catalog_provider)(Some(provider)) == 0
    }

    /// Publish the shared session VM (first registration wins).
    pub fn register_vm(&self, vm: crate::jni_env::JavaVm) -> bool {
        (self.register_shared_vm)(Some(vm)) == 0
    }

    /// Integer reply of the registered Reply Catalog; `None` on a miss.
    pub fn catalog_int(&self, class: &CStr, method: &CStr, signature: &CStr) -> Option<i64> {
        let mut value = 0;
        let hit = (self.catalog_query_int)(
            HostStr::new(class),
            HostStr::new(method),
            HostStr::new(signature),
            Some(&mut value),
        );
        (hit == 1).then_some(value)
    }

    /// Boolean reply of the registered Reply Catalog; `None` on a miss.
    pub fn catalog_bool(&self, class: &CStr, method: &CStr, signature: &CStr) -> Option<bool> {
        let mut value = 0;
        let hit = (self.catalog_query_bool)(
            HostStr::new(class),
            HostStr::new(method),
            HostStr::new(signature),
            Some(&mut value),
        );
        (hit == 1).then_some(value != 0)
    }
}

#[cfg(test)]
#[path = "../tests/unit/host_api_copy_semantics_tests.rs"]
mod copy_semantics_tests;
