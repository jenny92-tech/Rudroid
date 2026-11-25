//! INPUT: exact 80700 files, explicit isolated-profile config and Il2cppReady
//! OUTPUT: optional original-data/time/login sources and one original Title entry
//! POS: title-only offline candidate; default-off and not gameplay acceptance
#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_void, CStr, CString};

#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::{cell::RefCell, rc::Rc};

use rudroid_common::arm64_detour::{self, HookFn, InstalledDetour, InstalledQuiescentDetour, Original};
use rudroid_common::host_api::HostApiV0;
use rudroid_unity_common::il2cpp::{CheckedMethod, Class, Field, GcHandles, GcRoot, Il2cpp, ManagedArg, Method};
use rudroid_common::jni_env::{JavaVm, JniEnv};
use rudroid_common::mapped_memory::{MappedMemory, MappedRegions};
use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::ModuleKind;

use crate::sha256;

#[path = "offline_isolation.rs"]
mod isolation;
use isolation::{prepare_inputs, require_network_isolation};
#[cfg(test)]
use isolation::{bounded_file, isolated_network, local_id, prepare_profile};

type Ptr = *mut c_void;
type RefSource = unsafe extern "C-unwind" fn(Ptr, Ptr) -> Ptr;
type TimeSource = unsafe extern "C-unwind" fn(i32, Ptr) -> Ptr;
type LoginSource = unsafe extern "C-unwind" fn(Ptr, Ptr);
type UpdateSource = unsafe extern "C-unwind" fn(Ptr, Ptr);
type PlatformStringSource = unsafe extern "C-unwind" fn(Ptr) -> Ptr;

const TAG: &CStr = c"SOUL_KNIGHT_OFFLINE";
const MANAGER: &str = "RGScript.Util.LifeCycle.LifeCycleManager";
const MANIFEST: &str = "RGScript.Util.HotUpdate.HotUpdateManifest";
const CONFIGURATION: &str = "RGScript.Util.ForcePatch.ForcePatchDownloader.Configuration";
const DTO: &str = "System.DateTimeOffset";
const IRESULT: &str = "ChillyRoom.Services.Core.IResult<System.DateTimeOffset,ChillyRoom.Services.Core.Failure<System.Int32>>";
const RESULT: &str = "ChillyRoom.Services.Core.Result<System.DateTimeOffset,ChillyRoom.Services.Core.Failure<System.Int32>>";
const PLAYING: &str = "RGScript.Util.LifeCycle.PlayingState";
const TITLE_SCENE: &CStr = c"Title";
const LOAD_SCENE_RVA: usize = 0xed57e68;
const LOAD_SCENE_PREFIX: &str = "fe57bea9f44f01a974b40090755d00f0886e6f39b57240f9f30300aac8000037";
const WEBVIEW_CTOR_SHA: &str = "0cc11ced139f2c5c337b873dae8a03f0c42dba692b8457b2fded4a71c3e20ae9";
const LIB_SHA: &str = "f8a4c527ec910c341189f2697e543a285e2b6a7873d475a456c24dba137afcc3";
const MANIFEST_SHA: &str = "73aa4ce7966c8a8889c7d007067507f543c88690490fc11c148cb2554515112c";
const MANIFEST_PATH: &str = "assetpacks/UnityDataAssetPack/assets/AssetBundles/hot_update_manifest.json";
const MARKER: &str = ".rudroid-soul-knight-offline-profile";
// The original eight-entry 2.7.0/100 fallback opens every entry. Explicit
// offline policy closes only enum 20, FallbackCloudPackageRestore: the original
// WaitCloudPackageState then takes its disabled-entry NextState branch instead
// of awaiting a remote userdata restore. No cloud Task/result is substituted.
const OFFLINE_ENTRIES: [(&CStr, bool); 8] = [
    (c"MultiPlayer", true),
    (c"MultiPlayerRemote", true),
    (c"Sandbox", true),
    (c"Mall", true),
    (c"CheckIn", true),
    (c"Review", true),
    (c"NewAnnouncement", true),
    (c"FallbackCloudPackageRestore", false),
];
const REQUIRED: &[&str] = &[
    "il2cpp_domain_get", "il2cpp_domain_get_assemblies", "il2cpp_assembly_get_image",
    "il2cpp_image_get_name", "il2cpp_class_from_name", "il2cpp_class_get_type",
    "il2cpp_class_get_parent", "il2cpp_class_get_nested_types", "il2cpp_class_get_methods",
    "il2cpp_class_get_method_from_name", "il2cpp_class_get_field_from_name",
    "il2cpp_method_get_name", "il2cpp_method_get_class", "il2cpp_method_get_flags",
    "il2cpp_method_get_param_count", "il2cpp_method_get_param", "il2cpp_method_get_return_type",
    "il2cpp_method_is_generic", "il2cpp_method_is_inflated", "il2cpp_type_get_name",
    "il2cpp_type_get_type", "il2cpp_type_is_byref", "il2cpp_free",
    "il2cpp_class_is_valuetype", "il2cpp_class_value_size", "il2cpp_class_is_assignable_from",
    "il2cpp_field_get_parent", "il2cpp_field_get_type", "il2cpp_field_get_flags",
    "il2cpp_field_get_offset", "il2cpp_field_static_get_value", "il2cpp_field_get_value",
    "il2cpp_field_set_value", "il2cpp_field_set_value_object", "il2cpp_type_get_object",
    "il2cpp_object_new", "il2cpp_object_get_class", "il2cpp_object_unbox",
    "il2cpp_string_new", "il2cpp_string_new_utf16", "il2cpp_string_length", "il2cpp_string_chars", "il2cpp_runtime_invoke",
    "il2cpp_thread_current", "il2cpp_gchandle_new", "il2cpp_gchandle_get_target", "il2cpp_gchandle_free",
];

