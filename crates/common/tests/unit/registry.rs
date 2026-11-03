//! INPUT:  private items of `src/registry.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/registry.rs`; declared there with `#[path]`
use super::*;
use std::mem::size_of;

#[test]
fn pods_are_pointer_sized_fields_only() {
    // Every field is an integer or pointer; no Rust types cross the ABI.
    assert_eq!(size_of::<RudroidModuleEvent>(), 8 + 8 + 8);
    assert_eq!(size_of::<RudroidAndroidHooks>(), 16);
    assert_eq!(size_of::<RudroidAudioPolicy>(), 8 + 4 * 3 + 4 * 3);
    assert_eq!(size_of::<CatalogProviderV0>(), 4 * 8);
}

#[test]
fn profile_flags_match_reference() {
    assert_eq!(RUDROID_UNITY_PROFILE_NONE, 0);
    assert_eq!(RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG, 1);
}
