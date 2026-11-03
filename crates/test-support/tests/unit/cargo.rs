//! INPUT:  private items of `src/cargo.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/cargo.rs`; declared there with `#[path]`
use super::*;

#[test]
fn kit_feature_syntax_matches_the_docker_entry() {
    assert_eq!(parse_kit_features("").unwrap(), Vec::<String>::new());
    assert_eq!(
        parse_kit_features(&format!("--features {SDL_FEATURE}")).unwrap(),
        [SDL_FEATURE]
    );
    assert_eq!(
        parse_kit_features("--features=a/x,b/y --features c/z").unwrap(),
        ["a/x", "b/y", "c/z"]
    );
    assert!(parse_kit_features("--all-features").is_err());
    assert!(parse_kit_features("--features plain").is_err());
    assert!(parse_kit_features("--features").is_err());
}

#[test]
fn package_builds_get_their_own_plain_features() {
    let kit = vec![SDL_FEATURE.to_owned(), "other/x".to_owned()];
    assert_eq!(select_features(&kit, None), [SDL_FEATURE, "other/x"]);
    assert_eq!(select_features(&kit, Some("rudroid-linux-sdl-runtime")), ["sdl"]);
    assert!(select_features(&kit, Some("rudroid-android-elf-load")).is_empty());
}

#[test]
fn explicit_targets_do_not_resolve_stale_host_artifacts() {
    use std::ffi::OsStr;
    let root = Path::new("cache/target");
    assert_eq!(profile_path_for(root, None), root.join(profile_dir()));
    assert_eq!(profile_path_for(root, Some(OsStr::new(""))), root.join(profile_dir()));
    assert_eq!(profile_path_for(root, Some(OsStr::new("aarch64-unknown-linux-gnu"))),
               root.join("aarch64-unknown-linux-gnu").join(profile_dir()));
    assert_eq!(profile_path_for(root, Some(OsStr::new("/toolchains/handheld.json"))),
               root.join("handheld").join(profile_dir()));
}