static HOST: OnceLock<HostApiV0> = OnceLock::new();
static INPUTS: OnceLock<Inputs> = OnceLock::new();
static MODULE: AtomicUsize = AtomicUsize::new(0);
static ATTEMPTED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static CONFIG_ORIGINAL: Original<RefSource> = Original::new();
static MANIFEST_ORIGINAL: Original<RefSource> = Original::new();
static TIME_ORIGINAL: Original<TimeSource> = Original::new();
static LOGIN_ORIGINAL: Original<LoginSource> = Original::new();
static UPDATE_ORIGINAL: Original<UpdateSource> = Original::new();
static MANAGER_UPDATE_ORIGINAL: Original<UpdateSource> = Original::new();
static ANDROID_ID_ORIGINAL: Original<PlatformStringSource> = Original::new();
#[path = "offline_trace.rs"]
mod diagnostics;
use diagnostics::*;

struct Inputs {
    manifest: CString,
    profile_id: CString,
}

/// A Java attachment of this already-verified IL2CPP owner, never a way to
/// acquire managed ownership. Pending real exceptions keep their environment
/// attached for FAILED propagation/owner teardown; they are never cleared.
struct SourceJavaEnv { vm: JavaVm, env: JniEnv, detach: bool }
impl SourceJavaEnv {
    fn enter(host: &HostApiV0) -> Result<Self, &'static str> {
        let vm = host.java_vm().ok_or("Android ID shared JavaVM unavailable")?;
        if let Some(env) = vm.get_env(0x0001_0006) { return Ok(Self { vm, env, detach: false }); }
        let env = vm.attach_current_thread().ok_or("Android ID owner JNI attachment failed")?;
        Ok(Self { vm, env, detach: true })
    }
    fn finish<T>(mut self, result: Result<T, &'static str>) -> Result<T, &'static str> {
        if self.env.exception_check() {
            self.detach = false;
            return Err("Android ID Java exception");
        }
        if self.detach {
            self.detach = false;
            if self.vm.detach_current_thread() != 0 && result.is_ok() { return Err("Android ID JNI detach failed"); }
        }
        result
    }
}
impl Drop for SourceJavaEnv {
    fn drop(&mut self) {
        if self.detach && !self.env.exception_check() { let _ = self.vm.detach_current_thread(); }
    }
}

#[derive(Clone, Copy)]
struct Target {
    namespace: &'static CStr,
    class: &'static CStr,
    method: &'static CStr,
    static_: bool,
    parameters: &'static [&'static str],
    returns: &'static str,
    rva: usize,
    prefix: &'static str,
}

const CONFIG: Target = Target {
    namespace: c"RGScript.Util.ForcePatch", class: c"ForcePatchDownloader", method: c"GetConfiguration",
    static_: false, parameters: &[],
    returns: "System.Threading.Tasks.Task<RGScript.Util.ForcePatch.ForcePatchDownloader.Configuration>",
    rva: 0x7d3e510,
    prefix: "ff4301d1fe1b00f9f44f04a9d43304d088de7339f30300aae8010037e0e203d0",
};
const REMOTE_MANIFEST: Target = Target {
    namespace: c"RGScript.Util.HotUpdate", class: c"HotUpdateManager", method: c"DownloadRemoteConfig",
    static_: true, parameters: &["System.String"],
    returns: "System.Threading.Tasks.Task<RGScript.Util.HotUpdate.HotUpdateManifest>",
    rva: 0x7d398b0,
    prefix: "ff4301d1fe5703a9f44f04a9f43304f088527339f30300aae801003700e303f0",
};
const TIME: Target = Target {
    namespace: c"", class: c"NetTime", method: c"GetNetTimeForChillyRoomServer", static_: true,
    parameters: &["System.Int32"],
    returns: "System.Threading.Tasks.Task<ChillyRoom.Services.Core.IResult<System.DateTimeOffset,ChillyRoom.Services.Core.Failure<System.Int32>>>",
    rva: 0x6514998,
    prefix: "ffc301d1fe2b00f9f44f06a9d4f404b088727839f303002ae8010037c09f04f0",
};
const LOGIN: Target = Target {
    namespace: c"RGScript.Util.LifeCycle.StateSundry", class: c"NewAccountLoginAgent", method: c"Login",
    static_: false, parameters: &[], returns: "System.Void", rva: 0x7d35da4,
    prefix: "fe0f1ef8f44f01a9143404f088ce7239f30300aac800003720df03d000c046f9",
};
const STATE_UPDATE: Target = Target {
    namespace: c"RGScript.Util.LifeCycle", class: c"BaseLifeState", method: c"Update",
    static_: false, parameters: &[], returns: "System.Void", rva: 0x7d2d368,
    prefix: "fe4fbfa9f30300aa809695522000a072e1031faad0dd5b944001003680969552",
};
const MANAGER_UPDATE: Target = Target {
    namespace: c"RGScript.Util.LifeCycle", class: c"LifeCycleManager", method: c"Update",
    static_: false, parameters: &[], returns: "System.Void", rva: 0x7d2b37c,
    prefix: "fe0f1ef8f44f01a9743404b088567139f30300aac800003780e303b0003c40f9",
};
const ANDROID_ID: Target = Target {
    namespace: c"CoreKit.Util", class: c"NativeUtil", method: c"GetAndroidID",
    static_: true, parameters: &[], returns: "System.String", rva: 0x98ab5b4,
    prefix: "fe57bea9f44f01a9d35803f0150303f068427639b52244f9a802003780020390",
};

/// Validation runs before protected libraries and before any hook registration.
/// No offline config means no additional file reads, subscriptions or mutation.
pub fn arm(host: &HostApiV0) -> Result<(), String> {
    if !host.config_bool(c"game_patches.soul_knight.offline", false) { return Ok(()); }
    if !host.has_capability(c"il2cpp_patch") {
        return Err("offline mode requires explicitly selected il2cpp_patch".to_owned());
    }
    require_network_isolation()?;
    let inputs = prepare_inputs(host)?;
    INPUTS.set(inputs).map_err(|_| "offline mode was already armed")?;
    HOST.set(*host).map_err(|_| "offline host was already armed")?;
    if !host.on_module_loaded(ModuleKind::Il2cpp, mapped, std::ptr::null_mut())
        || !host.on_module_loaded(ModuleKind::Il2cppReady, ready, std::ptr::null_mut())
        || !host.on_module_loaded(ModuleKind::UnityFrame, propagate_failure, std::ptr::null_mut())
        || !host.on_module_loaded(ModuleKind::UnityMaintenance, propagate_failure, std::ptr::null_mut())
        || !host.on_module_loaded(ModuleKind::UnityStopping, stopping, std::ptr::null_mut()) {
        return Err("offline lifecycle subscription refused".to_owned());
    }
    host.log_line(TAG, "explicit isolated offline candidate armed; gameplay and save/reload remain unverified");
    Ok(())
}

