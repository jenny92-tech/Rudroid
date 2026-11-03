//! INPUT:  shared native-build contract, nine C ABI boundaries and two ARM64 assembly adapters
//! OUTPUT:  a private PIC archive linked only into android_bionic on Linux ARM64
//! POS:    compiles the small C ABI boundary without another dependency or plugin
fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: Some("rudroid_bionic_c"),
        sources: &[
            "src/stdio_variadic.c",
            "src/posix.c",
            "src/locale.c",
            "src/atfork.c",
            "src/math.c",
            "src/network.c",
            "src/logging.c",
            "src/signals.c",
            "src/jump.c",
            "src/jump.S",
            "src/ioctl.S",
        ],
        headers: &["src/symbol_table.h"],
        extra_flags: &[],
        link: &["m", "dl"],
    });
}
