//! INPUT:  fixed Docker target cache and completed Rust static allocator build
//! OUTPUT:  optional standalone Wayland Vulkan WSI dependency in the same profile
//! POS:    post-build entry for the native dependency; never a deployed runtime
#![forbid(unsafe_code)]

fn main() {
    let mut args = std::env::args().skip(1);
    let target = args.next().expect("target cache");
    let profile = args.next().expect("profile");
    assert!(args.next().is_none() && matches!(profile.as_str(), "release" | "debug"));
    assert!(cfg!(all(target_os = "linux", target_arch = "aarch64")), "fixed ARM64 Docker toolchain required");
    rudroid_native_build::vulkan_wsi::build(std::path::Path::new(&target), profile == "release");
}
