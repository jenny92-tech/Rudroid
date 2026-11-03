//! INPUT:  verified SDL 2.32.6 source, pinned Vulkan loader and existing GNU tools
//! OUTPUT:  optional private Wayland client libraries and their upstream notices
//! POS:    build client dependencies for KMS firmware without installing or replacing system libraries
#![forbid(unsafe_code)]

use std::{fs, path::Path, process::Command};

const CONFIGURE: &[&str] = &[
    "--enable-shared", "--disable-static", "--disable-rpath",
    "--enable-video-wayland", "--enable-wayland-shared", "--disable-libdecor",
    "--disable-video-x11", "--disable-video-kmsdrm", "--disable-video-rpi",
    "--disable-video-vivante", "--disable-video-directfb", "--disable-video-opengl",
    "--enable-video-opengles", "--enable-video-opengles2", "--disable-video-opengles1",
    "--enable-video-vulkan", "--enable-alsa", "--enable-alsa-shared",
    "--disable-pulseaudio", "--disable-jack", "--disable-pipewire", "--disable-oss",
    "--disable-sndio", "--disable-esd", "--disable-arts", "--disable-nas",
    "--disable-libsamplerate", "--disable-dbus", "--disable-ime",
    "--disable-ibus", "--disable-fcitx", "--enable-assertions=release",
];

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(status.success(), "{command:?}: {status}");
}

/// This optional bundle is distinct from the core kit and has its own ELF audit.
/// Only the fixed GNU ARM64 environment may compile it. Wayland, ALSA and the
/// Vulkan ICD continue to be provided by the chosen target runtime/firmware.
pub fn build(target: &Path, release: bool) {
    let inputs = target.join("native-inputs/portable-wayland");
    assert!(inputs.join("sources.json").is_file(), "prepare verified portable Wayland inputs first");
    let profile = if release { "release" } else { "debug" };
    let out = target.join("native-objects/portable-wayland/sdl-2.32.6").join(profile);
    fs::create_dir_all(&out).expect("SDL native output");
    let cflags = if release { "-O2 -fPIC -DNDEBUG" } else { "-O0 -g -fPIC" };
    let stamp = format!("{CONFIGURE:?}\n{cflags}\n");
    let changed = fs::read_to_string(out.join("configure.id")).ok().as_deref() != Some(stamp.as_str());
    if changed || !out.join("Makefile").is_file() {
        run(Command::new("bash").arg(inputs.join("sdl-2.32.6/configure")).args(CONFIGURE)
            .arg("--prefix=/unused-rudroid-private-client")
            .current_dir(&out).env("CC", "gcc").env("CXX", "g++").env("AR", "ar")
            .env("RANLIB", "ranlib").env("CFLAGS", cflags).env("CXXFLAGS", cflags)
            .env("CPPFLAGS", "").env("LDFLAGS", "").env("PKG_CONFIG", "pkg-config"));
    }
    let config = fs::read_to_string(out.join("include/SDL_config.h")).expect("SDL configuration");
    for required in ["SDL_VIDEO_DRIVER_WAYLAND", "SDL_VIDEO_VULKAN", "SDL_AUDIO_DRIVER_ALSA", "SDL_JOYSTICK_LINUX"] {
        assert!(config.lines().any(|line| line.trim() == format!("#define {required} 1")),
            "fixed image could not enable required SDL feature {required}");
    }
    let mut make = Command::new("make");
    make.arg("-j2").current_dir(&out);
    if changed && out.join("configure.id").exists() { make.arg("-B"); }
    run(&mut make);
    fs::write(out.join("configure.id"), stamp).expect("SDL compile identity");
    let dest = target.join(profile).join("portable-wayland");
    fs::create_dir_all(&dest).expect("private client bundle");
    for (from, to) in [
        (out.join("build/.libs/libSDL2-2.0.so.0"), "libSDL2-2.0.so.0"),
        (inputs.join("loader/libvulkan.so.1"), "libvulkan.so.1"),
        (inputs.join("sdl-2.32.6/LICENSE.txt"), "LICENSE-SDL.txt"),
        (inputs.join("loader/LICENSE-Vulkan-Loader.txt"), "LICENSE-Vulkan-Loader.txt"),
        (target.join("native-inputs/vulkan-wsi/headers/LICENSES/Apache-2.0.txt"), "LICENSE-Apache-2.0.txt"),
        (inputs.join("sources.json"), "sources.json"),
    ] {
        fs::copy(&from, dest.join(to)).unwrap_or_else(|error| panic!("{}: {error}", from.display()));
    }
}
