//! Expose the kit's SDL selection to the integration tests as
//! `cfg(rudroid_kit_sdl)`, so the display-only tests are reported as
//! `ignored` with a reason in a no-SDL run instead of passing empty.
//! `RUDROID_KIT_FEATURES` is the kit's single feature record (docker-rust.sh,
//! build-kit.sh); test-support fails loudly when it disagrees with the
//! actual outer build. The loader binary itself never reads the cfg.

fn main() {
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
