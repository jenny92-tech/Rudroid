//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  unit coverage for path validation and defaults
//! POS:    keeps test bodies out of `src/lib.rs`
use super::*;

#[test]
fn relative_guest_paths_reject_escapes() {
    assert!(valid_guest_relative_path("lib/arm64-v8a/libgame.so"));
    assert!(!valid_guest_relative_path(""));
    assert!(!valid_guest_relative_path("/abs/libgame.so"));
    assert!(!valid_guest_relative_path("../libgame.so"));
    assert!(!valid_guest_relative_path("lib/../libgame.so"));
    assert!(!valid_guest_relative_path("./libgame.so"));
}

#[test]
fn default_entrypoint_is_ndk_on_create() {
    assert_eq!(DEFAULT_ENTRYPOINT.to_bytes(), b"ANativeActivity_onCreate");
}
