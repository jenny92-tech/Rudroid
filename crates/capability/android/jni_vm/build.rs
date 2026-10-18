//! INPUT:  shared native-build contract and the native JNI variadic adapter
//! OUTPUT:  private PIC archive linked only into android_jni_vm
//! POS:    leaves argument promotion/va_list consumption to the platform C compiler
fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: Some("rudroid_jni_varargs"),
        // Separate archive member: referenced only by cfg(test) unit code,
        // so production binaries/libraries never pull the probe into the link.
        sources: &["src/varargs.c", "tests/fixtures/jni_scalar_probe.c"],
        headers: &["tests/fixtures/jni.h"],
        extra_flags: &[],
        link: &[],
    });
}
