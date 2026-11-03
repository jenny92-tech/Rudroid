//! INPUT:  private items of `src/build_id.rs` via `super`
//! OUTPUT:  `mismatch_kit_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/build_id.rs`; declared there with `#[path]`
use super::*;

#[test]
fn intentionally_mismatched_id_is_refused() {
    // Simulates a capability built by another kit build arriving in a
    // deploy dir: the loader compares descriptor id vs kit id.
    let kit = compiled_build_id();
    let foreign = if kit == "foreign-build" { "local-build" } else { "foreign-build" };
    let (active, candidate) = check_same_build(foreign).unwrap_err();
    assert_eq!(active, kit);
    assert_eq!(candidate, foreign);
}

#[test]
fn compiled_id_is_never_empty() {
    assert!(!compiled_build_id().is_empty());
}

// `install_active_build_id` is process-global by design; its accept/reject
// behavior is exercised by the loader integration tests (tickets 07/31)
// rather than inside this unit test binary.
