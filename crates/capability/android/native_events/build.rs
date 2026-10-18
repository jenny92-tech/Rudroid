//! INPUT:  shared native-build contract and public Linux epoll/pthread headers
//! OUTPUT:  compile-time verification of the Rust event module's native layouts
//! POS:    ABI gate only; no C event implementation or runtime archive
fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: None,
        sources: &["src/abi_gate.c"],
        headers: &[],
        extra_flags: &["-pthread"],
        link: &["pthread"],
    });
}
