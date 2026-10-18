//! Ticket 44 CLI acceptance: drive the real `rudroid-unity-loader`
//! executable end to end against deployed fixture capabilities.
//!
//! The probe capability (deployed as `libandroid_base.so`) reads
//! `device.displayWidth` through the Host API v0 inside its init phase, so
//! its log line proves the CLI installed the Host Config before the plugin
//! phases ran. Guest SOs come from the elf_load synthetic fixture — no real
//! game data is used anywhere here.

use std::path::{Path, PathBuf};
use std::process::Command;

use rudroid_test_support::cargo::{build_loader_fixture, ensure_workspace_cdylib};

const BIN: &str = env!("CARGO_BIN_EXE_rudroid-unity-loader");

#[test]
fn graphics_feature_query_needs_no_display_config_or_guest_and_reports_actual_runtime() {
    let dir=scratch_dir("graphics-features");
    let module=ensure_workspace_cdylib("rudroid-linux-sdl-runtime","platform_sdl_runtime");
    std::fs::copy(module,dir.join("libplatform_sdl_runtime.so")).unwrap();
    let output=Command::new(BIN).arg(&dir).arg("--graphics-features")
        .env("SDL_VIDEODRIVER","not-a-real-driver").env("SDL_AUDIODRIVER","not-a-real-driver").output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let stdout=String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("result=started") && !stdout.contains("[SDL]"));
    let features=std::env::var("RUDROID_KIT_FEATURES").unwrap_or_default();
    assert_eq!(stdout.trim().split(',').any(|s|s=="gles"),features.contains("/sdl"));
    assert_eq!(stdout.trim().split(',').any(|s|s=="vulkan-scale"),features.contains("/vulkan"));
    let invalid=Command::new(BIN).arg(&dir).args(["--graphics-features","--config","missing.toml"]).output().unwrap();
    assert_eq!(invalid.status.code(),Some(2));
    assert!(invalid.stdout.is_empty());
}

#[test]
fn vulkan_probe_failure_never_starts_guest_or_claims_a_frame() {
    let dir=scratch_dir("vulkan-probe");
    let module=ensure_workspace_cdylib("rudroid-linux-sdl-runtime","platform_sdl_runtime");
    std::fs::copy(module,dir.join("libplatform_sdl_runtime.so")).unwrap();
    let output=Command::new(BIN).arg(&dir).arg("--probe-vulkan")
        .env("SDL_VIDEODRIVER","not-a-real-driver").output().unwrap();
    assert!(!output.status.success());
    let stdout=String::from_utf8(output.stdout).unwrap();
    let stderr=String::from_utf8(output.stderr).unwrap();
    assert!(!stdout.contains("RUDROID_VULKAN=1") && !stdout.contains("result=started"));
    assert!(stderr.contains("VULKAN-PROBE"),"{stderr}");
    for extra in [vec!["--check"],vec!["--config","missing.toml"],vec!["--graphics-features"]] {
        let output=Command::new(BIN).arg(&dir).arg("--probe-vulkan").args(extra).output().unwrap();
        assert_eq!(output.status.code(),Some(2));
        assert!(output.stdout.is_empty());
    }
}

/// Build the android_base-kind probe fixture (outside the workspace) into
/// the workspace target dir, like the dummy capability helper in lib.rs.
fn build_probe_capability() -> PathBuf {
    build_loader_fixture("probe_capability", "rudroid_probe_capability")
}

