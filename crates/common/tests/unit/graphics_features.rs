//! INPUT:  compiled display feature contract
//! OUTPUT:  canonical tokens and rejection of unsupported/ownerless features
//! POS:    prevents a launcher from advertising features of a missing backend
use super::*;

#[test]
fn backend_features_require_their_owner_and_known_bits() {
    for bits in [1<<63, GLES, VULKAN, SDL2|VULKAN_SCALE, SDL2|GLES_SHARP] {
        assert!(tokens(bits).is_err(), "{bits:#x}");
    }
    assert_eq!(tokens(0).unwrap(), "");
    assert_eq!(tokens(SDL2|GLES|GLES_SCALE|VULKAN|VULKAN_SCALE|VULKAN_LINEAR).unwrap(),
        "sdl2,gles,gles-scale,vulkan,vulkan-scale,vulkan-linear");
}
