//! INPUT:  shared JNI VM, HostApi module events and the typed SetOutput original
//! OUTPUT:  FMOD addon registration, audio bridge and SetOutput fallback hook
//! POS:    FMOD guest ABI boundary; native installation and original publication share one typed operation
//! rudroid-android-addon-fmod: FMOD Android Addon (ticket 19; real bridge
//! behavior on the synthetic ABI probe ticket 56).
//!
//! Rust counterpart of the read-only `sdk_fmod` plugin. Ticket 56 implements
//! the reference chain against the kit's module-event surface:
//!
//! 1. `rudroid_plugin_register_jni` — joins the SHARED JNI session (ticket
//!    52) and registers the `org/fmod` Java stub set the oracle registers
//!    (`FMOD` / `AudioDevice` / `FMODAudioDevice`, fakefmod.h): the classes
//!    resolve, `FMOD.checkInit ()Z` answers `true` through the catalog-backed
//!    resolution, and a global anchor marks the registration session-wide.
//!    This runs regardless of guest files — the oracle's stub posture.
//! 2. `rudroid_plugin_init` — subscribes to the Unity module event (the
//!    reference on_unity_loaded trigger: FMOD is mapped only after Unity so
//!    FMOD variants importing Unity symbols can relocate), plus the `Fmod`
//!    and `FmodStudio` module events (the kit loads the libraries through
//!    the `android_elf_load` ELF provider and fires the events).
//! 3. On the `Fmod` event the addon resolves the oracle's SetOutput symbol
//!    list (`FMOD5_System_SetOutput`, `FMOD_System_SetOutput`, the C++
//!    mangled `setOutput`) through `so_symbol` and installs a REAL ticket-51
//!    ARM64 inline detour (the shared `rudroid_common::arm64_detour`;
//!    capability crates never statically link each other and the Host API
//!    has no hook slot). The
//!    hook body is the oracle `set_output_hook`: the AUDIOTRACK → NOSOUND →
//!    OPENSL → AUTODETECT → requested try-order against the callable
//!    trampoline, the reentrancy depth guard, and the forced-NOSOUND OK
//!    fallback with the oracle log lines.
//!
//! Missing guest libraries keep the explicit `StubbedMissingGuest` state
//! (log + the Java stub set still registered); a present-but-broken file
//! fails the loader bring-up through the ELF provider's diagnostics. No SDL
//! code lives in this crate — audio policy belongs to the Linux host side.
//!
//! Boundary: the real `libfmod.so`/`libfmodstudio.so` are licensed vendor
//! libraries and NOT experiment dependencies; every dynamic claim here is
//! proven against the self-compiled ABI probe
//! (`tests/fixtures/rudroid56_fmod_probe.c`), and the ticket-41 real-FMOD file stage
//! stays conditional.

#![deny(unsafe_op_in_unsafe_fn)]

