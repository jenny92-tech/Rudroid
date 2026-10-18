//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn owns_the_play_core_namespace_not_gms() {
    // Ticket 56 divergence fix: the oracle registers play-core names.
    assert_eq!(
        INTERFACES,
        [
            "com/google/android/play/core/tasks/OnCompleteListener",
            "com/google/android/play/core/tasks/OnSuccessListener",
            "com/google/android/play/core/tasks/OnFailureListener",
        ]
    );
    let (id, _) = provider_registration();
    assert_eq!(id, "sdk_google_play_core");
    assert!(TASK_CLASS.starts_with("com/google/android/play/core/tasks/"));
}

#[test]
fn surface_and_statuses_are_documented() {
    let (method, sig) = ADD_ON_COMPLETE_LISTENER;
    assert_eq!(method, "addOnCompleteListener");
    assert!(sig.starts_with("(Lcom/google/android/play/core/tasks/"));
    assert_eq!(ASSET_PACK_STATUS_ACTIVE, 3);
    assert_eq!(ASSET_PACK_STATUS_COMPLETED, 4);
}

#[test]
fn outcome_starts_pending_and_tracks_lifecycle() {
    let _guard = test_lock();
    reset_state_for_tests();
    assert_eq!(last_outcome(), None);
    set_outcome(RegistrationOutcome::Joined);
    assert_eq!(last_outcome(), Some(RegistrationOutcome::Joined));
    reset_state_for_tests();
}
