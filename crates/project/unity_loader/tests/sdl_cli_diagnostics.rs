//! Ticket 55 diagnostics through the real CLI binary: a deployed
//! platform_sdl_runtime that cannot create its session must fail the run
//! with its own explicit diagnostic, never a panic and never a silent skip.
//!
//! Each scenario spawns the loader as a child process because SDL video
//! state is process-global: the video driver environment must be read by a
//! fresh process per scenario. The three named situations are:
//! 1. no display service (no usable video driver) — SDL init fails,
//! 2. dummy video driver — refused explicitly (must never pass as GLES),
//! 3. display present but no GLES library — every context request fails.
//!
//! Runs in the SDL kit matrix (`docker-rust.sh test --sdl`: the `sdl`
//! feature build under Xvfb + Mesa); other runs report it `ignored` with the
//! reason (cfg `rudroid_kit_sdl` from build.rs). An SDL run without a display
//! fails instead of passing empty.

use std::path::{Path, PathBuf};
use std::process::Command;

use rudroid_test_support::cargo::build_loader_fixture;

const BIN: &str = env!("CARGO_BIN_EXE_rudroid-unity-loader");

fn build_sdl_kit() -> (PathBuf, PathBuf) {
    let probe = build_loader_fixture("probe_capability", "rudroid_probe_capability");
    let sdl_runtime = rudroid_test_support::cargo::ensure_workspace_cdylib(
        "rudroid-linux-sdl-runtime",
        "platform_sdl_runtime",
    );

    let dir = std::env::temp_dir().join(format!("rudroid-sdl-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&probe, dir.join("libandroid_base.so")).unwrap();
    std::fs::copy(&sdl_runtime, dir.join("libplatform_sdl_runtime.so")).unwrap();
    let config = dir.join("host.toml");
    std::fs::write(
        &config,
        "[device]\ndisplayWidth = 320\ndisplayHeight = 240\n",
    )
    .unwrap();
    (dir, config)
}

fn run_loader(
    kit: &Path,
    config: &Path,
    video_driver: &str,
    extra_env: &[(&str, &str)],
    clear_display: bool,
) -> (bool, String) {
    let mut command = Command::new(BIN);
    command.arg(kit).arg("--config").arg(config);
    command.env("SDL_VIDEODRIVER", video_driver);
    if clear_display {
        command.env_remove("DISPLAY");
    }
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let output = command.output().expect("spawn rudroid-unity-loader");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
#[cfg_attr(
    not(rudroid_kit_sdl),
    ignore = "needs the SDL kit under Xvfb: bash crates/scripts/docker-rust.sh test --sdl"
)]
fn sdl_session_failures_are_explicit_cli_diagnostics() {
    assert!(
        std::env::var_os("DISPLAY").is_some(),
        "the SDL kit tests need a display (docker-rust.sh test --sdl runs Xvfb)"
    );
    let (kit, config) = build_sdl_kit();

    // 1. No display service: the X11 driver with no X server to connect to.
    //    SDL 2.0.14 defers video init to window creation, so the explicit
    //    no-display shape is the window-creation failure naming the driver
    //    (an outright init failure reports the same way through the same
    //    channel). Either way the loader must fail with the capability's
    //    diagnostic, never silently.
    let (ok, stderr) = run_loader(&kit, &config, "x11", &[], true);
    assert!(!ok, "a kit that cannot create its session must not start");
    assert!(
        stderr.contains("cannot create its session"),
        "the loader must fail the run on the capability's diagnostic: {stderr}"
    );
    assert!(
        stderr.contains("SDL init failed") || stderr.contains("window creation failed"),
        "no-display diagnostic must carry the SDL failure: {stderr}"
    );

    // 2. Dummy video driver: SDL would happily init, but a dummy driver is
    //    never a display — refused before any GL request.
    let (ok, stderr) = run_loader(&kit, &config, "dummy", &[], false);
    assert!(!ok, "the dummy driver must not pass as a display");
    assert!(
        stderr.contains("cannot stand in for a display"),
        "dummy-driver diagnostic must be explicit: {stderr}"
    );

    // 3. Real display but broken EGL/GLES libraries. The production path
    // now uses EGL, so corrupting desktop libGL no longer exercises it.
    for (label, names, diagnostic) in [
        ("egl", ["libEGL.so.1", "libEGL.so"], "native EGL driver"),
        (
            "gles",
            ["libGLESv2.so.2", "libGLESv2.so"],
            "native GLES driver",
        ),
    ] {
        let broken = kit.join(format!("broken-{label}"));
        std::fs::create_dir_all(&broken).unwrap();
        for name in names {
            std::fs::write(broken.join(name), b"not an elf").unwrap();
        }
        let (ok, stderr) = run_loader(
            &kit,
            &config,
            "x11",
            &[("LD_LIBRARY_PATH", broken.to_str().expect("utf8 path"))],
            false,
        );
        assert!(
            !ok,
            "a display with no usable {label} cannot produce a session"
        );
        assert!(
            stderr.contains("cannot create its session"),
            "SDL failure channel: {stderr}"
        );
        assert!(
            stderr.contains(diagnostic),
            "must identify the broken driver library: {stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&kit);
}
