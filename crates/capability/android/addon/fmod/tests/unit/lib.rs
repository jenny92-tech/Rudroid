//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

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
