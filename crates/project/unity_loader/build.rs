//! INPUT: Cargo target/profile and the kit's SDL feature selection
//! OUTPUT: Linux CLI profile-export roots and integration-test SDL cfg
//! POS: keeps production identity inspectable while preserving bounded test feature selection
//! Expose the kit's SDL selection to the integration tests as
//! `cfg(rudroid_kit_sdl)`, so the display-only tests are reported as
//! `ignored` with a reason in a no-SDL run instead of passing empty.
//! `RUDROID_KIT_FEATURES` is the kit's single feature record (docker-rust.sh,
//! build-kit.sh); test-support fails loudly when it disagrees with the
//! actual outer build. The loader binary itself never reads the cfg.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for symbol in ["rudroid_release_build_marker", "rudroid_debug_build_marker"] {
            println!("cargo:rustc-link-arg-bin=rudroid-unity-loader=-Wl,--export-dynamic-symbol={symbol}");
        }
    }
    println!("cargo:rerun-if-env-changed=RUDROID_KIT_FEATURES");
    println!("cargo::rustc-check-cfg=cfg(rudroid_kit_sdl)");
    let features = std::env::var("RUDROID_KIT_FEATURES").unwrap_or_default();
    if features
        .split(|c: char| c.is_whitespace() || c == ',' || c == '=')
        .any(|feature| feature == "rudroid-linux-sdl-runtime/sdl")
    {
        println!("cargo:rustc-cfg=rudroid_kit_sdl");
    }
}