mod audio;
use std::ffi::{c_int, c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use rudroid_common::handles::HandleTable;
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::{JavaVm, JniEnv, JNI_VERSION_1_6};
use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::ModuleKind;

// The verified AArch64 inline detour shared through common (capability
// crates never statically link each other). Installs are process-lifetime.
use rudroid_common::arm64_detour::Original;

rudroid_common::declare_descriptor!(
    "sdk_fmod",
    rudroid_common::Kind::AndroidAddon,
    package = None::<&str>,
    requires = Some("android_base")
);

/// Guest libraries the C++ plugin maps at runtime. Deploy Selection must
/// place them under the game's `lib/arm64-v8a/` for real audio; without them
/// the addon keeps the documented stub result instead of crashing.
pub const GUEST_LIBRARIES: [&str; 2] = ["lib/arm64-v8a/libfmod.so", "lib/arm64-v8a/libfmodstudio.so"];

/// Log tag (oracle "FMOD").
pub const LOG_TAG: &str = "FMOD";

/// `FMOD_ERR_PLUGIN_MISSING`-ish value the C++ stub returns when the guest
/// FMOD runtime is absent (oracle sdk_fmod.cpp).
pub const STUB_ERR_PLUGIN_MISSING: i64 = 18;

/// FMOD output types (oracle enum in sdk_fmod.cpp).
pub const OUT_AUTODETECT: i32 = 0;
pub const OUT_NOSOUND: i32 = 2;
pub const OUT_AUDIOTRACK: i32 = 15;
pub const OUT_OPENSL: i32 = 16;

/// The oracle symbol candidates for SetOutput, in probe order.
pub const SET_OUTPUT_SYMBOLS: [&str; 3] = [
    "FMOD5_System_SetOutput",
    "FMOD_System_SetOutput",
    "_ZN4FMOD6System9setOutputE15FMOD_OUTPUTTYPE",
];

/// The observed bridge state (`None` before the addon armed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmodOutcome {
    /// Waiting for the Unity module event.
    Pending,
    /// No guest FMOD library: the explicit stub state (org/fmod registered).
    StubbedMissingGuest,
    /// The Fmod module event fired with a real provider load and the
    /// SetOutput hook installed (the trampoline is live).
    Bridged,
    /// The module event fired but no SetOutput symbol resolved — the oracle
    /// "WARNING: no SetOutput hook installed" posture (the original runs
    /// un-hooked; named, never a silent pass).
    HookFailed { reason: &'static str },
}

static OUTCOME: Mutex<Option<FmodOutcome>> = Mutex::new(None);
/// Idempotency per role (oracle g_fmod_install_attempted / g_fmod_load_attempted).
static FMOD_INSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static UNITY_SEEN: AtomicBool = AtomicBool::new(false);

fn set_outcome(outcome: FmodOutcome) {
    *OUTCOME.lock().unwrap() = Some(outcome);
}

/// The outcome of the last guest-library chain (`None` before it ran).
pub fn last_outcome() -> Option<FmodOutcome> {
    OUTCOME.lock().unwrap().clone()
}

#[doc(hidden)]
pub fn reset_state_for_tests() {
    UNITY_SEEN.store(false, Ordering::SeqCst);
    FMOD_INSTALL_ATTEMPTED.store(false, Ordering::SeqCst);
    *OUTCOME.lock().unwrap() = None;
    SET_OUTPUT_ORIG.clear();
    *JNI_JOINED.lock().unwrap() = false;
}

// ---------------------------------------------------------------- state

struct ArmedState {
    api: HostApiV0,
}

/// Every armed state, kept for the process like the reference plugin handle;
/// the subscription `user` is its handle.
static STATES: HandleTable<ArmedState> = HandleTable::new();
/// The armed state handle (0 = not armed). Re-init re-arms for the
/// in-process test suite; the loader brings a cdylib up exactly once.
static ARMED: AtomicUsize = AtomicUsize::new(0);

/// Trampoline (callable original) of the installed SetOutput detour.
static SET_OUTPUT_ORIG: Original<SetOutputFn> = Original::new();
/// Whether the org/fmod session join completed.
static JNI_JOINED: Mutex<bool> = Mutex::new(false);

fn api_log(api: &HostApiV0, message: &str) {
    let tag = CString::new(LOG_TAG).expect("static tag is NUL-free");
    api.log_line(&tag, message);
}

// ---------------------------------------------------------------- register-JNI

pub fn register_jni(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let Some(vm) = api.java_vm() else {
        api_log(
            api,
            "no JNI session registered — org/fmod registration skip (deploy \
             android_jni_vm, ticket 52)",
        );
        return 0;
    };
    match join_shared_session(api, vm) {
        Ok(()) => {
            *JNI_JOINED.lock().unwrap() = true;
            // Oracle log line (sdk_fmod.cpp register_jni).
            api_log(api, "FMOD Java/AudioTrack JNI STUB set registered");
            0
        }
        Err(reason) => {
            api_log(api, &format!("org/fmod session registration FAILED: {reason}"));
            -3
        }
    }
}

fn join_shared_session(api: &HostApiV0, vm: JavaVm) -> Result<(), &'static str> {
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return Err("GetEnv failed on the shared session");
    };
    join_with_env(api, env)
}

fn join_with_env(api: &HostApiV0, env: JniEnv) -> Result<(), &'static str> {
    let _ = api;
    let name = |value: &str| CString::new(value).map_err(|_| "class name contained NUL");

    // The oracle registers org/fmod/{FMODAudioDevice,AudioDevice,FMOD};
    // libfmod looks up org/fmod/AudioDevice (fakefmod.h notes). All resolve
    // through the shared session's class table.
    for class in [
        "org/fmod/FMODAudioDevice",
        "org/fmod/AudioDevice",
        "org/fmod/FMOD",
    ] {
        let c_name = name(class)?;
        let handle = env.find_class(&c_name);
        if handle.is_null() {
            if env.exception_check() {
                env.exception_clear();
            }
            return Err("an org/fmod class did not resolve");
        }
    }

    // FMOD.checkInit ()Z == true through the catalog-backed resolution (the
    // oracle's Java surface answers before setOutput runs).
    let fmod_class = name("org/fmod/FMOD")?;
    let fmod = env.find_class(&fmod_class);
    if fmod.is_null() {
        return Err("org/fmod/FMOD did not resolve");
    }
    let c_method = name("checkInit")?;
    let c_sig = name("()Z")?;
    let Some(method_id) = env.static_method(fmod, &c_method, &c_sig) else {
        if env.exception_check() {
            env.exception_clear();
        }
        return Err("FMOD.checkInit did not resolve against the catalog");
    };
    let ready = env.call_static_boolean(fmod, method_id, &[]);
    if !ready {
        return Err("FMOD.checkInit did not answer true");
    }

    // The session anchor.
    let anchor = env.new_global_ref(fmod);
    if anchor.is_null() {
        return Err("NewGlobalRef anchor failed");
    }
    Ok(())
}

