//! INPUT:  private items of `src/build_id.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/build_id.rs`; declared there with `#[path]`
use super::*;

#[test]
fn compiled_fallback_is_shared_default() {
    // The build script injects a deterministic kit id (env override or
    // source hash); every compilation unit in the same build must agree
    // with it.
    let id = compiled_build_id();
    assert!(!id.is_empty());
    assert!(check_same_build(id).is_ok());
}

#[test]
fn mismatch_reports_both_ids() {
    let foreign = "some-other-build";
    let err = check_same_build(foreign).unwrap_err();
    assert_eq!(err.0, compiled_build_id());
    assert_eq!(err.1, foreign);
}
