//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::registry::{HostStr, RudroidModuleLoadedCallback};

#[test]
fn oracle_constants_are_byte_faithful() {
    assert_eq!(STUB_ERR_PLUGIN_MISSING, 18);
    assert_eq!(SET_OUTPUT_SYMBOLS[0], "FMOD5_System_SetOutput");
    assert_eq!(SET_OUTPUT_SYMBOLS[2], "_ZN4FMOD6System9setOutputE15FMOD_OUTPUTTYPE");
    assert_eq!(OUT_AUDIOTRACK, 15);
    assert_eq!(OUT_NOSOUND, 2);
    assert_eq!(OUT_OPENSL, 16);
    assert!(GUEST_LIBRARIES.iter().all(|p| p.contains("libfmod")));
    assert_eq!(LOG_TAG, "FMOD");
}

#[test]
fn outcome_tracks_the_honest_states() {
    let _guard = test_lock();
    reset_state_for_tests();
    assert_eq!(last_outcome(), None);
    set_outcome(FmodOutcome::StubbedMissingGuest);
    assert_eq!(last_outcome(), Some(FmodOutcome::StubbedMissingGuest));
    set_outcome(FmodOutcome::HookFailed {
        reason: "no SetOutput symbol resolved",
    });
    assert!(matches!(last_outcome(), Some(FmodOutcome::HookFailed { .. })));
    assert!(!jni_registered(), "no session join before register-jni runs");
    reset_state_for_tests();
}

/// Host Config key `sdk_fmod.embedded_unity_audio` (default false) selects the
/// embedded-Unity posture. The guarantee under test is narrow but load-bearing:
/// in that mode init subscribes to NOTHING and arms no state, so a `Fmod` module
/// event can never reach the oracle's forced-NOSOUND + OK fallback and no
/// `SetOutput` detour exists. Everything else the host table offers must stay
/// untouched, which `stubbed()` enforces by panicking on any other entry.
#[test]
fn embedded_mode_installs_no_module_subscription_or_detour() {
    let _guard = test_lock();
    reset_state_for_tests();

    /// Records the kinds the host was asked to subscribe to, installing none.
    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    extern "C" fn record(kind: u32, _: Option<RudroidModuleLoadedCallback>, _: *mut c_void) -> c_int {
        SEEN.lock().unwrap().push(kind);
        0
    }
    extern "C" fn config_bool(key: HostStr<'_>, fallback: c_int) -> c_int {
        match key.to_cstr().map(|key| key.to_bytes()) {
            Some(b"sdk_fmod.embedded_unity_audio") => 1,
            _ => fallback,
        }
    }
    extern "C" fn ignore_log(_: HostStr<'_>, _: HostStr<'_>) {}
    extern "C" fn missing_vm() -> Option<JavaVm> { None }
    extern "C" fn default_bool(_: HostStr<'_>, fallback: c_int) -> c_int { fallback }

    let api = HostApiV0 {
        config_get_bool: config_bool,
        register_module_loaded: record,
        log: ignore_log,
        shared_vm: missing_vm,
        ..HostApiV0::stubbed()
    };
    // SAFETY: the table and recorder stay live for the whole test; no guest
    // module is mapped and no callback is installed.
    assert_eq!(unsafe { plugin_init(&api) }, 0, "the phase stays accepted");
    assert!(SEEN.lock().unwrap().is_empty(), "embedded mode subscribes to nothing");
    assert_eq!(ARMED.load(Ordering::SeqCst), 0, "no armed state can serve a hook body");

    assert_eq!(register_jni(&api), -3, "missing VM cannot pass embedded registration");
    assert!(!jni_registered());
    let legacy = HostApiV0 { config_get_bool: default_bool, ..api };
    assert_eq!(register_jni(&legacy), 0, "the existing legacy missing-VM skip stays unchanged");
    assert_eq!(last_outcome(), None, "no guest-library chain outcome is claimed");
    assert!(SET_OUTPUT_ORIG.get().is_none(), "no trampoline was installed");
    assert!(!FMOD_INSTALL_ATTEMPTED.load(Ordering::SeqCst), "the hook path never ran");
    reset_state_for_tests();
}