extern "C" fn mapped(event: Option<&RudroidModuleEvent>, _: Ptr) {
    if let Some(event) = event.filter(|e| e.kind == ModuleKind::Il2cpp as u32) {
        MODULE.store(event.base, Ordering::Release);
    }
}

extern "C" fn ready(_: Option<&RudroidModuleEvent>, _: Ptr) {
    if ATTEMPTED.swap(true, Ordering::AcqRel) { return; }
    let Some(host) = HOST.get() else { return; };
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    // SAFETY: same-kit Il2cppReady pins both images and is the idle pre-game
    // installation window. Metadata/source identities are checked below.
    if let Err(reason) = unsafe { install(host, MODULE.load(Ordering::Acquire)) } {
        fail(reason);
    }
    #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
    fail("offline mode requires Linux ARM64");
}

fn fail(reason: &'static str) {
    if let Some(runtime) = RUNTIME.get() { runtime.stopped.store(true, Ordering::Release); }
    if !FAILED.swap(true, Ordering::AcqRel) {
        if let Some(host) = HOST.get() {
            host.log_error(TAG, &format!("offline mode failed closed: {reason}"));
        }
    }
    raise_failure();
}

/// A hook may run on UnityMain while nativeRender belongs to another attached
/// profile thread. Preserve the failure atomically and also propagate it at
/// that profile's real frame/maintenance callback. The existing player loop
/// checks pending JNI exceptions and then runs its normal finish_run teardown;
/// this code never broadcasts a fictitious stop event or exits from a hook.
extern "C" fn propagate_failure(event: Option<&RudroidModuleEvent>, _: Ptr) {
    if event.is_some_and(|event| event.kind == ModuleKind::UnityFrame as u32) && trace().enabled {
        // Profile-thread diagnostics read atomics only. No IL2CPP call, object
        // access, attachment or scene work may move into this callback.
        let claimed = RUNTIME.get().is_some_and(|runtime| runtime.scene_once.claimed());
        if let Some(line) = trace().heartbeat(claimed) { trace_line(&line); }
    }
    if FAILED.load(Ordering::Acquire)
        && event.is_some_and(|event| event.kind == ModuleKind::UnityFrame as u32
            || event.kind == ModuleKind::UnityMaintenance as u32) {
        raise_failure();
    }
}

fn raise_failure() {
    let Some(env) = HOST.get().and_then(|host| host.java_vm()).and_then(|vm| vm.get_env(0x0001_0006)) else {
        return; // Keep FAILED for the real attached profile callback.
    };
    if env.exception_check() { return; } // Preserve the original failure identity.
    let exception = env.find_class(c"java/lang/IllegalStateException");
    if !exception.is_null() && !env.exception_check() {
        env.throw_new(exception, c"Soul Knight offline initialization failed; remote fallback is forbidden");
    }
    env.delete_local_ref(exception);
}

extern "C" fn stopping(_: Option<&RudroidModuleEvent>, _: Ptr) {
    if let Some(runtime) = RUNTIME.get() {
        runtime.stopped.store(true, Ordering::Release);
        if runtime.api.thread_id() == Some(runtime.owner) {
            // UnityStopping is emitted on the owner before nativeDone; code
            // and IL2CPP stay pinned while these engine-owned roots are freed.
            let old = CACHE.with(|slot| std::mem::take(&mut **slot.borrow_mut()));
            drop(old);
        }
    }
}

// Managed handles stay on their attached owner. Snapshot the Rc before guest calls.
// TLS teardown can run after VM detach if shutdown was interrupted. Only the
// explicit owner stopping callback may free handles through IL2CPP.
thread_local! { static CACHE: RefCell<std::mem::ManuallyDrop<Cache>> = RefCell::new(std::mem::ManuallyDrop::new(Cache::default())); }

#[derive(Default)]
struct Cache { configuration: Option<Rc<GcRoot>>, manifest: Option<Rc<GcRoot>>, time: Option<GcRoot>, android_id: Option<GcRoot> }

#[derive(Default)]
struct SceneOnce(AtomicBool);
impl SceneOnce {
    fn claimed(&self) -> bool { self.0.load(Ordering::Acquire) }
    fn run<T>(&self, ready: Result<Option<T>, &'static str>, invoke: impl FnOnce(T) -> Result<(), &'static str>)
        -> Result<bool, &'static str> {
        let Some(roots) = ready? else { return Ok(false); };
        // Claim before allocation/invocation: errors and reentrant callbacks
        // cannot retry scene loading or fall through to an online provider.
        if self.0.swap(true, Ordering::AcqRel) { return Ok(false); }
        invoke(roots)?;
        Ok(true)
    }
}

struct SceneEntry {
    manager_class: Class,
    playing_class: Class,
    life_state: Field,
    ready: [Field; 3],
    load: CheckedMethod,
}

#[path = "offline_patches.rs"]
mod patches;
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use patches::{WebViewWarmup, NotificationUnavailable, AnalyticsUnavailable};

