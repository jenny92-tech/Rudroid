//! INPUT:  fixed Docker target cache and completed Rust static allocator build
//! OUTPUT:  standalone Wayland Vulkan WSI and optional private SDL/Vulkan clients
//! POS:    post-build entry for the native dependency; never a deployed runtime
#![forbid(unsafe_code)]

fn main() {
    let mut args = std::env::args().skip(1);
    let target = args.next().expect("target cache");
    let profile = args.next().expect("profile");
    let clients = args.next();
    assert!(clients.as_deref().is_none_or(|arg| arg == "--portable-wayland")
        && args.next().is_none() && matches!(profile.as_str(), "release" | "debug"));
    assert!(cfg!(all(target_os = "linux", target_arch = "aarch64")), "fixed ARM64 Docker toolchain required");
    rudroid_native_build::vulkan_wsi::build(std::path::Path::new(&target), profile == "release");
    if clients.is_some() {
        rudroid_native_build::portable_wayland::build(std::path::Path::new(&target), profile == "release");
    }
}
