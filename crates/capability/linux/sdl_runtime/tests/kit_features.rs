//! INPUT: outer SDL feature and declared Docker kit selection
//! OUTPUT: mismatch failure before accepting a mislabelled kit
//! POS: SDL integration contract

#[test]
fn outer_sdl_feature_matches_the_declared_kit_features() {
    rudroid_test_support::cargo::assert_kit_features(platform_sdl_runtime::SDL_ENABLED);
}