impl SceneEntry {
    fn prepare(api: &Il2cpp) -> Result<Self, &'static str> {
        let manager = class(api, c"Assembly-CSharp.dll", c"RGScript.Util.LifeCycle", c"LifeCycleManager")?;
        let playing = class(api, c"Assembly-CSharp.dll", c"RGScript.Util.LifeCycle", c"PlayingState")?;
        if api.class_type(playing).and_then(|ty| api.type_name(ty)).as_deref() != Some(PLAYING) {
            return Err("offline PlayingState type mismatch");
        }
        let field = |name: &CStr, ty: &str, offset: usize| {
            let f = api.field(manager, name).ok_or("offline scene readiness field missing")?;
            if api.field_parent(f) != Some(manager) || api.field_flags(f) != Some(1)
                || api.field_offset(f) != Some(offset)
                || api.field_type(f).and_then(|t| api.type_name(t)).as_deref() != Some(ty) {
                return Err("offline scene readiness field mismatch");
            }
            Ok(f)
        };
        let life_state = field(c"_lifeState", "RGScript.Util.LifeCycle.ILifeState", 256)?;
        let ready = [field(c"<IsInitSuccess>k__BackingField", "System.Boolean", 24)?,
            field(c"<IsResourcesPrepared>k__BackingField", "System.Boolean", 25)?,
            field(c"<DataIsReady>k__BackingField", "System.Boolean", 26)?];
        let scene = class(api, c"UnityEngine.CoreModule.dll", c"UnityEngine.SceneManagement", c"SceneManager")?;
        let load = api.matching_checked_method(scene, c"LoadScene", true, &["System.String"], "System.Void")
            .ok_or("original Title scene signature mismatch")?;
        if !api.ordinary_method(load.metadata()) || api.method_class(load.metadata()) != Some(scene)
            || api.method_flags(load.metadata()) != Some(150) {
            return Err("original Title scene method identity mismatch");
        }
        code_guard(api, load.metadata(), LOAD_SCENE_RVA, LOAD_SCENE_PREFIX)?;
        Ok(Self { manager_class: manager, playing_class: playing, life_state, ready, load })
    }
}

struct Runtime {
    api: Il2cpp,
    gc: GcHandles,
    owner: usize,
    installed: AtomicBool,
    stopped: AtomicBool,
    busy: [AtomicBool; 6],
    login_claimed: AtomicBool,
    scene_once: SceneOnce,
    scene_entry: SceneEntry,
    manager_is_stop: Option<Field>,
    sources: [Method; 4],
}

/// Checked owner-thread access for managed source callbacks.
struct OwnerScope<'a> {
    runtime: &'a Runtime,
    thread: std::marker::PhantomData<Rc<()>>,
}

impl std::ops::Deref for OwnerScope<'_> {
    type Target = Runtime;
    fn deref(&self) -> &Runtime { self.runtime }
}

struct Enter<'a>(&'a AtomicBool);
impl Drop for Enter<'_> { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }

impl Runtime {
    fn live(&self) -> Result<(), &'static str> {
        if FAILED.load(Ordering::Acquire) { return Err("offline runtime failed"); }
        if self.stopped.load(Ordering::Acquire) { return Err("offline runtime stopping"); }
        if self.api.thread_id() != Some(self.owner) { return Err("offline callback is not on its owner thread"); }
        Ok(())
    }
    fn owner_scope(&self) -> Result<OwnerScope<'_>, &'static str> {
        self.live()?;
        if !self.installed.load(Ordering::Acquire) { return Err("offline hook transaction incomplete"); }
        Ok(OwnerScope { runtime: self, thread: std::marker::PhantomData })
    }
}

impl OwnerScope<'_> {
    fn enter(&self, index: usize) -> Result<Enter<'_>, &'static str> {
        if self.busy[index].swap(true, Ordering::AcqRel) { return Err("offline source reentered"); }
        Ok(Enter(&self.busy[index]))
    }
}

