//! INPUT:  shared player lifecycle, HostApi module events and verified Unity native images
//! OUTPUT:  engine Spec, profile registration, gated cleanup repair/presentation facade and optional Swappy fallback
//! POS:    Unity 2020–2021.3 engine profile; native patches stay version-owned
//! rudroid-unity_2021_3: Unity engine profile capability (tickets 26/53).
//!
//! The prepare/bootstrap/run player lifecycle every engine version shares
//! lives in `rudroid_unity_player_lifecycle`; this crate owns only what
//! differs for Unity 2020–2021.3:
//!
//! 1. [`ENGINE_SPEC`] — the reference `unity_engine::Spec` for
//!    `plugins/unity_2021_3` (player class, JNI signatures, init flags) plus
//!    the lifecycle data (log tag, callbacks, shared `currentActivity`
//!    publication and older Java services).
//! 2. `rudroid_plugin_register_jni` — the early JNI phase, delegated to
//!    the shared `register_jni` with this spec.
//! 3. `rudroid_plugin_init` — subscribes the exact-image cleanup-order
//!    repair and optional Swappy fallback to the Unity module event.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod teardown_order;

use std::ffi::{c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use rudroid_common::arm64_detour::Original;
use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::{RudroidModuleEvent, RUDROID_UNITY_PROFILE_NONE};
use rudroid_common::ModuleKind;
use rudroid_unity_player_lifecycle::{self as lifecycle, Profile, Spec, INIT_JNI_CONTEXT};

rudroid_common::declare_descriptor!(ENGINE_SPEC.id, rudroid_common::Kind::UnityProfile);

/// Engine specification — the reference fields mirror `unity_engine::Spec`
/// for unity_2021_3 (`projects/unityloader/plugins/unity_2021_3`).
pub const ENGINE_SPEC: Spec = Spec {
    id: "unity_2021_3",
    log_tag: c"UNITY2021",
    registered_message: "Unity 2021.3 JNI and early engine profile registered",
    player_class: "com/unity3d/player/UnityPlayer",
    player_constructor_signature: "()V",
    init_jni_signature: INIT_JNI_CONTEXT,
    flags: RUDROID_UNITY_PROFILE_NONE,
    init_jni_int_arg: 0,
    render_sleep_us: 0,
    extra_player_callbacks: &[],
    publishes_current_activity: true,
    register_java_services: Some(lifecycle::java::register_2021),
};

/// This verified image needs the C host-presentation facade to select its
/// working Android audio path. Other images must not inherit that audio reset.
fn enable_verified_presentation_route(api: &HostApiV0) -> bool {
    use rudroid_common::jni_env::{Arg, JNI_VERSION_1_6};
    let Some(env) = api.java_vm().and_then(|vm| vm.get_env(JNI_VERSION_1_6)) else {
        return false;
    };
    if env.exception_check() { return false; }
    let class = env.find_class(c"android/media/MediaRouter");
    let method = env.static_method(class, c"_rudroidSetHostPresentationRoute", c"(Z)V");
    if let Some(method) = method {
        env.call_static_void(class, method, &[Arg::bool(true)]);
    }
    let succeeded = method.is_some() && !env.exception_check();
    env.delete_local_ref(class);
    if !succeeded {
        env.exception_describe();
        env.exception_clear();
    }
    succeeded
}

struct Unity2021;

impl Profile for Unity2021 {
    const SPEC: &'static Spec = &ENGINE_SPEC;
}

#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    lifecycle::register_jni::<Unity2021>(api)
}

fn log_on(api: &HostApiV0, message: &str) {
    lifecycle::log(api, ENGINE_SPEC.log_tag, message);
}

/// Host table pinned for the Unity module event and the Swappy hook log.
static NATIVE_API: OnceLock<HostApiV0> = OnceLock::new();

/// Arm the startup-only Unity native patch callback. The optional Swappy
/// fallback uses the ABI's .symtab-capable symbol lookup; teardown repair uses
/// dynamic exports and exact native-image evidence.
#[no_mangle]
/// # Safety
/// The host table and callbacks remain mapped for the process; guest symbol
/// lookup and module events obey the same-kit native ABI and quiescent hook
/// installation contract.
unsafe extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -1;
    }
    if NATIVE_API.set(*api).is_err() {
        return 0;
    }
    if !api.on_module_loaded(ModuleKind::Unity, on_unity_mapped, std::ptr::null_mut()) {
        log_on(api, "failed to register Unity module callback");
        return -1;
    }
    log_on(api, "Unity 2021.3 compatibility plugin armed");
    0
}

