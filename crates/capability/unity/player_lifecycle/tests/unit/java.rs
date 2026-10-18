//! INPUT:  private items of `src/java.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/java.rs`; declared there with `#[path]`
use super::*;

#[test]
fn every_native_matches_its_descriptor() {
    // `Native::new*` panics when a Rust prototype disagrees with its descriptor.
    let natives = natives(false);
    assert_eq!(
        natives
            .iter()
            .map(|(_, natives)| natives.len())
            .sum::<usize>(),
        37
    );
    let player = &natives
        .iter()
        .find(|(class, _)| *class == c"com/unity3d/player/UnityPlayer")
        .unwrap()
        .1;
    assert_eq!(
        player
            .iter()
            .filter(|native| native.name() == c"getNetworkProxySettings")
            .count(),
        1
    );
    for class in [
        c"com/unity3d/player/UnityPlayer",
        c"com/unity3d/player/UnityPlayerForActivityOrService",
    ] {
        let methods = &natives.iter().find(|(name, _)| *name == class).unwrap().1;
        assert_eq!(
            methods
                .iter()
                .filter(|m| m.name() == c"getNetworkConnectivity")
                .count(),
            1
        );
    }
    let reflection = &natives
        .iter()
        .find(|(class, _)| *class == c"com/unity3d/player/ReflectionHelper")
        .unwrap()
        .1;
    assert_eq!(
        reflection
            .iter()
            .filter(|native| native.name() == c"newProxyInstance")
            .count(),
        2
    );
}

#[test]
fn older_profile_uses_the_context_only_pad_signature_and_missing_delivery_reply() {
    let n = natives(true);
    assert!(n.iter().all(|(class, _)| *class != c"com/unity3d/player/UnityAccessibilityDelegate"),
        "the Unity 6 delegate must not change the older profile's Java contract");
    let player = &n
        .iter()
        .find(|(c, _)| *c == c"com/unity3d/player/UnityPlayer")
        .unwrap()
        .1;
    assert!(player.iter().any(|m| m.name() == c"initializeGoogleAr"));
    let pad = &n
        .iter()
        .find(|(c, _)| *c == c"com/unity3d/player/PlayAssetDeliveryUnityWrapper")
        .unwrap()
        .1;
    assert_eq!(pad.iter().filter(|m| m.name() == c"init").count(), 1);
}