fn select_no_near<T>(entry: usize, near: Result<T, arm64_detour::DetourError>,
    wide: impl FnOnce() -> Result<T, arm64_detour::DetourError>) -> Result<T, arm64_detour::DetourError> {
    match near {
        Err(arm64_detour::DetourError::NoNearTrampoline { target }) if target == entry => wide(),
        other => other,
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
enum SourceDetour { Entry(InstalledDetour), Quiescent(InstalledQuiescentDetour), Warmup(WebViewWarmup), Notification(NotificationUnavailable), Analytics(AnalyticsUnavailable) }

/// Prefer the existing one-word redirect. Only precise allocation exhaustion
/// before target/typed-slot publication permits a guarded wider startup write.
/// # Safety
/// Il2cppReady keeps the entire exact source's first 16 bytes idle, with no
/// interior entry, until this transaction and typed-original publication end.
/// The target/replacement have type F and all code stays process-pinned.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
unsafe fn install_near_source<F: HookFn>(runtime: &Runtime, original: &Original<F>,
    source: Target, method: Method, flags: u32, replacement: F) -> Result<SourceDetour, arm64_detour::DetourError> {
    let entry = runtime.api.base() + source.rva;
    // SAFETY: caller's pinned exact ABI and pre-game idle/publication contract.
    let near = unsafe { original.install(entry, replacement) }.map(SourceDetour::Entry);
    select_no_near(entry, near, || {
        let guard = || -> Result<(), &'static str> {
            runtime.live()?;
            if runtime.installed.load(Ordering::Acquire) || source.prefix.len() != 64 {
                return Err("offline wide fallback outside its startup guard");
            }
            // Re-resolve the same declaring class/full overload and ordinary
            // MethodInfo, exact RVA/32-byte runtime prefix and saved flags.
            if target(&runtime.api, source)? != method || runtime.api.method_flags(method) != Some(flags) {
                return Err("offline wide fallback metadata identity changed");
            }
            let regions = MappedRegions::snapshot().map_err(|_| "offline source mappings unavailable")?;
            require_executable_source(&regions, entry, 32).map_err(|_| "offline wide fallback source mapping is not RX")?;
            Ok(())
        };
        guard().map_err(|reason| arm64_detour::DetourError::Protection(reason.to_owned()))?;
        // SAFETY: all guards just revalidated on the original Ready owner;
        // the caller keeps all 16 bytes idle through typed publication.
        let wide = unsafe { original.install_quiescent_tracked(entry, replacement) }?;
        if rudroid_common::logging::diagnostics_enabled() {
            trace_line(&format!("offline source quiescent fallback installed method={}", source.method.to_string_lossy()));
        }
        Ok(SourceDetour::Quiescent(wide))
    })
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
unsafe fn install(host: &HostApiV0, base: usize) -> Result<(), &'static str> {
    if base == 0 { return Err("IL2CPP module is unavailable"); }
    // SAFETY: caller's pinned same-kit module contract; runtime operations
    // additionally require the precise signatures and exact source prefixes.
    let api = unsafe { Il2cpp::bind(base, host.so_symbol, REQUIRED) }.map_err(|_| "required IL2CPP exports missing")?;
    if !api.has("il2cpp_class_from_type") && !api.has("il2cpp_class_from_il2cpp_type") {
        return Err("IL2CPP class-from-type export unavailable");
    }
    let application = class(&api, c"UnityEngine.CoreModule.dll", c"UnityEngine", c"Application")?;
    if api.unity_version(application).as_deref() != Ok("2022.3.62f2") {
        return Err("offline Unity build mismatch");
    }
    let gc = api.verified_gc_handles(application)?;
    let owner = api.thread_id().ok_or("IL2CPP owner thread is unavailable")?;
    observe_native_util_metadata(host, &api);
    observe_webview_metadata(host, &api);
    let sources = [target(&api, CONFIG)?, target(&api, REMOTE_MANIFEST)?, target(&api, TIME)?, target(&api, LOGIN)?];
    let state_update = target(&api, STATE_UPDATE)?;
    if api.method_flags(state_update) != Some(454) { return Err("offline state Update flags mismatch"); }
    let mut source_flags = [0; 5];
    for (index, method) in sources.iter().chain(std::iter::once(&state_update)).enumerate() {
        source_flags[index] = api.method_flags(*method).ok_or("offline source flags unavailable")?;
    }
    let regions = MappedRegions::snapshot().map_err(|_| "offline source mappings unavailable")?;
    let scene_entry = SceneEntry::prepare(&api)?;
    // Optional metadata/code preflight is read-only and never blocks the
    // functional sources. Once writing starts it shares their full rollback.
    let manager_observer = if trace().enabled {
        (|| {
            let method = target(&api, MANAGER_UPDATE).ok()?;
            if api.method_flags(method) != Some(486) { return None; }
            require_executable_source(&regions, base + MANAGER_UPDATE.rva, 32).ok()?;
            let field = api.field(scene_entry.manager_class, c"<IsStop>k__BackingField")?;
            if api.field_parent(field) != Some(scene_entry.manager_class) || api.field_flags(field) != Some(1)
                || api.field_offset(field) != Some(48)
                || api.field_type(field).and_then(|ty| api.type_name(ty)).as_deref() != Some("System.Boolean") { return None; }
            Some((method, field))
        })()
    } else { None };
    if trace().enabled && manager_observer.is_none() {
        if let Some(line) = trace().observe_manager(ManagerObservation::Stage("manager-update preflight-unavailable")) { trace_line(&line); }
    }
    let android_id = target_in_image(&api, c"com.chillyroom.corekit.Runtime.dll", ANDROID_ID)?;
    if api.method_flags(android_id) != Some(150) { return Err("Android ID source flags mismatch"); }
    require_executable_source(&regions, base + ANDROID_ID.rva, 32)?;
    let webview_warmup = WebViewWarmup::prepare(&api, &regions)?;
    let notification_unavailable = NotificationUnavailable::prepare(&api, &regions)?;
    let analytics_unavailable = AnalyticsUnavailable::prepare(&api, &regions)?;
    let runtime = Runtime {
        api, gc, owner, installed: AtomicBool::new(false), stopped: AtomicBool::new(false),
        busy: std::array::from_fn(|_| AtomicBool::new(false)), login_claimed: AtomicBool::new(false),
        scene_once: SceneOnce::default(), scene_entry, manager_is_stop: manager_observer.map(|(_, field)| field),
        sources,
    };
    RUNTIME.set(runtime).map_err(|_| "offline runtime already installed")?;
    let runtime = RUNTIME.get().expect("published runtime");
    let mut written = Vec::new();
    // SAFETY: every entry has its matching complete native ABI and exact
    // build prefix; Il2cppReady keeps six sources, the optional passive manager
    // hook and three code policies idle
    // until the complete transaction and original publication end.
    let result = unsafe { (|| {
        written.push(install_near_source(runtime, &CONFIG_ORIGINAL, CONFIG, sources[0], source_flags[0], configuration_hook as RefSource).map_err(|error| {
            host.log_line(TAG, &format!("configuration source detour error: {error}"));
            "configuration detour failed"
        })?);
        written.push(install_near_source(runtime, &MANIFEST_ORIGINAL, REMOTE_MANIFEST, sources[1], source_flags[1], manifest_hook as RefSource).map_err(|error| {
            host.log_line(TAG, &format!("manifest source detour error: {error}"));
            "manifest detour failed"
        })?);
        written.push(install_near_source(runtime, &TIME_ORIGINAL, TIME, sources[2], source_flags[2], time_hook as TimeSource).map_err(|error| {
            host.log_line(TAG, &format!("time source detour error: {error}"));
            "time detour failed"
        })?);
        written.push(install_near_source(runtime, &LOGIN_ORIGINAL, LOGIN, sources[3], source_flags[3], login_hook as LoginSource).map_err(|error| {
            host.log_line(TAG, &format!("login source detour error: {error}"));
            "login detour failed"
        })?);
        written.push(install_near_source(runtime, &UPDATE_ORIGINAL, STATE_UPDATE, state_update, source_flags[4], update_hook as UpdateSource).map_err(|error| {
            host.log_line(TAG, &format!("state Update source detour error: {error}"));
            "state Update detour failed"
        })?);
        if let Some((method, _)) = manager_observer {
            written.push(install_near_source(runtime, &MANAGER_UPDATE_ORIGINAL, MANAGER_UPDATE, method, 486, manager_update_hook as UpdateSource).map_err(|error| {
                host.log_line(TAG, &format!("manager Update observer detour error: {error}"));
                "manager Update observer detour failed"
            })?);
        }
        // Only this exact, fully guarded source needs the wider startup patch:
        // its complete 16-byte prefix is idle in this Il2cppReady transaction.
        written.push(SourceDetour::Quiescent(ANDROID_ID_ORIGINAL.install_quiescent_tracked(base + ANDROID_ID.rva, android_id_hook as PlatformStringSource).map_err(|error| {
            host.log_line(TAG, &format!("Android ID source detour error: {error}"));
            "Android ID source detour failed"
        })?));
        webview_warmup.patch(true).map_err(|_| "WebView eager warmup deferral failed")?;
        written.push(SourceDetour::Warmup(webview_warmup));
        notification_unavailable.patch(true).map_err(|_| "notification unavailable policy installation failed")?;
        written.push(SourceDetour::Notification(notification_unavailable));
        analytics_unavailable.patch(true).map_err(|_| "analytics unavailable policy installation failed")?;
        written.push(SourceDetour::Analytics(analytics_unavailable));
        Ok::<(), &'static str>(())
    })() };
    if let Err(reason) = result {
        let mut rollback_failed = false;
        for hook in written.iter().rev() {
            // SAFETY: installation is still idle; each record is our just-written detour.
            rollback_failed |= unsafe { match hook {
                SourceDetour::Entry(hook) => arm64_detour::restore(hook),
                SourceDetour::Quiescent(hook) => arm64_detour::restore_quiescent(hook),
                SourceDetour::Warmup(patch) => patch.patch(false),
                SourceDetour::Notification(patch) => patch.patch(false),
                SourceDetour::Analytics(patch) => patch.patch(false),
            } }.is_err();
        }
        // A still-installed hook must retain its callable original.
        if rollback_failed { return Err("offline rollback failed"); }
        CONFIG_ORIGINAL.clear(); MANIFEST_ORIGINAL.clear(); TIME_ORIGINAL.clear(); LOGIN_ORIGINAL.clear();
        UPDATE_ORIGINAL.clear();
        MANAGER_UPDATE_ORIGINAL.clear();
        ANDROID_ID_ORIGINAL.clear();
        return Err(reason);
    }
    RUNTIME.get().expect("published runtime").installed.store(true, Ordering::Release);
    if rudroid_common::logging::diagnostics_enabled() { host.log_line(TAG, "WebView eager warmup deferral installed; original lazy getter retained"); }
    if rudroid_common::logging::diagnostics_enabled() { host.log_line(TAG, "notification unavailable Initialize policy installed; no manager or scheduler provided"); }
    if rudroid_common::logging::diagnostics_enabled() { host.log_line(TAG, "analytics unavailable Init policy installed; no telemetry success or callbacks provided"); }
    if manager_observer.is_some() { host.log_line(TAG, "manager Update passive observer installed; no state or scene mutation"); }
    host.log_line(TAG, "five exact offline sources and state Update continuation installed; gameplay still requires acceptance");
    Ok(())
}

/// Three fixed image candidates and two fixed method names only. This optional
/// install-owner observation never invokes a getter, copies code or reads ID
/// values. Entry-minus-base is metadata, not proof of an executable mapping.
fn observe_native_util_metadata(host: &HostApiV0, api: &Il2cpp) {
    if !rudroid_common::logging::diagnostics_enabled() { return; }
    let mut complete = true;
    let images = match api.inventory() {
        Ok(images) => images,
        Err(_) => { complete = false; Vec::new() },
    };
    // Inventory indexes image names only; class/method lookup below never
    // falls back outside the three literal candidates.
    let images: Vec<_> = images.into_iter().filter_map(|image| {
        api.image_name(image).map(|name| (image, name)).or_else(|| { complete = false; None })
    }).collect();
    let names = ["GetAndroidID", "GetUniqueId"];
    let mut found: [Vec<(&str, u32, usize)>; 2] = std::array::from_fn(|_| Vec::new());
    for image_name in ["Assembly-CSharp.dll", "Assembly-CSharp-firstpass.dll", "com.chillyroom.corekit.Runtime.dll"] {
        let candidates: Vec<_> = images.iter().filter(|(_, name)| name.as_str() == image_name).collect();
        if candidates.len() > 1 { complete = false; continue; }
        let Some((image, _)) = candidates.first().copied() else { continue; };
        let Some(class) = api.class_from_name(*image, c"CoreKit.Util", c"NativeUtil") else { continue; };
        let Some(methods) = api.methods(class) else { complete = false; continue; };
        for method in methods {
            let Some(name) = api.method_name(method) else { complete = false; continue; };
            let Some(index) = names.iter().position(|wanted| *wanted == name.as_str()) else { continue; };
            if !api.ordinary_method(method) || api.method_class(method) != Some(class)
                || api.checked_method(method, true, &[], "System.String").is_none() { continue; }
            match (api.method_flags(method), api.method_entry(method).and_then(|entry| entry.checked_sub(api.base()))) {
                (Some(flags), Some(delta)) => found[index].push((image_name, flags, delta)),
                _ => complete = false,
            }
        }
    }
    // At most two records, once in the pre-game installation window; none of
    // these missing/ambiguous observations affect installation or source flow.
    for (name, found) in names.into_iter().zip(found) {
        if complete && found.len() == 1 {
            let (image, flags, delta) = found[0];
            host.log_line(TAG, &format!("NativeUtil {name} image={image} flags={flags} metadata-entry-delta={delta:#x}; RX unverified"));
        } else {
            host.log_line(TAG, &format!("NativeUtil {name} metadata unavailable"));
        }
    }
}

/// One diagnostic-only declaration snapshot: two exact images, four named
/// classes, no object/field values, code reads, cctors or business invocations.
/// The footer is unconditional so a private collector can stop after sampling.
#[derive(Default)]
struct WebViewMetadataBudget { lines: usize, bytes: usize, truncated: bool }
impl WebViewMetadataBudget {
    fn accept(&mut self, line: &str) -> bool {
        // Reserve one line and 128 bytes for the mandatory terminal marker.
        if self.lines >= 63 || line.len() > 1536 || self.bytes + line.len() + 1 > 24 * 1024 - 128 {
            self.truncated = true; return false;
        }
        self.lines += 1; self.bytes += line.len() + 1; true
    }
}

fn webview_method_selected(class: usize, name: &str) -> bool {
    match class {
        0 => name == ".ctor" || name.starts_with("get_") || name.starts_with("set_"),
        2 => matches!(name, ".ctor" | "SetWebViewDefaultSetting"),
        3 => matches!(name, "GetOrCreateUniWebView" | "BeforeUniWebviewCreateSSetting"
            | "AfterUniWebviewCreateSetting" | "SetGameOrientationAndSize"),
        _ => false,
    }
}

fn webview_method_declaration(api: &Il2cpp, method: Method) -> Option<String> {
    let name = api.method_name(method)?;
    let count = api.method_param_count(method)?;
    if name.len() > 128 || count > 8 || !api.ordinary_method(method) { return None; }
    let returns = api.type_name(api.method_return_type(method)?)?;
    if returns.len() > 512 { return None; }
    let mut parameters = Vec::new();
    for index in 0..count {
        let name = api.type_name(api.method_param(method, index)?)?;
        if name.len() > 512 { return None; }
        parameters.push(name);
    }
    let flags = api.method_flags(method)?;
    let entry = api.method_entry(method).and_then(|entry| entry.checked_sub(api.base()))
        .map_or_else(|| "unavailable".to_owned(), |entry| format!("{entry:#x}"));
    Some(format!("{} {returns} {name}({}) flags={flags} metadata-entry-delta={entry}; RX unverified",
        if flags & 0x10 != 0 { "static" } else { "instance" }, parameters.join(",")))
}

fn observe_webview_metadata(host: &HostApiV0, api: &Il2cpp) {
    if !rudroid_common::logging::diagnostics_enabled() { return; }
    let specs = [
        (c"CoreKit.Config", c"GameConfig", "CoreKit.Config.GameConfig"),
        (c"ChillyRoom.Webview", c"WebviewSetting", "ChillyRoom.Webview.WebviewSetting"),
        (c"ChillyRoom.Services.Core", c"ChillyRoomServiceCore", "ChillyRoom.Services.Core.ChillyRoomServiceCore"),
        (c"ChillyRoom.Webview", c"WebViewClient", "ChillyRoom.Webview.WebViewClient"),
    ];
    let mut budget = WebViewMetadataBudget::default();
    let mut unavailable = false;
    let emit = |budget: &mut WebViewMetadataBudget, line: String| {
        if budget.accept(&line) { host.log_line(TAG, &line); }
    };
    let images = api.inventory().ok().and_then(|images| images.into_iter()
        .map(|image| Some((image, api.image_name(image)?))).collect::<Option<Vec<_>>>());
    let mut classes = Vec::new();
    for (namespace, name, full) in specs {
        let mut found = Vec::new();
        let mut unique = images.is_some();
        if let Some(images) = &images {
            for image in ["com.chillyroom.service.core.Runtime.dll", "com.chillyroom.corekit.Runtime.dll"] {
                let candidates: Vec<_> = images.iter().filter(|(_, name)| name == image).collect();
                if candidates.len() > 1 { unique = false; continue; }
                let Some((owner, _)) = candidates.first().copied() else { continue; };
                let Some(class) = api.class_from_name(*owner, namespace, name) else { continue; };
                if api.class_type(class).and_then(|ty| api.type_name(ty)).as_deref() == Some(full) {
                    found.push((class, image));
                } else { unique = false; }
            }
        }
        let resolved = if unique && found.len() == 1 { found.pop() } else { None };
        unavailable |= resolved.is_none();
        emit(&mut budget, format!("WebViewMetadata class name={full} image={} status={}",
            resolved.map_or("unavailable", |(_, image)| image), if resolved.is_some() { "resolved" } else { "unavailable" }));
        classes.push(resolved);
    }
    'classes: for (index, resolved) in classes.into_iter().enumerate() {
        let Some((class, _)) = resolved else { continue; };
        let full = specs[index].2;
        if index < 2 {
            match api.fields(class) {
                Some(fields) => for field in fields {
                    let declaration = (|| {
                        let name = api.field_name(field)?;
                        let ty = api.type_name(api.field_type(field)?)?;
                        if name.len() > 128 || ty.len() > 512 { return None; }
                        Some(format!("WebViewMetadata field owner={full} name={name} type={ty} flags={} offset={}",
                            api.field_flags(field)?, api.field_offset(field)?))
                    })();
                    match declaration { Some(line) => emit(&mut budget, line), None => unavailable = true }
                    if budget.truncated { break 'classes; }
                },
                None => unavailable = true,
            }
        }
        if index == 1 { continue; }
        match api.methods(class) {
            Some(methods) => for method in methods {
                let Some(name) = api.method_name(method) else { unavailable = true; continue; };
                if !webview_method_selected(index, &name) { continue; }
                match webview_method_declaration(api, method) {
                    Some(declaration) => emit(&mut budget, format!("WebViewMetadata method owner={full} {declaration}")),
                    None => unavailable = true,
                }
                if budget.truncated { break 'classes; }
            },
            None => unavailable = true,
        }
    }
    let status = if budget.truncated { "truncated" } else if unavailable { "unavailable" } else { "complete" };
    host.log_line(TAG, &format!("WebViewMetadata observation end status={status}"));
}

fn class(api: &Il2cpp, image: &CStr, namespace: &CStr, name: &CStr) -> Result<Class, &'static str> {
    let image = api.exact_image(image.to_str().map_err(|_| "offline assembly name invalid")?)
        .ok_or("offline assembly identity mismatch")?;
    api.class_from_name(image, namespace, name).ok_or("offline class metadata unavailable")
}

fn target(api: &Il2cpp, spec: Target) -> Result<Method, &'static str> {
    target_in_image(api, c"Assembly-CSharp.dll", spec)
}

