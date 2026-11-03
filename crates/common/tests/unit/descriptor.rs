//! INPUT:  private items of `src/descriptor.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/descriptor.rs`; declared there with `#[path]`
use super::*;

#[test]
fn kinds_match_reference_discriminants() {
    // RudroidPluginKind: LOGGING=1 .. GAME_PATCH=7.
    assert_eq!(Kind::Logging as u32, 1);
    assert_eq!(Kind::AndroidBase as u32, 2);
    assert_eq!(Kind::UnityProfile as u32, 3);
    assert_eq!(Kind::Platform as u32, 4);
    assert_eq!(Kind::AndroidAddon as u32, 5);
    assert_eq!(Kind::Runtime as u32, 6);
    assert_eq!(Kind::GamePatch as u32, 7);
}

#[test]
fn kind_round_trip() {
    for kind in Kind::ALL {
        assert_eq!(Kind::from_u32(kind as u32), Some(kind));
    }
    assert_eq!(Kind::from_u32(0), None);
    assert_eq!(Kind::from_u32(8), None);
}

#[test]
fn descriptor_builder_records_gate_and_deps() {
    let d = CapabilityDescriptor::new("silksong_unity6", Kind::GamePatch)
        .target_package("com.game.silksong")
        .requires(&["unity_6", "android_base"]);
    assert_eq!(d.target_package, Some("com.game.silksong"));
    assert_eq!(d.required, ["unity_6", "android_base"]);
    assert_eq!(d.kind, Kind::GamePatch);
}

#[test]
fn package_gate_follows_reference_semantics() {
    // Gated patch: loads only on a package match.
    assert!(package_gate(Some("com.game.silksong"), Some("com.game.silksong")));
    assert!(!package_gate(Some("com.game.silksong"), Some("com.other")));
    assert!(!package_gate(Some("com.game.silksong"), None));
    // Ungated capability: always loads.
    assert!(package_gate(None, Some("com.game.silksong")));
    assert!(package_gate(None, None));
}

#[test]
fn plain_descriptor_has_no_gate_or_deps() {
    let d = CapabilityDescriptor::new("log_stderr", Kind::Logging);
    assert_eq!(d.target_package, None);
    assert!(d.required.is_empty());
}
