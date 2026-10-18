//! INPUT:  verified upstream WSI/Vulkan-Headers sources in the fixed native-input cache
//! OUTPUT:  optional Wayland WSI shared library with Rust allocator, private manifest and all applicable notices
//! POS:    fixes the external dependency build flags without replacing system GPU libraries
#![forbid(unsafe_code)]

use std::{fs, path::{Path, PathBuf}, process::Command};

/// Fingerprinted with the kit; preparation refuses a different vendor patch.
pub const WSI_PATCH_SHA256: &str = "10a01403c8f38c8193af29e1151b324ca1d7b7bead6b28709a5dbf2c67e98ea7";

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(status.success(), "{command:?}: {status}");
}

fn pkg_config(args: &[&str]) -> String {
    let output = Command::new("pkg-config").args(args).output().expect("existing pkg-config");
    assert!(output.status.success(), "pkg-config {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("pkg-config UTF-8 output")
}

/// Build the pinned external layer only for the supported GNU ARM64 target.
/// Inputs are prepared and checked by the Docker entry before its source freeze.
/// No compiler downloads, environment flags or system installation occur here.
pub fn build(target: &Path, release: bool) {
    let inputs = target.join("native-inputs/vulkan-wsi");
    for name in ["wsi-patched", "headers", "receipt.json"] {
        assert!(inputs.join(name).exists(), "use docker-rust.sh --vulkan to prepare {name}");
    }
    let out = target.join("native-objects/vulkan-wsi").join(if release { "release" } else { "debug" });
    fs::create_dir_all(&out).expect("WSI compiler output");
    let source = inputs.join("wsi-patched");
    run(Command::new("gcc").args(["-std=c11", "-Werror", "-fsyntax-only", "-D__user="])
        .arg("-I").arg(source.join("util/wsialloc"))
        .arg("-I").arg(inputs.join("uapi"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/vulkan_wsi_abi.c")));
    let flags = pkg_config(&["--cflags", "wayland-client"]);
    let libraries = pkg_config(&["--libs", "wayland-client"]);
    // The Wayland recipe uses DRM format constants only, no libdrm calls.
    let protocols = inputs.join("protocols");
    let mut sources: Vec<PathBuf> = [
        "layer/layer.cpp", "layer/private_data.cpp", "layer/surface_api.cpp",
        "layer/swapchain_api.cpp", "layer/swapchain_maintenance_api.cpp",
        "util/timed_semaphore.cpp", "util/custom_allocator.cpp", "util/extension_list.cpp",
        "util/log.cpp", "util/format_modifiers.cpp", "util/drm/drm_utils.cpp", "util/drm/format_table.c",
        "wsi/external_memory.cpp", "wsi/frame_boundary.cpp", "wsi/surface_properties.cpp",
        "wsi/swapchain_base.cpp", "wsi/synchronization.cpp", "wsi/wsi_factory.cpp",
        "wsi/wayland/surface_properties.cpp", "wsi/wayland/surface.cpp",
        "wsi/wayland/wl_helpers.cpp", "wsi/wayland/swapchain.cpp",
    ].iter().map(|name| source.join(name)).collect();
    for (name, directory) in [
        ("linux-dmabuf-unstable-v1", "unstable/linux-dmabuf"),
        ("linux-explicit-synchronization-unstable-v1", "unstable/linux-explicit-synchronization"),
        ("presentation-time", "stable/presentation-time"),
    ] {
        let xml = protocols.join(directory).join(format!("{name}.xml"));
        let header = out.join(format!("{name}-client-protocol.h"));
        // The upstream recipe uses -protocol.c for unstable protocols and
        // -client-protocol.c for presentation-time; include names must match.
        let generated = out.join(format!("{name}{}.c", if name == "presentation-time" { "-client-protocol" } else { "-protocol" }));
        run(Command::new("wayland-scanner").arg("client-header").arg(&xml).arg(&header));
        if name == "linux-explicit-synchronization-unstable-v1" {
            // This pinned upstream header includes the protocol spelling while
            // its CMake recipe generates the client-protocol spelling.
            fs::copy(&header, out.join(format!("{name}-protocol.h"))).expect("upstream include alias");
        }
        run(Command::new("wayland-scanner").arg("public-code").arg(xml).arg(&generated));
        sources.push(generated);
    }
    let mut objects = Vec::new();
    for (index, file) in sources.iter().enumerate() {
        let cpp = file.extension().is_some_and(|ext| ext == "cpp");
        let object = out.join(format!("{index}.o"));
        let mut compiler = Command::new(if cpp { "g++" } else { "gcc" });
        compiler.args([if cpp { "-std=c++17" } else { "-std=gnu99" }, "-fPIC", "-fvisibility=hidden",
            "-Wall", "-pthread", "-DBUILD_WSI_WAYLAND=1", "-DBUILD_WSI_HEADLESS=0", "-DBUILD_WSI_DISPLAY=0",
            "-DVULKAN_WSI_LAYER_EXPERIMENTAL=0", "-DWSI_IMAGE_COMPRESSION_CONTROL_SWAPCHAIN=0",
            "-DWSI_DISPLAY_SUPPORT_FORMAT_MODIFIERS=0", "-DENABLE_INSTRUMENTATION=0",
            "-DWAYLAND_FIFO_PRESENTATION_THREAD_ENABLED=0", "-Ulinux", "-DWSIALLOC_MEMORY_HEAP_NAME=system",
            // Userspace export of the Linux UAPI address-space annotation.
            "-D__user="]);
        compiler.arg(if release { "-O2" } else { "-O0" });
        if release { compiler.arg("-DNDEBUG"); }
        for include in [&source, &out, &inputs.join("headers/include"),
            &source.join("util/wsialloc"), &source.join("util/drm"), &source.join("util"), &inputs.join("uapi")] {
            compiler.arg("-I").arg(include);
        }
        compiler.args(flags.split_whitespace()).arg("-c").arg(file).arg("-o").arg(&object);
        run(&mut compiler);
        objects.push(object);
    }
    let allocator = target.join(if release { "release" } else { "debug" }).join("librudroid_vulkan_wsi_allocator.a");
    assert!(allocator.is_file(), "completed Rust wsialloc static archive required");
    run(Command::new("g++").args(["-shared", "-pthread", "-Wl,--no-undefined", "-Wl,--exclude-libs,ALL", "-Wl,--gc-sections"])
        .args(&objects).arg(&allocator).args(libraries.split_whitespace()).args(["-ldl", "-lm", "-lrt"])
        .arg("-o").arg(out.join("libVkLayer_window_system_integration.so")));
    // Keep an optional non-capability DSO outside the descriptor scanner's directory.
    let profile = target.join(if release { "release" } else { "debug" }).join("vulkan-wsi");
    fs::create_dir_all(&profile).expect("WSI artifact directory");
    for name in ["libVkLayer_window_system_integration.so", "VkLayer_window_system_integration.json"] {
        let from = if name.ends_with(".json") { inputs.join("layer.json") } else { out.join(name) };
        fs::copy(from, profile.join(name)).expect("WSI artifact");
    }
    fs::copy(inputs.join("receipt.json"), profile.join("sources.json")).expect("WSI provenance");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (source, destination) in [
        ("LICENSE", "LICENSE-Rudroid.txt"),
        ("NOTICE", "NOTICE-Rudroid.txt"),
        ("THIRD-PARTY-NOTICES", "THIRD-PARTY-NOTICES-Rudroid.txt"),
    ] {
        fs::copy(repository.join(source), profile.join(destination)).expect("linked allocator notices");
    }
    fs::copy(inputs.join("wsi/LICENSE"), profile.join("LICENSE-WSI.txt")).expect("WSI license");
    for (source, destination) in [
        ("headers/LICENSE.md", "LICENSE-Vulkan-Headers.md"),
        ("headers/LICENSES/Apache-2.0.txt", "LICENSE-Apache-2.0.txt"),
        ("headers/LICENSES/MIT.txt", "LICENSE-MIT.txt"),
        ("protocols/COPYING", "LICENSE-Wayland-Protocols.txt"),
    ] {
        fs::copy(inputs.join(source), profile.join(destination)).expect("external dependency license");
    }
}