fn target_in_image(api: &Il2cpp, image: &CStr, spec: Target) -> Result<Method, &'static str> {
    let c = class(api, image, spec.namespace, spec.class)?;
    let checked = api.matching_checked_method(c, spec.method, spec.static_, spec.parameters, spec.returns)
        .ok_or("offline source signature mismatch")?;
    let method = checked.metadata();
    if !api.ordinary_method(method) || api.method_class(method) != Some(c)
        || api.method_entry(method) != api.base().checked_add(spec.rva) {
        return Err("offline source native identity mismatch");
    }
    code_guard(api, method, spec.rva, spec.prefix)?;
    Ok(method)
}

#[cfg(test)]
fn executable_source_range(maps: &str, address: usize, bytes: usize) -> bool {
    MappedRegions::contains_in(maps, address, bytes, true)
}

fn require_executable_source(regions: &MappedRegions, address: usize, bytes: usize) -> Result<(), &'static str> {
    if regions.contains(address, bytes, true) { Ok(()) } else { Err("Android ID source is not mapped RX") }
}

fn code_guard(api: &Il2cpp, method: Method, rva: usize, prefix: &str) -> Result<(), &'static str> {
    if api.method_entry(method) != api.base().checked_add(rva) { return Err("managed entry RVA mismatch"); }
    let length = prefix.len() / 2;
    if !matches!(length, 16 | 32) || prefix.len() != length * 2 { return Err("offline source guard length invalid"); }
    let mut bytes = [0; 32];
    MappedMemory::open().map_err(|_| "code guard reader unavailable")?
        .read_exact(api.base() + rva, &mut bytes[..length]).map_err(|_| "source code guard unreadable")?;
    if sha256::hex(&bytes[..length]) != prefix { return Err("offline source code prefix mismatch"); }
    Ok(())
}

