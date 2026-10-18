//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn owns_the_three_task_listener_interfaces() {
    let (id, interfaces) = provider_registration();
    assert_eq!(id, "sdk_google_gms");
    assert_eq!(
        interfaces,
        [
            "com/google/android/gms/tasks/OnCompleteListener",
            "com/google/android/gms/tasks/OnSuccessListener",
            "com/google/android/gms/tasks/OnFailureListener",
        ]
    );
}

#[test]
fn registered_surface_carries_oracle_values() {
    // Oracle google_gms.cpp: isSuccessful() -> true, isAuthenticated() -> false.
    assert_eq!(IS_SUCCESSFUL, ("isSuccessful", "()Z"));
    assert_eq!(IS_AUTHENTICATED, ("isAuthenticated", "()Z"));
    let invocation = complete_invocation(42);
    assert_eq!(invocation.method_name, "onComplete");
    assert_eq!(
        invocation.interface_name,
        "com/google/android/gms/tasks/OnCompleteListener"
    );
}

#[test]
fn outcome_starts_pending_and_tracks_lifecycle() {
    let _guard = test_lock();
    reset_state_for_tests();
    assert_eq!(last_outcome(), None);
    set_outcome(RegistrationOutcome::NoSession);
    assert_eq!(last_outcome(), Some(RegistrationOutcome::NoSession));
    set_outcome(RegistrationOutcome::Failed {
        reason: "Task.isSuccessful did not resolve against the catalog",
    });
    assert!(matches!(last_outcome(), Some(RegistrationOutcome::Failed { .. })));
    reset_state_for_tests();
}