/// Build the workspace elf_load cdylib (the kit's guest mapping provider).
fn build_elf_load_capability() -> PathBuf {
    ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load")
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rudroid-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_config(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("host.toml");
    std::fs::write(&path, body).unwrap();
    path
}

struct Kit {
    dir: PathBuf,
    config: PathBuf,
}

/// Kit with the probe capability and a config whose guest gamedata directory
/// does not exist.
fn probe_kit(name: &str, config_body: &str) -> Kit {
    let dir = scratch_dir(name);
    let probe = build_probe_capability();
    std::fs::copy(&probe, dir.join("libandroid_base.so")).unwrap();
    let config = write_config(&dir, config_body);
    Kit { dir, config }
}

fn run_cli(kit: &Kit, extra_args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let output = Command::new(BIN)
        .arg(&kit.dir)
        .arg("--config")
        .arg(&kit.config)
        .args(extra_args.iter())
        .output()
        .expect("spawn rudroid-unity-loader");
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

const CONFIG_MISSING_GAMEDATA: &str = "[device]\ndisplayWidth = 1234\n[paths]\n";

#[test]
fn run_mode_requires_config() {
    let dir = scratch_dir("no-config");
    let probe = build_probe_capability();
    std::fs::copy(&probe, dir.join("libandroid_base.so")).unwrap();
    let output = Command::new(BIN).arg(&dir).output().unwrap();
    assert!(
        !output.status.success(),
        "run mode without --config must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--config"),
        "error must name the missing flag: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_mode_installs_config_and_names_missing_gamedata() {
    let missing = scratch_dir("gamedata-absent").join("absent-gamedata");
    let kit = probe_kit(
        "config-installed",
        &format!(
            "[device]\ndisplayWidth = 1234\n[paths]\ngame_files = \"{}\"\n",
            missing.display()
        ),
    );
    let (status, stdout, stderr) = run_cli(&kit, &[]);
    assert!(!status.success(), "missing gamedata must exit non-zero");
    assert!(
        stderr.contains("displayWidth=1234"),
        "plugin init must observe the installed config, got stderr: {stderr}"
    );
    // Ticket 53 reference order: prepare runs before any guest file check,
    // so a kit without a Unity profile fails at that gate first. The
    // gamedata diagnostic behind the gate is covered by
    // profile_lifecycle_fixture::cli_run_fails_when_gamedata_is_missing.
    assert!(
        stderr.contains("no Unity profile capability"),
        "error must name the missing Unity profile gate: {stderr}"
    );
    assert!(
        stdout.contains("android_base"),
        "descriptors still print: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&kit.dir);
}

#[test]
fn run_mode_names_missing_game_files_key() {
    // No [paths] section: GuestPlan cannot be built and the error names the
    // config key.
    let kit = probe_kit("missing-key", "[device]\ndisplayWidth = 1234\n");
    let (status, _, stderr) = run_cli(&kit, &[]);
    assert!(
        !status.success(),
        "missing paths.game_files must exit non-zero"
    );
    assert!(
        stderr.contains("paths.game_files"),
        "error must name the config key: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&kit.dir);
}

#[test]
fn relative_game_root_and_deploy_dir_are_resolved_before_native_init() {
    let kit = probe_kit("relative-game-root", "[device]\ndisplayWidth=1234\n[paths]\ngame_files='gamedata'\n");
    std::fs::create_dir(kit.dir.join("gamedata")).unwrap();
    let root = kit.dir.join("gamedata").canonicalize().unwrap();
    let output = Command::new(BIN).current_dir(&kit.dir)
        .args([".", "--config", "host.toml"])
        .env("RUDROID_EXPECT_GAME_CWD", &root).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success()); // This probe kit deliberately has no Unity profile.
    assert!(stderr.contains("game cwd/config root verified by native init"), "{stderr}");
    assert!(stderr.contains("no Unity profile capability"), "{stderr}");
    std::fs::remove_dir_all(kit.dir).unwrap();
}

#[test]
fn check_mode_never_runs_guest_or_phases() {
    let kit = probe_kit(
        "check-mode",
        &format!("{CONFIG_MISSING_GAMEDATA}game_files = \"/definitely-missing-gamedata\"\n"),
    );
    let (status, stdout, stderr) = run_cli(&kit, &["--check"]);
    assert!(status.success(), "--check validates descriptors: {stderr}");
    assert!(
        stdout.contains("result=check-only") && stdout.contains("guest=not-started"),
        "check banner must separate it from a game start: {stdout}"
    );
    assert!(
        !stderr.contains("displayWidth"),
        "phases must not run in check mode: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&kit.dir);
}

#[test]
fn rudroid_skip_guest_is_explicit_and_never_a_start() {
    let kit = probe_kit(
        "skip-guest",
        &format!("{CONFIG_MISSING_GAMEDATA}game_files = \"/definitely-missing-gamedata\"\n"),
    );
    let output = Command::new(BIN)
        .arg(&kit.dir)
        .arg("--config")
        .arg(&kit.config)
        .env("RUDROID_SKIP_GUEST", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "skip smoke succeeds: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("result=skipped") && stdout.contains("NOT a game start"),
        "skip must be explicit, never a start claim: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&kit.dir);
}

#[test]
fn run_mode_requires_the_profile_before_mapping_guest_modules() {
    // Ticket 53 reference order (reference main.cpp: prepare_unity_runtime
    // precedes every guest load): a kit without a Unity profile fails at
    // the prepare gate, BEFORE any guest module is mapped.
    let dir = scratch_dir("guest-map");
    let probe = build_probe_capability();
    let elf_load = build_elf_load_capability();
    std::fs::copy(&probe, dir.join("libandroid_base.so")).unwrap();
    std::fs::copy(&elf_load, dir.join("libandroid_elf_load.so")).unwrap();

    // Synthetic AArch64 guest modules (fixture data, not real game files):
    // they must stay unmapped.
    let so = android_elf_load::fixture::build_minimal_aarch64_so();
    let lib_dir = dir.join("gamedata/lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    std::fs::write(lib_dir.join("libc++_shared.so"), &so).unwrap();
    std::fs::write(lib_dir.join("libmain.so"), &so).unwrap();
    std::fs::write(lib_dir.join("libil2cpp.so"), &so).unwrap();

    let config = write_config(
        &dir,
        &format!(
            "[device]\ndisplayWidth = 1234\n[paths]\ngame_files = \"{}\"\n",
            dir.join("gamedata").display()
        ),
    );
    let kit = Kit { dir, config };
    let (status, stdout, stderr) = run_cli(&kit, &[]);

    // Phases ran with the installed config...
    assert!(
        stderr.contains("displayWidth=1234"),
        "config must reach plugin phases: {stderr}"
    );
    // ...and the kit cannot start: no Unity profile capability.
    assert!(!status.success(), "a kit without a profile cannot start");
    assert!(
        stderr.contains("no Unity profile capability"),
        "the not-startable reason must name the profile gate: {stderr}"
    );
    // Nothing was mapped and nothing started.
    assert!(
        !stdout.contains("Main") && !stdout.contains("Il2cpp"),
        "the profile gate precedes guest mapping (reference order): {stdout}"
    );
    let _ = std::fs::remove_dir_all(&kit.dir);
}

/// Ticket 65 acceptance: when a guest module fails to map, the CLI must
/// surface the capability's specific reason (missing symbol with its name,
/// unknown relocation with offset/kind, parse diagnostics) instead of a
/// generic "reported a load failure".
#[test]
fn cli_reports_specific_elf_diagnostics() {
    // Fixture surgery constants (see android_elf_load::fixture layout).
    const DYNSYM: usize = 384; // 3 entries * 24
    const RELA: usize = 528; // 2 entries * 24

    let dir = scratch_dir("guest-diagnostics");
    let probe = build_probe_capability();
    let elf_load = build_elf_load_capability();
    // Ticket 53 reference order: the profile gate (prepare) precedes the
    // guest mapping, so the diagnostic kit carries the full chain —
    // session, hooks fixture and a real profile cdylib — and the ELF
    // diagnostics surface from the guest bring-up behind it.
    let jni_vm = ensure_workspace_cdylib("rudroid-android-jni-vm", "android_jni_vm");
    let profile = ensure_workspace_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    let hooks = build_loader_fixture("hooks_capability", "android_hooks_fixture");
    std::fs::copy(&probe, dir.join("libandroid_base.so")).unwrap();
    std::fs::copy(&elf_load, dir.join("libandroid_elf_load.so")).unwrap();
    std::fs::copy(&jni_vm, dir.join("libandroid_jni_vm.so")).unwrap();
    std::fs::copy(&profile, dir.join("libunity_2021_3.so")).unwrap();
    std::fs::copy(&hooks, dir.join("libandroid_hooks_fixture.so")).unwrap();
    let lib_dir = dir.join("gamedata/lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();

    let run = |name: &str, so: &[u8]| {
        std::fs::write(lib_dir.join(name), so).unwrap();
        let config = write_config(
            &dir,
            &format!(
                "[device]\ndisplayWidth = 1234\n[paths]\ngame_files = \"{}\"\n",
                dir.join("gamedata").display()
            ),
        );
        let kit = Kit {
            dir: dir.clone(),
            config,
        };
        run_cli(&kit, &[])
    };

    // 1. A required import no module provides: the error names the symbol.
    let mut so = android_elf_load::fixture::build_minimal_aarch64_so();
    so[DYNSYM + 48 + 4] = 0x12; // strong GLOBAL, still SHN_UNDEF
    let (status, _, stderr) = run("libmain.so", &so);
    assert!(
        !status.success(),
        "an unsatisfied strong import must fail the load"
    );
    assert!(
        stderr.contains("rudroid_import"),
        "the diagnostic must name the missing symbol: {stderr}"
    );
    assert!(
        stderr.contains("libmain.so"),
        "the diagnostic must name the module path: {stderr}"
    );

    // 2. An unknown relocation type: the error carries the module path, the
    // exact r_offset and the kind (ticket 68 completes the ticket-65
    // assertions; the touched entry targets fixture slot DATA+8 = 0x208).
    let mut so = android_elf_load::fixture::build_minimal_aarch64_so();
    so[RELA + 8..RELA + 16].copy_from_slice(&0xDEADu64.to_le_bytes()); // r_type 57005
    let (status, _, stderr) = run("libmain.so", &so);
    assert!(
        !status.success(),
        "an unknown relocation must fail the load"
    );
    assert!(
        stderr.contains("57005") && stderr.contains("relocation"),
        "the diagnostic must keep the relocation kind: {stderr}"
    );
    assert!(
        stderr.contains("0x208"),
        "the diagnostic must carry the exact r_offset (fixture slot DATA+8): {stderr}"
    );
    assert!(
        stderr.contains("libmain.so"),
        "the diagnostic must name the module path: {stderr}"
    );

    // 3. A malformed image: the parse diagnostic survives and names the path.
    let so = [0u8; 512];
    let (status, _, stderr) = run("libmain.so", &so);
    assert!(!status.success(), "a malformed image must fail the load");
    assert!(
        stderr.contains("magic") || stderr.contains("ELF"),
        "the parse diagnostic must survive to the CLI: {stderr}"
    );
    assert!(
        stderr.contains("libmain.so"),
        "the parse diagnostic must name the module path: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
