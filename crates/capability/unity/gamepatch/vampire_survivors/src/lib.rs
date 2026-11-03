//! INPUT:  package-gated IL2CPP events, shared exact-build ABI table and Unity Shader metadata
//! OUTPUT:  version-validated fallback for a missing Hidden/Internal-Colored debug shader
//! POS:    title plugin; original APK, metadata and successful shader lookups stay intact
#![deny(unsafe_op_in_unsafe_fn)]

use rudroid_common::{
    arm64_detour::Original,
    host_api::{HostApiV0, HOST_API_VERSION},
    il2cpp::{GcHandles, Il2cpp, Inventory},
    registry::RudroidModuleEvent,
    ModuleKind,
};
use std::{
    ffi::{c_void, CStr, CString},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        OnceLock,
    },
};

rudroid_common::declare_descriptor!(
    "vampire_survivors",
    rudroid_common::Kind::GamePatch,
    package = Some("com.poncle.vampiresurvivors"),
    requires = Some("unity_6,il2cpp_patch")
);

static HOST: OnceLock<HostApiV0> = OnceLock::new();
static MODULE: AtomicUsize = AtomicUsize::new(0);
static ATTEMPTED: AtomicBool = AtomicBool::new(false);
static ORIGINAL: Original<FindShader> = Original::new();
static STRINGS: OnceLock<(Il2cpp, GcHandles)> = OnceLock::new();
const MISSING: &str = "Hidden/Internal-Colored";
type FindShader = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> *mut c_void;

/// Every export `install` checks before touching metadata.
const REQUIRED_EXPORTS: &[&str] = &[
    "il2cpp_domain_get",
    "il2cpp_domain_get_assemblies",
    "il2cpp_assembly_get_image",
    "il2cpp_image_get_name",
    "il2cpp_class_from_name",
    "il2cpp_class_get_method_from_name",
    "il2cpp_class_get_field_from_name",
    "il2cpp_method_get_flags",
    "il2cpp_method_get_param_count",
    "il2cpp_method_get_param",
    "il2cpp_type_get_type",
    "il2cpp_method_get_return_type",
    "il2cpp_class_from_type",
    "il2cpp_string_length",
    "il2cpp_string_chars",
    "il2cpp_string_new",
    "il2cpp_runtime_invoke",
    "il2cpp_thread_current",
    "il2cpp_gchandle_new",
    "il2cpp_gchandle_get_target",
    "il2cpp_gchandle_free",
];

fn log(message: &CStr) {
    if let Some(api) = HOST.get() {
        api.log_line(c"VS_GAMEPATCH", &message.to_string_lossy());
    }
}

/// Borrow only guest-managed strings; paired IL2CPP GC handles protect the
/// temporary fallback across allocation/lookup. No guest object becomes a
/// Rust owner. C-unwind lets a guest C++ exception cross the hook legally and
/// the `GcRoot` guard releases the temporary handle during unwinding.
///
/// # Safety
/// `original` is the trampoline of the managed `Shader.Find(string)` and
/// `name`/`method` are the arguments the guest passed to the hook.
unsafe fn lookup(
    api: Il2cpp,
    handles: GcHandles,
    original: FindShader,
    name: *mut c_void,
    method: *mut c_void,
) -> *mut c_void {
    // SAFETY: caller contract — the guest's own call, forwarded unchanged.
    let found = unsafe { original(name, method) };
    // SAFETY: `name` is the guest's null or live managed string argument.
    if !found.is_null() || !unsafe { api.string_eq(name, MISSING) } {
        return found;
    }
    let fallback = api.string_new(c"Sprites/Default");
    if fallback.is_null() {
        return found;
    }
    // SAFETY: the string just allocated is live on this hook's stack.
    let Some(root) = (unsafe { handles.root(fallback) }) else {
        return found;
    };
    let fallback = root.target();
    if fallback.is_null() {
        return found;
    }
    // SAFETY: the rooted string replaces the guest's name argument.
    let result = unsafe { original(fallback, method) };
    if !result.is_null() {
        log(c"missing debug shader resolved with Sprites/Default; original successful lookups preserved");
    }
    result
}
unsafe extern "C-unwind" fn shader_find(name: *mut c_void, method: *mut c_void) -> *mut c_void {
    match (STRINGS.get(), ORIGINAL.get()) {
        // SAFETY: `original` is the installed detour's trampoline of the
        // validated Shader.Find entry; the guest's arguments pass through.
        (Some(&(api, handles)), Some(original)) => unsafe { lookup(api, handles, original, name, method) },
        _ => std::ptr::null_mut(),
    }
}

fn enabled(api: &HostApiV0) -> bool {
    // An absent key reads as "", as the getter's own fallback copy did.
    let package = api.config_string(c"package.packageName", 256).unwrap_or_default();
    let package = CString::new(package).unwrap_or_default();
    applies(
        &package,
        api.config_bool(c"game_patches.vampire_survivors.missing_debug_shader", false),
    )
}
fn applies(package: &CStr, configured: bool) -> bool {
    configured && package == c"com.poncle.vampiresurvivors"
}

