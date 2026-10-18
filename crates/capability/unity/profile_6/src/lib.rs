//! INPUT:  shared Unity player lifecycle library, Unity 6 Java services and the il2cpp module event
//! OUTPUT:  unity_6 descriptor, engine Spec, early profile registration and the runtime-invoke Bionic TLS guard
//! POS:    Unity 6 engine profile: a Spec literal, its Java services and this version's native-phase patch
//! rudroid-unity_6: Unity engine profile capability (tickets 27/53).
//!
//! The prepare/bootstrap/run player lifecycle every engine version shares
//! lives in `rudroid_unity_player_lifecycle`; this crate owns only what
//! differs for Unity 6:
//!
//! 1. [`ENGINE_SPEC`] — the reference `unity_engine::Spec` for
//!    `plugins/unity_6` (`UnityPlayerForActivityOrService`, the
//!    `(Landroid/content/Context;I)V` initJni shape, `INIT_JNI_INT_ARG`)
//!    plus the lifecycle data: the extra `initializeGoogleAr` player
//!    callback, `UnityPlayer.currentActivity` publication and the Java
//!    services in [`java`] (ReflectionHelper, interface proxies, PAD).
//! 2. `rudroid_plugin_register_jni` — the early JNI phase, delegated to
//!    the shared `register_jni`; records that the JNI phase ran.
//! 3. `rudroid_plugin_init` — the reference native phase: once libil2cpp
//!    is mapped (module event), suppress the false Bionic stack-guard branch
//!    in Unity 6's `il2cpp_runtime_invoke` wrapper ([`tls_guard`]).

use rudroid_unity_player_lifecycle::java;


use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::{RudroidModuleEvent, RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG};
use rudroid_common::ModuleKind;
use rudroid_unity_player_lifecycle::{
    self as lifecycle, CallbackBody, PlayerCallback, Profile, Spec, INIT_JNI_CONTEXT_INT,
};

rudroid_common::declare_descriptor!(ENGINE_SPEC.id, rudroid_common::Kind::UnityProfile);

/// Engine ABI mirrors the reference `unity_engine::Spec` for unity_6
/// (`projects/unityloader/plugins/unity_6`); pacing defaults have moved to
/// target_fps, so the previous extra 16ms sleep is zero.
pub const ENGINE_SPEC: Spec = Spec {
    id: "unity_6",
    log_tag: c"UNITY6",
    registered_message: "Unity 6 early engine profile registered",
    player_class: "com/unity3d/player/UnityPlayerForActivityOrService",
    player_constructor_signature: "(Landroid/content/Context;)V",
    init_jni_signature: INIT_JNI_CONTEXT_INT,
    flags: RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG,
    init_jni_int_arg: 0,
    render_sleep_us: 0,
    extra_player_callbacks: &[PlayerCallback {
        name: c"initializeGoogleAr",
        body: CallbackBody::False,
    }],
    publishes_current_activity: true,
    register_java_services: Some(java::register_6),
};

struct Unity6;

impl Profile for Unity6 {
    const SPEC: &'static Spec = &ENGINE_SPEC;
}

#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let registered = lifecycle::register_jni::<Unity6>(api);
    if registered == 0 {
        JNI_REGISTERED.store(true, Ordering::Release);
    }
    registered
}

fn log_on(api: &HostApiV0, message: &str) {
    lifecycle::log(api, ENGINE_SPEC.log_tag, message);
}

/// Reference `g_jni_registered`: the native phase requires the JNI phase.
static JNI_REGISTERED: AtomicBool = AtomicBool::new(false);
/// Host table pinned for the il2cpp module event (process lifetime).
static NATIVE_API: OnceLock<HostApiV0> = OnceLock::new();

/// The reference native phase (`rudroid_plugin_init`): the C++ host calls
/// it right after libil2cpp is mapped; here the module event delivers that
/// point, so init subscribes and the guard installs on the il2cpp event.
#[no_mangle]
/// # Safety
/// The host table and callbacks remain mapped for the process; guest symbol
/// lookup and module events obey the same-kit native ABI and quiescent hook
/// installation contract.
unsafe extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION
        || !JNI_REGISTERED.load(Ordering::Acquire)
    {
        return -1;
    }
    if NATIVE_API.set(*api).is_err() {
        return 0;
    }
    if !api.on_module_loaded(ModuleKind::Il2cpp, on_il2cpp_mapped, std::ptr::null_mut()) {
        log_on(api, "il2cpp module subscription rejected; TLS guard not armed");
        return -1;
    }
    0
}

extern "C" fn on_il2cpp_mapped(event: Option<&RudroidModuleEvent>, _user: *mut c_void) {
    let Some(event) = event else {
        return;
    };
    let Some(api) = NATIVE_API.get() else {
        return;
    };
    if event.kind != ModuleKind::Il2cpp as u32 || event.base == 0 {
        return;
    }
    let runtime_invoke = api.guest_symbol(event.base, c"il2cpp_runtime_invoke");
    // SAFETY: a resolved export of the freshly mapped il2cpp image.
    unsafe { tls_guard::install(runtime_invoke, &|message| log_on(api, message)) };
}

