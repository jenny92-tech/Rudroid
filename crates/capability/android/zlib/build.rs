//! INPUT:  shared native-build contract and the gated native zlib C boundary
//! OUTPUT:  a private PIC archive linked only into android_zlib
//! POS:    checks the LP64 stream layout before supplying native compression addresses
fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: Some("rudroid_zlib"),
        sources: &["src/zlib.c"],
        headers: &[],
        extra_flags: &[],
        link: &["z"],
    });
}