extern "C" fn mapped(event: Option<&RudroidModuleEvent>, _user: *mut c_void) {
    if let Some(event) = event {
        if event.kind == ModuleKind::Il2cpp as u32 {
            MODULE.store(event.base, Ordering::Release);
        }
    }
}
extern "C" fn ready(_event: Option<&RudroidModuleEvent>, _user: *mut c_void) {
    if ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(api) = HOST.get() else {
        return;
    };
    let module = MODULE.load(Ordering::Acquire);
    if module == 0 {
        log(c"IL2CPP module missing; debug shader patch not installed");
        return;
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    // SAFETY: `module` is the base the Il2cpp event reported for the mapped,
    // never-unmapped guest libil2cpp; Il2cppReady precedes managed startup,
    // so Shader.Find is quiescent.
    if let Err(reason) = unsafe { install(api, module) } {
        log(reason);
    }
    #[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
    log(c"debug shader detour requires Linux ARM64");
}

/// Il2cppReady precedes managed game startup.
///
/// # Safety
/// `module` is the load base of the live IL2CPP module, never unmapped. The
/// caller must ensure the managed Shader.Find entry is quiescent during
/// install (Unity's native render workers use a separate engine entry), and
/// pin both modules for process life, as with the existing title detours.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
unsafe fn install(host: &HostApiV0, module: usize) -> Result<(), &'static CStr> {
    // SAFETY: caller contract.
    let il2cpp = unsafe { Il2cpp::bind(module, host.so_symbol, REQUIRED_EXPORTS) }
        .map_err(|_| c"required IL2CPP metadata/string exports missing; patch not installed")?;
    let images = il2cpp.inventory().map_err(|error| match error {
        Inventory::NoDomain => c"IL2CPP domain unavailable; patch not installed",
        Inventory::InvalidAssemblies => c"IL2CPP assembly inventory invalid; patch not installed",
    })?;
    let (mut shader, mut debug, mut application) = (None, None, None);
    for image in images {
        let Some(name) = il2cpp.image_name(image) else {
            continue;
        };
        match name.as_str() {
            "UnityEngine.CoreModule.dll" => {
                shader = il2cpp.class_from_name(image, c"UnityEngine", c"Shader");
                application = il2cpp.class_from_name(image, c"UnityEngine", c"Application");
            }
            "PhaserPort.dll" => debug = il2cpp.class_from_name(image, c"", c"VSDebug"),
            _ => (),
        }
    }
    let (Some(shader), Some(debug)) = (shader, debug) else {
        return Err(c"Vampire Survivors shader/debug metadata mismatch; patch not installed");
    };
    if il2cpp.field(debug, c"_debugMat").is_none() || il2cpp.field(debug, c"s_drawDebug").is_none() {
        return Err(c"Vampire Survivors shader/debug metadata mismatch; patch not installed");
    }
    let Some(target) = il2cpp.method(shader, c"Find", 1) else {
        return Err(c"Shader.Find(string) metadata missing; patch not installed");
    };
    let version = application
        .ok_or("Application metadata unavailable")
        .and_then(|application| il2cpp.unity_version(application))
        .map_err(|_| c"Unity version query unavailable; patch not installed")?;
    let abi = match rudroid_common::il2cpp_abi::require_gc_handle_abi(&version) {
        Ok(abi @ rudroid_common::il2cpp_abi::GcHandleAbi::PointerWidth) => abi,
        result => {
            let reason = result.err().unwrap_or_else(|| {
                format!("shader patch requires pointer-width GC handles: {version}")
            });
            host.log_line(c"VS_GAMEPATCH", &reason);
            return Err(
                c"Unity build GC-handle ABI unverified for this patch; original entry retained",
            );
        }
    };
    host.log_line(
        c"VS_GAMEPATCH",
        &format!("Unity build GC-handle ABI verified: {version}; pointer-width adapter"),
    );
    if !il2cpp.method_shape(target, 1, true) {
        return Err(c"Shader.Find native ABI mismatch; patch not installed");
    }
    let parameter = il2cpp.method_param(target, 0);
    let return_type = il2cpp.method_return_type(target);
    let (Some(parameter), Some(return_type)) = (parameter, return_type) else {
        return Err(c"Shader.Find native ABI mismatch; patch not installed");
    };
    if il2cpp.type_kind(parameter) != Some(rudroid_common::il2cpp::TYPE_STRING)
        || il2cpp.class_from_type(return_type) != Some(shader)
    {
        return Err(c"Shader.Find native ABI mismatch; patch not installed");
    }
    let Some(entry) = il2cpp.method_entry(target) else {
        return Err(c"Shader.Find entry missing; patch not installed");
    };
    // SAFETY: this exact build/target has verified pointer-width exports.
    let handles = unsafe { il2cpp.gc_handles(abi) }
        .ok_or(c"required IL2CPP metadata/string exports missing; patch not installed")?;
    STRINGS
        .set((il2cpp, handles))
        .map_err(|_| c"debug shader context already installed")?;
    // SAFETY: caller contract — a validated, quiescent managed entry.
    unsafe { ORIGINAL.install_quiescent(entry, shader_find) }
    .map_err(|reason| {
        host.log_line(c"VS_GAMEPATCH", &reason.to_string());
        c"Shader.Find detour failed; original entry retained"
    })?;
    log(c"optional missing debug shader fallback installed via metadata; original assets retained");
    Ok(())
}

/// Host callbacks and guest mappings remain live for the whole process.
#[no_mangle]
/// # Safety
/// `api` is the process-pinned same-kit host table; symbol lookup and emitted
/// module events identify live, correctly typed guest exports.
pub unsafe extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> i32 {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    if !enabled(api) {
        return 0;
    }
    HOST.get_or_init(|| *api);
    if !api.on_module_loaded(ModuleKind::Il2cpp, mapped, std::ptr::null_mut())
        || !api.on_module_loaded(ModuleKind::Il2cppReady, ready, std::ptr::null_mut())
    {
        return -3;
    }
    log(c"optional missing debug shader fallback armed for Vampire Survivors");
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