#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION { return -2; }
    if let Some(vm) = api.java_vm() {
        let Some(env) = vm.get_env(JNI_VERSION_1_6) else { return -3; };
        if audio::register(api, env) != 0 { return -3; }
    }
    register_jni(api)
}

// ---------------------------------------------------------------- module events

type SetOutputFn = unsafe extern "C" fn(system: *mut c_void, output_type: i32) -> i32;

/// The oracle `set_output_hook`: try the host-sane output order against the
/// callable original, with the reentrancy depth guard, and the forced-
/// NOSOUND OK fallback (oracle sdk_fmod.cpp, line-faithful).
unsafe extern "C" fn set_output_hook(system: *mut c_void, output_type: i32) -> i32 {
    let orig = SET_OUTPUT_ORIG.get();
    let state = STATES.get(ARMED.load(Ordering::SeqCst) as *mut c_void);
    let (Some(orig), Some(state)) = (orig, state) else {
        return STUB_ERR_PLUGIN_MISSING as i32; // FMOD_ERR_PLUGIN_MISSING-ish
    };
    // SAFETY: the typed trampoline receives the guest's live System on the
    // current call thread; SetOutput keeps it alive for every fallback try.
    // This closure stays inside the hook and cannot retain the System.
    let call_original = |candidate| unsafe { orig(system, candidate) };

    // Reentrancy guard (oracle g_set_output_depth): the try-order calls the
    // original, which some hosts re-enter through.
    thread_local! {
        static DEPTH: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    }
    if DEPTH.get() > 0 {
        return call_original(output_type);
    }

    api_log(&state.api, &format!("SetOutput requested type={output_type}"));
    let try_order = [
        OUT_AUDIOTRACK,
        OUT_NOSOUND,
        OUT_OPENSL,
        OUT_AUTODETECT,
        output_type,
    ];

    DEPTH.set(DEPTH.get() + 1);
    let mut best = STUB_ERR_PLUGIN_MISSING as i32;
    for candidate in try_order {
        let result = call_original(candidate);
        api_log(&state.api, &format!("  try type={candidate} -> result={result}"));
        if result == 0 {
            DEPTH.set(DEPTH.get() - 1);
            return 0;
        }
        best = result;
    }

    // All candidates failed: the oracle forces NOSOUND and returns OK — the
    // silent stand-in the reference ships (recorded; the host audio-policy
    // rejection of silent drivers is the kit-side counterpart, see the
    // ticket-55 pump).
    let nosound = call_original(OUT_NOSOUND);
    DEPTH.set(DEPTH.get() - 1);
    if nosound == 0 {
        return 0;
    }
    api_log(
        &state.api,
        &format!("SetOutput all failed (last={best}); returning OK for NOSOUND"),
    );
    0
}

/// Unity module event (the reference on_unity_loaded trigger): check the
/// guest FMOD libraries' presence and log the honest posture.
extern "C" fn on_unity_loaded(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(state)) = (event, STATES.get(user)) else {
        return;
    };
    if event.kind != ModuleKind::Unity as u32 {
        return;
    }
    if UNITY_SEEN.swap(true, Ordering::SeqCst) {
        api_log(&state.api, "repeated unity module event; already attempted (idempotent)");
        return;
    }
    let Some(game_files) = state.api.config_string(c"paths.game_files", 512).filter(|value| !value.is_empty()) else {
        api_log(
            &state.api,
            "paths.game_files not configured; guest library presence unknown (staying pending)",
        );
        return;
    };
    let root = PathBuf::from(&game_files);
    let missing = GUEST_LIBRARIES
        .iter()
        .filter(|library| !root.join(library).is_file())
        .count();
    if missing == GUEST_LIBRARIES.len() {
        set_outcome(FmodOutcome::StubbedMissingGuest);
        api_log(
            &state.api,
            &format!("guest FMOD libraries are missing: {} (stub state, non-fatal)", GUEST_LIBRARIES.join(", ")),
        );
    } else {
        api_log(
            &state.api,
            &format!(
                "guest FMOD libraries present ({missing} absent), awaiting module events"
            ),
        );
    }
}