fn source<T>(index: usize, operation: impl FnOnce(&OwnerScope<'_>) -> Result<T, &'static str>) -> Option<T> {
    if let Some(line) = trace().provider(index, 0) { trace_line(line); }
    let result = (|| {
        let runtime = RUNTIME.get().ok_or("offline runtime context unavailable")?;
        let owner = runtime.owner_scope()?;
        let _entered = owner.enter(index)?;
        operation(&owner)
    })();
    if let Some(line) = trace().provider(index, if result.is_ok() { 1 } else { 2 }) { trace_line(line); }
    finish_source(result)
}

fn finish_source<T>(result: Result<T, &'static str>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(reason) => { fail(reason); None }
    }
}

unsafe extern "C-unwind" fn configuration_hook(_: Ptr, _: Ptr) -> Ptr {
    // SAFETY: source checks the exact installed source's attached owner; all
    // managed objects are constructed/rooted through the checked runtime tools.
    source(0, |runtime| runtime.configuration()).unwrap_or(std::ptr::null_mut())
}

unsafe extern "C-unwind" fn manifest_hook(_: Ptr, _: Ptr) -> Ptr {
    // SAFETY: exact installed source on its attached owner; URI/account data is never read.
    source(1, |runtime| runtime.manifest()).unwrap_or(std::ptr::null_mut())
}

