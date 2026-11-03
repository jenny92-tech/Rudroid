//! INPUT:  shared native-build contract and typed NDK media C boundary
//! OUTPUT:  a private PIC archive linked only into android_media_fallback
//! POS:    keeps Android data symbols and native opaque handles at the C ABI boundary
fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: Some("rudroid_media"),
        sources: &["src/media.c"],
        headers: &[],
        // Typed-failure entry points keep the full NDK prototypes they ignore.
        extra_flags: &["-Wno-unused-parameter"],
        link: &[],
    });
}