/// The Fmod module event: the loader mapped libfmod through the ELF
/// provider. Resolve SetOutput and install the real detour (oracle
/// on_fmod_loaded; the JNI_OnLoad call itself is the loader's ticket-52
/// guest contract, which runs right after this event).
extern "C" fn on_fmod_loaded(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(state)) = (event, STATES.get(user)) else {
        return;
    };
    if event.kind != ModuleKind::Fmod as u32 {
        return;
    }
    if FMOD_INSTALL_ATTEMPTED.swap(true, Ordering::SeqCst) {
        api_log(&state.api, "repeated fmod module event; already attempted (idempotent)");
        return;
    }
    if event.base == 0 {
        set_outcome(FmodOutcome::HookFailed {
            reason: "module event reported no load base",
        });
        api_log(&state.api, "FAILED: fmod module event reported no load base");
        return;
    }
    for symbol in SET_OUTPUT_SYMBOLS {
        let c_symbol = CString::new(symbol).expect("static symbol is NUL-free");
        let Some(address) = state.api.guest_symbol(event.base, &c_symbol) else {
            // Oracle parity: an unresolved candidate falls through to the
            // next symbol in the list (so_symbol_all has no v1 equivalent —
            // the event's module base already scopes the lookup).
            continue;
        };
        // SAFETY: address is a resolved code export in the exec mapping.
        match unsafe { install_set_output_detour(address) } {
            Ok(trampoline) => {
                set_outcome(FmodOutcome::Bridged);
                api_log(
                    &state.api,
                    &format!(
                        "hooked {symbol} @ {address:#x} (single) trampoline={trampoline:#x}"
                    ),
                );
                return;
            }
            Err(err) => {
                api_log(&state.api, &format!("detour failed for {symbol}: {err}"));
            }
        }
    }
    set_outcome(FmodOutcome::HookFailed {
        reason: "no SetOutput symbol resolved",
    });
    // Oracle warning line.
    api_log(&state.api, "WARNING: no SetOutput hook installed");
}

/// Install and publish SetOutput before the guest starts calling it.
///
/// # Safety
/// `address` is the mapped SetOutput export and stays idle until publication.
unsafe fn install_set_output_detour(address: usize) -> Result<usize, String> {
    // SAFETY: caller's mapped-code and publication contract; the typed slot
    // enforces the replacement's exact SetOutput signature.
    unsafe { SET_OUTPUT_ORIG.install(address, set_output_hook) }
        .map(|installed| installed.trampoline)
        .map_err(|err| format!("{err:?}"))
}

/// The FmodStudio module event: record the studio library load (the oracle
/// loads it only behind the fmod+JNI gate the loader plan enforces).
extern "C" fn on_fmod_studio_loaded(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(state)) = (event, STATES.get(user)) else {
        return;
    };
    if event.kind != ModuleKind::FmodStudio as u32 {
        return;
    }
    api_log(
        &state.api,
        &format!(
            "studio library loaded through the ELF provider: {} base={:#x}",
            GUEST_LIBRARIES[1], event.base
        ),
    );
}

/// Whether the org/fmod session join completed.
pub fn jni_registered() -> bool {
    *JNI_JOINED.lock().unwrap()
}

// ---------------------------------------------------------------- init

/// Subscribe to the native module lifecycle and arm the FMOD detour.
///
/// # Safety
/// The same-kit host pins both modules and resolves exact SetOutput native
/// prototypes. Fmod events keep the entry idle through patch publication.
pub unsafe fn plugin_init(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let user = STATES.insert(ArmedState {
        api: *api,
    });
    ARMED.store(user as usize, Ordering::SeqCst);
    // Logged as 0 (accepted) / 1 (rejected): the host's own code is not surfaced.
    let code = |accepted: bool| c_int::from(!accepted);
    let registered_unity = code(api.on_module_loaded(ModuleKind::Unity, on_unity_loaded, user));
    let registered_fmod = code(api.on_module_loaded(ModuleKind::Fmod, on_fmod_loaded, user));
    let registered_studio = code(api.on_module_loaded(ModuleKind::FmodStudio, on_fmod_studio_loaded, user));
    if registered_unity != 0 || registered_fmod != 0 || registered_studio != 0 {
        api_log(
            api,
            &format!(
                "module subscription rejected (unity={registered_unity}, \
                 fmod={registered_fmod}, fmodstudio={registered_studio}) — not armed"
            ),
        );
        return -3;
    }
    // Oracle line (sdk_fmod.cpp init).
    api_log(api, "FMOD SDK compatibility plugin armed");
    0
}

#[no_mangle]
/// # Safety
/// `api` is the process-pinned same-kit host table; symbol lookup and emitted
/// module events identify live, correctly typed guest exports.
pub unsafe extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    // SAFETY: the native entry requires the same host and lifecycle contract.
    unsafe { plugin_init(api) }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