unsafe extern "C-unwind" fn time_hook(_: i32, _: Ptr) -> Ptr {
    // SAFETY: exact source hook; original NetTime cache/callback handling remains downstream.
    source(2, |runtime| runtime.time()).unwrap_or(std::ptr::null_mut())
}

unsafe extern "C-unwind" fn login_hook(_: Ptr, _: Ptr) {
    // SAFETY: original Login timing registered callbacks; source validates owner
    // and keeps every argument rooted. Errors never invoke the remote provider.
    source(3, |runtime| {
        if runtime.login_claimed.load(Ordering::Acquire) { Ok(()) } else { runtime.login() }
    });
}

unsafe extern "C-unwind" fn android_id_hook(_: Ptr) -> Ptr {
    // SAFETY: exact static String()/hidden MethodInfo ABI; source first checks
    // the existing IL2CPP owner. Errors never call the original Android bridge.
    source(5, |runtime| runtime.android_id()).unwrap_or(std::ptr::null_mut())
}

unsafe extern "C-unwind" fn update_hook(receiver: Ptr, method: Ptr) {
    if trace().enabled { trace().update_hits.fetch_add(1, Ordering::Relaxed); }
    let Some(original) = UPDATE_ORIGINAL.get() else { fail("original state Update unavailable"); return; };
    // SAFETY: exact instance Void()/hidden MethodInfo ABI, startup-published
    // typed trampoline. Both this hook and its original use C-unwind, as does
    // il2cpp_init_detour: a guest exception may unwind to its original caller
    // without crossing an extern C registry frame or a Rust catch_unwind.
    // The relocated first STP saves LR/x19; orig+4's original CFI describes
    // that 16-byte frame. No throwing call runs inside the trampoline itself.
    // Only a normal return reaches the extra scene operation below.
    unsafe { original(receiver, method) };
    if trace().enabled { trace().update_returns.fetch_add(1, Ordering::Relaxed); }
    let Some(runtime) = RUNTIME.get() else { return; };
    if FAILED.load(Ordering::Acquire) || runtime.stopped.load(Ordering::Acquire) || runtime.scene_once.claimed() { return; }
    if trace().enabled && runtime.api.thread_id() != Some(runtime.owner) { trace_stage("update owner-mismatch"); }
    // SAFETY: actual managed Update caller, no attachment/owner substitution.
    // source enforces the existing owner before reading any managed object.
    // receiver is compared as an address only: original Update may have
    // replaced/destroyed that state, so only a freshly rooted current is used.
    source(4, |runtime| runtime.scene(receiver));
}

/// Passive diagnostics from the actual managed caller, never the profile's
/// render callback. Preserve the complete original dispatch and its C-unwind
/// exception path; a throwing original cannot reach the observer below.
unsafe extern "C-unwind" fn manager_update_hook(receiver: Ptr, method: Ptr) {
    if trace().enabled { trace().manager_hits.fetch_add(1, Ordering::Relaxed); }
    let Some(original) = MANAGER_UPDATE_ORIGINAL.get() else { return; };
    // SAFETY: exact instance Void()/hidden MI ABI and pre-game-published
    // trampoline. The first STR/STP/ADRP/LDRB contain no throwing call; tail
    // orig+4 or +16 has the original SP+32 / x19,x20,LR CFI. Guest exceptions
    // unwind through the same C-unwind boundary as the existing state hook.
    unsafe { original(receiver, method) };
    if trace().enabled { trace().manager_returns.fetch_add(1, Ordering::Relaxed); }
    if !trace().wants_manager() { return; }
    let observation = match RUNTIME.get() {
        None => ManagerObservation::Stage("manager-update runtime-unavailable"),
        Some(runtime) if FAILED.load(Ordering::Acquire) || runtime.stopped.load(Ordering::Acquire)
            || !runtime.installed.load(Ordering::Acquire) => ManagerObservation::Stage("manager-update runtime-inactive"),
        Some(runtime) if runtime.api.thread_id() != Some(runtime.owner) => ManagerObservation::Stage("manager-update owner-mismatch"),
        // SAFETY: actual original owner, live runtime and only fresh cache
        // roots below. The old receiver is compared as an address only.
        Some(runtime) => runtime.owner_scope().and_then(|owner| owner.observe_manager(receiver))
            .unwrap_or(ManagerObservation::Stage("manager-update observation-unavailable")),
    };
    if let Some(line) = trace().observe_manager(observation) { trace_line(&line); }
}

#[path = "offline_sources.rs"]
mod sources;

#[cfg(test)]
#[path = "../tests/unit/offline.rs"]
mod tests;