/// Reference `il2cpp_compat::install_runtime_invoke_tls_guard`
/// (`plugins/unity_6/il2cpp_compat.cpp`).
///
/// Android's ARM64 stack protector reads Bionic's TLS_SLOT_STACK_GUARD at
/// TP+40. The guest runs on a glibc thread, where that address is ordinary
/// host TLS and can legitimately change while a managed method calls a
/// JNI/libc thunk, so Unity 6's runtime-invoke wrapper reports a false stack
/// smash. Only the wrapper's complete epilogue pattern is matched, and only
/// its comparison branch is replaced by a `NOP`; host and capability stack
/// protection stay intact.
pub mod tls_guard {
    /// `ldr x8, [x24, #40]; ldur x9, [x29, #-8]; cmp x8, x9;
    /// b.ne __stack_chk_fail` — current vs entry TP+40.
    pub const RUNTIME_INVOKE_TLS_CHECK: [u32; 4] =
        [0xF940_1708, 0xF85F_83A9, 0xEB09_011F, 0x5400_00E1];
    pub const NOP: u32 = 0xD503_201F;
    /// Words of the wrapper body scanned for the pattern.
    pub const SCAN_INSTRUCTIONS: usize = 256;

    const BRANCH_MASK: u32 = 0xFC00_0000;
    const BRANCH_OPCODE: u32 = 0x1400_0000;

    /// Address of the matched pattern: `il2cpp_runtime_invoke` must open
    /// with a `B` to its implementation, whose first `SCAN_INSTRUCTIONS`
    /// words contain the complete pattern.
    ///
    /// # Safety
    /// `runtime_invoke` must be a mapped code address whose branch target
    /// is readable for `SCAN_INSTRUCTIONS` words.
    pub unsafe fn find_check(runtime_invoke: usize) -> Option<usize> {
        // SAFETY: caller contract.
        let entry = unsafe { (runtime_invoke as *const u32).read_volatile() };
        if entry & BRANCH_MASK != BRANCH_OPCODE {
            return None;
        }
        let implementation =
            rudroid_common::arm64_detour::decode_b(entry, runtime_invoke) as *const u32;
        (0..=SCAN_INSTRUCTIONS - RUNTIME_INVOKE_TLS_CHECK.len())
            .find(|index| {
                RUNTIME_INVOKE_TLS_CHECK.iter().enumerate().all(|(offset, word)| {
                    // SAFETY: inside the caller-guaranteed scan window.
                    unsafe { implementation.add(index + offset).read_volatile() == *word }
                })
            })
            // SAFETY: as above.
            .map(|index| unsafe { implementation.add(index) as usize })
    }

    // Address-space bounds are checked before following an export's B.
    // The install caller still owns the image and excludes unmap/patch races.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    fn readable(address: usize, len: usize) -> bool {
        let Some(end) = address.checked_add(len) else { return false; };
        let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else { return false; };
        maps.lines().any(|line| {
            let mut fields = line.split_whitespace();
            let (Some(range), Some(permissions)) = (fields.next(), fields.next()) else { return false; };
            let Some((start, stop)) = range.split_once('-') else { return false; };
            let (Ok(start), Ok(stop)) = (usize::from_str_radix(start, 16), usize::from_str_radix(stop, 16)) else { return false; };
            permissions.starts_with('r') && address >= start && end <= stop
        })
    }

    /// Install the guard; returns the patched pattern address. Every
    /// outcome is logged with the reference wording.
    ///
    /// # Safety
    /// `runtime_invoke` is 0 or the guest's `il2cpp_runtime_invoke` export
    /// in a mapped, process-owned code image, kept mapped and idle through
    /// pattern scanning and patching. Unreadable scan windows are rejected.
    pub unsafe fn install(runtime_invoke: Option<usize>, log: &dyn Fn(&str)) -> Option<usize> {
        let Some(runtime_invoke) = runtime_invoke else {
            log("il2cpp_runtime_invoke not found; TLS guard skipped");
            return None;
        };
        #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
        {
            if runtime_invoke % 4 != 0 || !readable(runtime_invoke, 4) {
                log("runtime-invoke scan window unreadable; guard skipped");
                return None;
            }
            // SAFETY: checked readable aligned entry, pinned by the caller.
            let entry = unsafe { (runtime_invoke as *const u32).read_volatile() };
            if entry & BRANCH_MASK == BRANCH_OPCODE {
                let implementation = rudroid_common::arm64_detour::decode_b(entry, runtime_invoke);
                if !readable(implementation, SCAN_INSTRUCTIONS * 4) {
                    log("runtime-invoke scan window unreadable; guard skipped");
                    return None;
                }
            }
            // SAFETY: both entry and branch scan window are readable and pinned.
            let Some(check) = (unsafe { find_check(runtime_invoke) }) else {
                log("runtime-invoke TLS pattern not present; guard skipped");
                return None;
            };
            // SAFETY: the matched b.ne inside the guest image; replacing a
            // conditional branch with a NOP is a single aligned store.
            if unsafe { rudroid_common::arm64_detour::patch_word(check + 12, NOP) }.is_err() {
                log("runtime-invoke TLS guard mprotect failed");
                return None;
            }
            log(&format!(
                "installed glibc/Bionic runtime-invoke TLS guard at {check:#x}"
            ));
            Some(check)
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
        {
            let _ = runtime_invoke;
            log("runtime-invoke TLS guard is ARM64-only");
            None
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
