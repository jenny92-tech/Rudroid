//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn every_profile_but_release_compiles_unoptimized() {
    assert_eq!(optimization("release"), "-O2");
    assert_eq!(optimization("debug"), "-O0");
    assert_eq!(optimization(""), "-O0");
}

#[test]
fn c_sources_get_the_language_standard_and_assembly_does_not() {
    let object = Path::new("/out/x.o");
    let c = compile_args("src/x.c", "release", &["-pthread"], object);
    assert_eq!(
        c,
        [
            "-std=c11", "-fPIC", "-fvisibility=hidden", "-Wall", "-Wextra", "-Werror",
            "-pthread", "-O2", "-c", "src/x.c", "-o", "/out/x.o",
        ]
    );
    let asm = compile_args("src/x.S", "debug", &[], object);
    assert_eq!(
        asm,
        ["-fPIC", "-fvisibility=hidden", "-Wall", "-Wextra", "-Werror", "-O0", "-c", "src/x.S", "-o", "/out/x.o"]
    );
}

#[test]
fn same_stem_sources_get_distinct_objects() {
    assert_eq!(object_name("src/jump.c"), "jump.c.o");
    assert_eq!(object_name("src/jump.S"), "jump.S.o");
    assert_ne!(object_name("src/jump.c"), object_name("src/jump.S"));
}

#[test]
fn cross_builds_use_the_prefixed_gnu_toolchain() {
    assert_eq!(toolchain(NATIVE_TARGET), ("gcc", "ar"));
    assert_eq!(
        toolchain("x86_64-unknown-linux-gnu"),
        ("aarch64-linux-gnu-gcc", "aarch64-linux-gnu-ar")
    );
}
