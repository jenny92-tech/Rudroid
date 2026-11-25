//! INPUT: a trusted prepared ARM64 Linux Mono bridge, JIT and P/Invoke map
//! OUTPUT: process-pinned native alias and scoped bridge callbacks
//! POS: owns the native bridge alongside guest libdl without publishing a guest IL2CPP image
//!
//! The ua_mono_setup layout follows PortMaster-Unity's runtime/mono64/il2cpp_mono.h.
//! See THIRD-PARTY-NOTICES for its MIT notice and source revision.
#![deny(unsafe_op_in_unsafe_fn)]

use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
use rudroid_common::{
    elf_identity::{aarch64_dyn_abi_class, aarch64_dyn_identity, ElfAbiClass},
    handles::HandleTable,
    host_api::HostApiV0,
    native_alias::{GuestLibdl, NativeAlias},
    registry::HostStr,
};
use rudroid_unity_common::mono_abi::MonoStartup;
use std::{
    collections::HashMap,
    ffi::{c_int, c_void, CStr, CString},
    fs::File,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

rudroid_common::declare_descriptor!(
    "unity_mono",
    rudroid_common::Kind::Runtime,
    package = None::<&str>,
    requires = Some("android_elf_load")
);

mod pacing;
mod steam;
mod service;

use service::MONO_SERVICE;

/// Reject kits that also deploy `il2cpp_patch` and publish the Mono service.
#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.has_capability(c"il2cpp_patch") {
        api.log_error(
            c"MONO",
            "unity_mono cannot load alongside il2cpp_patch (Mono never maps libil2cpp)",
        );
        return -1;
    }
    if !api.register_service_typed(&MONO_SERVICE) {
        api.log_error(c"MONO", "failed to register rudroid.unity.mono.v1");
        return -1;
    }
    0
}

#[cfg(all(test, target_arch = "aarch64", target_env = "gnu"))]
#[path = "../tests/unit/lib.rs"]
mod tests;

const ALIAS: &CStr = c"libil2cpp.so";
const RELATIVE_ALIAS: &CStr = c"lib/arm64-v8a/libil2cpp.so";

#[repr(C)]
struct BridgeHost<'a> {
    size: c_int,
    dlopen: extern "C" fn(HostStr<'_>, c_int) -> *mut c_void,
    dlsym: extern "C" fn(*mut c_void, HostStr<'_>) -> *mut c_void,
    libdir: HostStr<'a>,
    unity_version: HostStr<'a>,
    managed_dir: HostStr<'a>,
    gc_time_slice_ms: c_int,
    // This is the upstream size-versioned prefix, before read_asset. It loads
    // assemblies from the supplied directory without the optional APK filter.
}

struct State {
    bridge: &'static Library,
    _runtime: &'static Library,
    guest: &'static GuestLibdl,
    _version: CString,
    managed: CString,
    libdir: CString,
    plugins: HashMap<String, Plugin>,
}

enum Plugin {
    Missing,
    OfflineSteam,
    Guest(CString),
    Native(&'static Library),
}

#[derive(PartialEq, Eq)]
enum Rule<'a> {
    Missing,
    OfflineSteam,
    Library(&'a str),
    GuestLibrary(&'a str),
    NativeLibrary(&'a str),
}

static STATE: OnceLock<State> = OnceLock::new();
static START: OnceLock<()> = OnceLock::new();
static PINVOKES: HandleTable<PInvoke> = HandleTable::new();
static PINVOKE_CACHE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

enum PInvoke {
    Guest(usize),
    Native(&'static Library),
    Steam,
}

extern "C" fn bridge_symbol(name: HostStr<'_>) -> usize {
    let Some(name) = name
        .to_cstr()
        .filter(|name| name.to_bytes().starts_with(b"il2cpp_"))
    else {
        return 0;
    };
    if name == c"il2cpp_add_internal_call" {
        if let Some(wrapper) = pacing::registration_wrapper() {
            return wrapper;
        }
    }
    STATE
        .get()
        .and_then(|state| library_symbol(state.bridge, name))
        .unwrap_or(0)
}

fn load_library(path: &Path) -> Result<&'static Library, String> {
    File::open(path)
        .and_then(|file| aarch64_dyn_identity(&file))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: the operator selects these trusted immutable ARM64 glibc
    // shared objects. Native constructors and code references stay process-pinned.
    let library = unsafe { Library::open(Some(path), RTLD_NOW | RTLD_LOCAL) }
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Box::leak(Box::new(library)))
}

fn library_symbol(library: &Library, name: &CStr) -> Option<usize> {
    // SAFETY: library is pinned, and callers invoke only a resolved ABI
    // they know. Copy the address rather than lending a symbol handle.
    let address = unsafe { library.get::<*mut c_void>(name.to_bytes_with_nul()) }.ok()?;
    (!address.is_null()).then_some(*address as usize)
}

fn strip_known_prefix<'a>(name: &'a [u8], libdir: &[u8], managed: &[u8]) -> &'a [u8] {
    let mut rest = name
        .strip_prefix(libdir)
        .and_then(|tail| tail.strip_prefix(b"/"))
        .or_else(|| {
            name.strip_prefix(managed)
                .and_then(|tail| tail.strip_prefix(b"/"))
        })
        .unwrap_or(name);
    while let Some(tail) = rest.strip_prefix(b"./") {
        rest = tail;
    }
    for prefix in [
        &b"lib/arm64-v8a/"[..],
        &b"Plugins/arm64-v8a/"[..],
        &b"Plugins/Android/"[..],
        &b"Plugins/"[..],
    ] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            rest = tail;
            break;
        }
    }
    rest
}