/// Startup-only native patches. Each gets one install attempt; teardown repair
/// requires an exact native-image match, while Swappy also needs explicit opt-in.
extern "C" fn on_unity_mapped(event: Option<&RudroidModuleEvent>, _user: *mut c_void) {
    let Some(event) = event else {
        return;
    };
    let Some(api) = NATIVE_API.get() else {
        return;
    };
    if event.kind != ModuleKind::Unity as u32 || event.base == 0 {
        return;
    }
    // SAFETY: the host's module event supplies a process-pinned Unity image
    // before player startup; no thread executes the affected entries yet.
    unsafe {
        #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
        if !teardown_order::INSTALL_ATTEMPTED.swap(true, Ordering::AcqRel) {
            match teardown_order::install(api, event.base) {
                Ok(true) => {
                    log_on(api, "verified Unity 2020.2.2f1 teardown: native owner stops producers; native metadata cache before allocator");
                    if enable_verified_presentation_route(api) {
                        log_on(api, "verified Unity 2020.2.2f1 host-presentation audio compatibility enabled");
                    } else {
                        log_on(api, "required host-presentation audio compatibility control unavailable");
                    }
                },
                Ok(false) => {},
                Err(error) => log_on(api, &format!("Unity cleanup-order repair refused: {error}; code was not repaired")),
            }
        }
        if api.config_bool(c"unity.swappy_invalid_interval_fallback", false)
            && !swappy::INSTALL_ATTEMPTED.swap(true, Ordering::AcqRel)
        {
            let refresh_rate = api.config_i64(c"device.displayRefreshRate", 60);
            // Swappy is usually hidden; use the .symtab-capable lookup.
            let target = api.guest_symbol_all(event.base, swappy::SYMBOL);
            swappy::install(target, refresh_rate, &|message| log_on(api, message));
        }
    }
}

/// Reference Swappy invalid-interval fallback (`plugins/unity_2021_3`):
/// `SwappyGL_getSwapIntervalNS` returning `UINT64_MAX` is replaced by the
/// display refresh interval; any valid interval passes through unchanged.
pub mod swappy {
    use super::*;

    pub const SYMBOL: &CStr = c"SwappyGL_getSwapIntervalNS";

    pub(crate) static INSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);
    pub(crate) static ORIGINAL: Original<GetSwapIntervalNsFn> = Original::new();
    type GetSwapIntervalNsFn = unsafe extern "C" fn() -> u64;
    static FALLBACK_INTERVAL_NS: AtomicU64 = AtomicU64::new(16_666_667);
    static LOGGED: AtomicBool = AtomicBool::new(false);

    /// Reference clamp: `device.displayRefreshRate` outside 1..=1000 → 60.
    pub fn refresh_rate(configured: i64) -> i64 {
        if configured <= 0 || configured > 1000 {
            60
        } else {
            configured
        }
    }

    pub fn fallback_interval_ns(refresh_rate: i64) -> u64 {
        1_000_000_000 / refresh_rate as u64
    }

    /// The detour body (reference `get_swap_interval_ns_hook`).
    pub extern "C" fn get_swap_interval_ns_hook() -> u64 {
        let interval = match ORIGINAL.get() {
            // SAFETY: the trampoline replays the saved prologue of
            // `uint64_t SwappyGL_getSwapIntervalNS()`.
            Some(original) => unsafe { original() },
            None => u64::MAX,
        };
        if interval != u64::MAX {
            return interval;
        }
        let fallback = FALLBACK_INTERVAL_NS.load(Ordering::Relaxed);
        if !LOGGED.swap(true, Ordering::AcqRel) {
            if let Some(api) = NATIVE_API.get() {
                log_on(
                    api,
                    &format!("invalid swap interval UINT64_MAX; fallback={fallback} ns"),
                );
            }
        }
        fallback
    }

    /// Install the fallback over `target`; returns the trampoline.
    ///
    /// # Safety
    /// `target` is `None` or `SwappyGL_getSwapIntervalNS` in a mapped,
    /// process-owned code image.
    pub unsafe fn install(target: Option<usize>, refresh_rate: i64, log: &dyn Fn(&str)) -> Option<usize> {
        let refresh_rate = self::refresh_rate(refresh_rate);
        FALLBACK_INTERVAL_NS.store(fallback_interval_ns(refresh_rate), Ordering::Relaxed);
        let Some(target) = target else {
            log("SwappyGL_getSwapIntervalNS not found; fallback not installed");
            return None;
        };
        // SAFETY: caller contract; the guest has not called the target yet.
        let original = unsafe { ORIGINAL.install(target, get_swap_interval_ns_hook) }
            .ok().map(|detour| detour.trampoline);
        let Some(original) = original else {
            log("failed to install invalid-interval fallback");
            return None;
        };
        log(&format!(
            "installed invalid-interval fallback at {target:#x} ({refresh_rate} Hz)"
        ));
        Some(original)
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