extern "C" fn open(name: HostStr<'_>, flags: c_int) -> *mut c_void {
    let Some(name) = name.to_cstr() else {
        return std::ptr::null_mut();
    };
    let Some(state) = STATE.get() else {
        return std::ptr::null_mut();
    };
    let basename = strip_known_prefix(
        name.to_bytes(),
        state.libdir.to_bytes(),
        state.managed.to_bytes(),
    );
    let key = std::str::from_utf8(basename).ok().and_then(normalize);
    let cache_key = key
        .map(|k| format!("key:{k}"))
        .unwrap_or_else(|| format!("raw:{}", name.to_string_lossy()));
    let cache = PINVOKE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(&cached) = cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&cache_key)
    {
        return cached as *mut c_void;
    }
    let mapped = key.and_then(|key| state.plugins.get(key));
    let open_guest = |name: &CStr| {
        let handle = (state.guest.open)(HostStr::new(name), flags);
        (!handle.is_null()).then_some(PInvoke::Guest(handle as usize))
    };
    let handle = match mapped {
        Some(Plugin::Guest(path)) => {
            let handle = open_guest(path);
            if handle.is_none() {
                rudroid_common::error!(
                    "[MONO] mapped P/Invoke guest library {path:?} was not loaded"
                );
            }
            handle
        }
        Some(Plugin::Native(module)) => Some(PInvoke::Native(module)),
        Some(Plugin::OfflineSteam) => {
            rudroid_common::diagnostic!("[MONO] P/Invoke {name:?}: offline Steam (no client)");
            Some(PInvoke::Steam)
        }
        Some(Plugin::Missing) => None,
        None => open_guest(name),
    };
    let Some(handle) = handle else {
        return std::ptr::null_mut();
    };
    let inserted = PINVOKES.insert(handle);
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    let entry = *guard.entry(cache_key).or_insert(inserted as usize);
    entry as *mut c_void
}

extern "C" fn lookup(handle: *mut c_void, name: HostStr<'_>) -> *mut c_void {
    let Some(call) = PINVOKES.get(handle) else {
        return std::ptr::null_mut();
    };
    match *call {
        PInvoke::Guest(handle) => {
            let Some(state) = STATE.get() else {
                return std::ptr::null_mut();
            };
            (state.guest.lookup)(handle as *mut c_void, name)
        }
        PInvoke::Native(module) => name
            .to_cstr()
            .and_then(|name| library_symbol(module, name))
            .unwrap_or(0) as *mut c_void,
        PInvoke::Steam => name.to_cstr().map(steam::symbol).unwrap_or(0) as *mut c_void,
    }
}

/// # Safety
/// Arguments obey the same-kit HostStr contract. The explicitly selected
/// glibc objects and their immutable dependencies are trusted to run native
/// constructors and the PortMaster-Unity bridge ABI.
#[no_mangle]
unsafe extern "C" fn rudroid_unity_mono_start(
    config: &MonoStartup<'_>,
    guest: &'static GuestLibdl,
    host: Option<&'static HostApiV0>,
) -> c_int {
    match start(config, guest, host) {
        Ok(()) => 0,
        Err(reason) => {
            rudroid_common::error!("[MONO] {reason}");
            -1
        }
    }
}

pub(crate) fn start(
    config: &MonoStartup<'_>,
    guest: &'static GuestLibdl,
    host: Option<&'static HostApiV0>,
) -> Result<(), String> {
    if !cfg!(all(target_arch = "aarch64", target_env = "gnu")) {
        return Err("Mono startup requires ARM64 glibc".into());
    }
    START
        .set(())
        .map_err(|_| "Mono startup has already been attempted")?;
    for name in ["UA_MONO64_RUNTIME", "UA_MONO64_MANAGED"] {
        if std::env::var_os(name).is_some_and(|value| !value.is_empty()) {
            return Err(format!(
                "{name} overrides the configured Mono artifact paths"
            ));
        }
    }
    let bridge = path(config.bridge)?;
    let runtime = path(config.runtime)?;
    let libdir = path(config.guest_lib_dir)?;
    let managed_path = path(config.managed_dir)?;
    let native_dir = bridge.parent().ok_or("missing bridge parent directory")?;
    if bridge.file_name().and_then(|name| name.to_str()) != Some("libil2cpp.so")
        || runtime != native_dir.join("libmonobdwgc-2.0.so")
    {
        return Err("invalid bridge/runtime sibling paths".into());
    }
    for file in ["libMonoPosixHelper.so", "libmono-native.so"] {
        if let Some(helper) = contained_file(native_dir, file)? {
            File::open(&helper)
                .and_then(|f| aarch64_dyn_identity(&f))
                .map_err(|e| format!("{}: {e}", helper.display()))?;
        }
    }
    let plugins = match config.plugins_map.to_cstr() {
        Some(_) => load_plugins(&path(config.plugins_map)?, &libdir, native_dir)?,
        None => HashMap::new(),
    };
    let version = config
        .unity_version
        .to_cstr()
        .ok_or("missing Unity version")?
        .to_owned();
    let managed = c_path(&managed_path)?;
    let absolute_alias = c_path(&libdir.join("libil2cpp.so"))?;
    let libdir = c_path(&libdir)?;
    // Check the JIT before the bridge constructor, which aborts if its
    // sibling cannot be loaded or lacks these mandatory exports.
    let native_runtime = load_library(&runtime)?;
    for name in [
        c"mono_jit_init_version",
        c"mono_runtime_invoke",
        c"mono_add_internal_call",
        c"mono_gc_collect",
        c"mono_gc_get_used_size",
        c"mono_gc_get_heap_size",
    ] {
        if library_symbol(native_runtime, name).is_none() {
            return Err(format!("{} is missing {name:?}", runtime.display()));
        }
    }
    let native_bridge = load_library(&bridge)?;
    for name in [c"il2cpp_init", c"il2cpp_runtime_invoke"] {
        if library_symbol(native_bridge, name).is_none() {
            return Err(format!("{} is missing {name:?}", bridge.display()));
        }
    }
    let setup = library_symbol(native_bridge, c"ua_mono_setup")
        .ok_or_else(|| format!("{} is missing ua_mono_setup", bridge.display()))?;
    STATE
        .set(State {
            bridge: native_bridge,
            guest,
            _runtime: native_runtime,
            _version: version,
            managed,
            libdir,
            plugins,
        })
        .map_err(|_| "Mono state was already published")?;
    let state = STATE.get().ok_or("Mono state publication lost")?;
    let bridge_host = BridgeHost {
        size: std::mem::size_of::<BridgeHost<'_>>() as c_int,
        dlopen: open,
        dlsym: lookup,
        libdir: HostStr::new(&state.libdir),
        unity_version: HostStr::new(&state._version),
        managed_dir: HostStr::new(&state.managed),
        gc_time_slice_ms: 0,
    };
    // SAFETY: the pinned, explicitly trusted bridge exports the documented
    // setup ABI. Borrowed C strings live inside process-pinned STATE.
    let code = unsafe {
        let setup: extern "C" fn(&BridgeHost<'_>) -> c_int = std::mem::transmute(setup);
        setup(&bridge_host)
    };
    if code != 0 {
        return Err(format!(
            "{} rejected ua_mono_setup ({code})",
            bridge.display()
        ));
    }
    pacing::install(config, native_bridge, host)?;
    // Unity resolves a player-side symlink before it asks guest libdl to
    // open the bridge. Accept its exact native pathname and guest aliases.
    let bridge_path = c_path(&bridge)?;
    let alias = NativeAlias {
        names: [ALIAS, RELATIVE_ALIAS, &absolute_alias, &bridge_path].map(HostStr::new),
        symbol: bridge_symbol,
    };
    if (guest.register_alias)(&alias) != 0 {
        return Err("Android ELF loader rejected the native bridge alias".into());
    }
    rudroid_common::diagnostic!(
        "[MONO] registered {} using {} (Unity {})",
        bridge.display(),
        runtime.display(),
        config.unity_version.to_cstr().unwrap().to_string_lossy()
    );
    Ok(())
}

fn path(field: HostStr<'_>) -> Result<PathBuf, String> {
    let text = field
        .to_cstr()
        .and_then(|text| text.to_str().ok())
        .ok_or("invalid Mono path")?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(format!("{text} is not an absolute Mono path"));
    }
    Ok(path)
}

fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())
}

fn contained_file(dir: &Path, basename: &str) -> Result<Option<PathBuf>, String> {
    let path = dir.join(basename);
    match path.symlink_metadata() {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        result => {
            result.map_err(|e| format!("{}: {e}", path.display()))?;
            let resolved = path
                .canonicalize()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let canonical_dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
            if !resolved.starts_with(&canonical_dir) || !resolved.is_file() {
                return Err(format!(
                    "{} is not a regular file within {}",
                    path.display(),
                    dir.display()
                ));
            }
            // Preserve the declared basename for guest libdl and upstream dllmaps.
            Ok(Some(path))
        }
    }
}

fn library_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && !matches!(name, "." | "..")
}

fn normalize(name: &str) -> Option<&str> {
    if !library_name(name) {
        return None;
    }
    let name = name.strip_prefix("lib").unwrap_or(name);
    let name = name
        .strip_suffix(".so")
        .or_else(|| name.strip_suffix(".dll"))
        .unwrap_or(name);
    (!name.is_empty()).then_some(name)
}

fn valid_so_target(target: &str) -> bool {
    library_name(target) && (target.ends_with(".so") || target.contains(".so."))
}

fn load_plugins(
    path: &Path,
    guest: &Path,
    native: &Path,
) -> Result<HashMap<String, Plugin>, String> {
    const LIMIT: u64 = 64 * 1024;
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(LIMIT + 1).read_to_string(&mut text))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() as u64 > LIMIT {
        return Err("plugins.map exceeds its size limit".into());
    }
    let mut rules = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let code = line.split('#').next().unwrap_or_default();
        let words: Vec<_> = code.split_whitespace().collect();
        let (name, rule) = match words.as_slice() {
            [] => continue,
            ["map", name, "none"] => (*name, Rule::Missing),
            ["map", name, "stub", "steam"] => (*name, Rule::OfflineSteam),
            ["map", name, "lib", target] if valid_so_target(target) => {
                (*name, Rule::Library(target))
            }
            ["map", name, "guest", target] if valid_so_target(target) => {
                (*name, Rule::GuestLibrary(target))
            }
            ["map", name, "native", target] if valid_so_target(target) => {
                (*name, Rule::NativeLibrary(target))
            }
            _ => {
                return Err(format!(
                    "{}:{}: unsupported or malformed P/Invoke mapping",
                    path.display(),
                    index + 1
                ))
            }
        };
        let key = normalize(name)
            .ok_or_else(|| format!("{}:{}: invalid P/Invoke name", path.display(), index + 1))?;
        match rules.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(rule);
            }
            std::collections::hash_map::Entry::Occupied(entry) if *entry.get() == rule => {}
            _ => {
                return Err(format!(
                    "{}:{}: conflicting P/Invoke aliases for {name}",
                    path.display(),
                    index + 1
                ))
            }
        }
    }
    let same_dir = guest
        .canonicalize()
        .ok()
        .zip(native.canonicalize().ok())
        .is_some_and(|(g, n)| g == n);
    let mut loaded_native: HashMap<PathBuf, &'static Library> = HashMap::new();
    let mut load_native_once = |target_path: &Path| -> Result<&'static Library, String> {
        let key = target_path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", target_path.display()))?;
        if let Some(&lib) = loaded_native.get(&key) {
            return Ok(lib);
        }
        let lib = load_library(target_path)?;
        loaded_native.insert(key, lib);
        Ok(lib)
    };
    rules.into_iter().map(|(name, rule)| {
        let plugin = match rule {
            Rule::Missing => Plugin::Missing,
            Rule::OfflineSteam => Plugin::OfflineSteam,
            Rule::GuestLibrary(target) => {
                let Some(path) = contained_file(guest, target)? else {
                    return Err(format!("P/Invoke {name}: {target} not found in guest library directory"));
                };
                Plugin::Guest(c_path(&path)?)
            }
            Rule::NativeLibrary(target) => {
                if matches!(target, "libil2cpp.so" | "libmonobdwgc-2.0.so") {
                    return Err(format!("P/Invoke {name}: {target} cannot be mapped as a native plugin"));
                }
                let Some(path) = contained_file(native, target)? else {
                    return Err(format!("P/Invoke {name}: {target} not found in native library directory"));
                };
                Plugin::Native(load_native_once(&path)?)
            }
            Rule::Library(target) if same_dir => {
                if matches!(target, "libil2cpp.so" | "libmonobdwgc-2.0.so") {
                    return Err(format!("P/Invoke {name}: {target} cannot be mapped as a plugin"));
                }
                let Some(path) = contained_file(guest, target)? else {
                    return Err(format!("P/Invoke {name}: {target} not found in shared library directory"));
                };
                let abi = File::open(&path)
                    .and_then(|file| aarch64_dyn_abi_class(&file))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                match abi {
                    ElfAbiClass::AndroidBionic => Plugin::Guest(c_path(&path)?),
                    ElfAbiClass::LinuxGnu => Plugin::Native(load_native_once(&path)?),
                    ElfAbiClass::Ambiguous => {
                        return Err(format!(
                            "P/Invoke {name}: {target} shares one directory with ambiguous ELF ABI; declare `map {name} guest {target}` or `map {name} native {target}`"
                        ));
                    }
                }
            }
            Rule::Library(target) => match (contained_file(guest, target)?, contained_file(native, target)?) {
                (Some(path), None) => Plugin::Guest(c_path(&path)?),
                (None, Some(path)) if target != "libil2cpp.so" && target != "libmonobdwgc-2.0.so" => {
                    Plugin::Native(load_native_once(&path)?)
                }
                _ => return Err(format!("P/Invoke {name}: {target} must name one supplied plugin in the guest or native directory")),
            },
        };
        Ok((name.to_owned(), plugin))
    }).collect()
}
