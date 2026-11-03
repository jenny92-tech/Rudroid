//! Ticket 56 acceptance: the Android Addons' real-load chains through the
//! deployed kit.
//!
//! - The loader's guest plan maps the addon guest libraries (libunity, burst,
//!   helpers, xlua, fmod, fmodstudio) through the android_elf_load provider
//!   and fires the remaining `ModuleKind` events through the ticket-45 C ABI
//!   (with the reference plugin gating: burst/helpers/xlua/fmod only after
//!   Unity; fmodstudio only after fmod with a usable JNI_OnLoad).
//! - The deployed addon cdylibs subscribe through the Host API, log their
//!   honest mapping outcomes, and stay idempotent under repeated events.
//! - GMS/Play Core/FMOD join the SHARED JNI session (ticket 52) and their
//!   org/fmod + GMS + Play Core surfaces answer through it; the gcc guest
//!   probe re-verifies the GMS/Play Core surface from INSIDE the guest.
//! - The FMOD SetOutput detour (ticket-51 arm64) re-orders the probe's
//!   outputs through the oracle try-order; the registered audio policy
//!   (ticket 45 registry) closes the host-audio-policy side of the chain in
//!   the same kit.
//!
//! gcc fixtures are compiled by the tests themselves (rudroid53/52 convention,
//! [`Probe`]): the addon probes from each crate's `tests/fixtures/` (fmod with
//! `-DRUDROID56_FMOD_AUDIOTRACK_FAILS`, `-DRUDROID56_FMOD_NO_SETOUTPUT` and
//! `-DRUDROID56_FMOD_ALL_FAIL`), the GMS guest and `tests/fixtures/rudroid56_minimal.c`;
//! the probe tests run on Linux ARM64 only. Self-compiled fixtures only —
//! no real Unity/FMOD/vendor libraries and no handheld are involved (see the
//! ticket-56 manifest for the boundary).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rudroid_test_support::cargo::{
    build_fixture, build_loader_fixture, compile_c_fixture, ensure_workspace_cdylib,
};
use rudroid_unity_loader::{bring_up, guest, load_deploy_dir, runtime, Selection};

/// Serializes every test in this binary: the runtime registry and the JNI
/// session are process-global.
static SERIAL: Mutex<()> = Mutex::new(());

const JNI_VERSION_1_6: i32 = 0x0001_0006;

fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    ensure_workspace_cdylib(package, artifact)
}

/// Build the fixture catalog provider (lives outside the workspace, own
/// manifest, built into the workspace target so it shares the kit id).
fn build_catalog_fixture() -> PathBuf {
    build_fixture(
        "project/unity_loader/tests/fixtures/sdk_catalog_fixture/Cargo.toml",
        "sdk_fixture_catalog",
    )
}

/// The ticket-56 guest probes, compiled here with the container's C
/// compiler (`-nostdlib`, the rudroid52/53 convention) — the tests never
/// depend on prebuilt files passed through the environment.
#[derive(Clone, Copy)]
enum Probe {
    Minimal,
    Burst,
    Helpers,
    Xlua,
    /// The AUDIOTRACK candidate fails, forcing the oracle try-order.
    Fmod,
    /// No `FMOD5_System_SetOutput` export (missing-symbol path).
    FmodNoSetOutput,
    /// Every candidate fails (forced NOSOUND fallback).
    FmodAllFail,
    GmsGuest,
}

fn gcc_probe(probe: Probe) -> PathBuf {
    let (source, flags, name): (&str, &[&str], &str) = match probe {
        Probe::Minimal => ("project/unity_loader/tests/fixtures/rudroid56_minimal.c", &[], "rudroid56_minimal.so"),
        Probe::Burst => ("capability/android/addon/unity_burst/tests/fixtures/rudroid56_burst_probe.c", &[], "rudroid56_burst.so"),
        Probe::Helpers => ("capability/android/addon/unity_helpers/tests/fixtures/rudroid56_helpers_probe.c", &[], "rudroid56_helpers.so"),
        Probe::Xlua => ("capability/android/addon/xlua/tests/fixtures/rudroid56_xlua_probe.c", &[], "rudroid56_xlua.so"),
        Probe::Fmod => (FMOD_PROBE, &["-DRUDROID56_FMOD_AUDIOTRACK_FAILS"], "rudroid56_fmod_audiotrack_fails.so"),
        Probe::FmodNoSetOutput => (FMOD_PROBE, &["-DRUDROID56_FMOD_NO_SETOUTPUT"], "rudroid56_fmod_no_setoutput.so"),
        Probe::FmodAllFail => (FMOD_PROBE, &["-DRUDROID56_FMOD_ALL_FAIL"], "rudroid56_fmod_all_fail.so"),
        Probe::GmsGuest => ("capability/android/addon/gms/tests/fixtures/rudroid56_gms_guest.c", &[], "rudroid56_gms_guest.so"),
    };
    compile_c_fixture(source, flags, name)
}

const FMOD_PROBE: &str = "capability/android/addon/fmod/tests/fixtures/rudroid56_fmod_probe.c";

/// One process-global log sink shared by every test in this binary (the sink
/// installs once and ignores later installs).
fn captured_log() -> &'static Arc<Mutex<Vec<String>>> {
    static LOG: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
    LOG.get_or_init(|| {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let handle = captured.clone();
        rudroid_unity_loader::host_api::install_log_sink(Box::new(move |tag, message| {
            handle.lock().unwrap().push(format!("[{tag}] {message}"));
        }));
        captured
    })
}

/// The config getters install ONCE per process (the CLI boots one config per
/// process; `install_config` is first-write-wins). Every test therefore shares
/// ONE game_files path and re-stages its gamedata before bring-up.
fn shared_game_files() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rudroid56-shared-gamedata-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib_dir = dir.join("lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    dir
}

/// Stage a minimal image at `lib/arm64-v8a/<name>` in the shared gamedata.
fn stage(shared: &Path, name: &str, from: &Path) {
    std::fs::copy(from, shared.join("lib/arm64-v8a").join(name)).unwrap();
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rudroid56-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The core kit: android_base + ELF provider + JNI session + the fixture
/// catalog + all six addon cdylibs. Returns the deploy dir.
fn deploy_core_kit(name: &str) -> PathBuf {
    let kit_dir = temp_dir(name);
    let put = |artifact: &Path, deployed_name: &str| {
        std::fs::copy(artifact, kit_dir.join(deployed_name)).unwrap();
    };
    put(&ensure_cdylib("rudroid-android-framework", "android_base"), "libandroid_base.so");
    put(&ensure_cdylib("rudroid-android-elf-load", "android_elf_load"), "libandroid_elf_load.so");
    put(&ensure_cdylib("rudroid-android-jni-vm", "android_jni_vm"), "libandroid_jni_vm.so");
    put(&build_catalog_fixture(), "libsdk_fixture_catalog.so");
    put(&ensure_cdylib("rudroid-android-addon-unity-burst", "sdk_unity_burst"), "libsdk_unity_burst.so");
    put(&ensure_cdylib("rudroid-android-addon-unity-helpers", "sdk_unity_helpers"), "libsdk_unity_helpers.so");
    put(&ensure_cdylib("rudroid-android-addon-xlua", "sdk_xlua"), "libsdk_xlua.so");
    put(&ensure_cdylib("rudroid-android-addon-fmod", "sdk_fmod"), "libsdk_fmod.so");
    put(&ensure_cdylib("rudroid-android-addon-gms", "sdk_google_gms"), "libsdk_google_gms.so");
    put(&ensure_cdylib("rudroid-android-addon-play-core", "sdk_google_play_core"), "libsdk_google_play_core.so");
    kit_dir
}

/// Gamedata with the full reference guest set: libmain/libil2cpp (minimal),
/// libunity, and the four addon guest libraries (the fmod variant probe for
/// libfmod). Restages the SHARED gamedata dir; returns its path.
fn gamedata_full() -> PathBuf {
    let game_files = shared_game_files();
    let minimal = gcc_probe(Probe::Minimal);
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        stage(&game_files, name, &minimal);
    }
    stage(&game_files, "lib_burst_generated.so", &gcc_probe(Probe::Burst));
    stage(&game_files, "libUnityHelpers_Android.so", &gcc_probe(Probe::Helpers));
    stage(&game_files, "libxlua.so", &gcc_probe(Probe::Xlua));
    stage(&game_files, "libfmod.so", &gcc_probe(Probe::Fmod));
    stage(&game_files, "libfmodstudio.so", &gcc_probe(Probe::Fmod));
    game_files
}

fn guest_plan(game_files: &Path) -> guest::GuestPlan {
    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = \"{}\"\n",
        game_files.display()
    ))
    .unwrap();
    guest::GuestPlan::from_config(&config).expect("plan")
}

/// The Host Config the plugin getters observe (the CLI installs one before
/// the phases; the fixture tests do the same here).
fn kit_config(game_files: &Path) -> rudroid_host_config::HostConfig {
    rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = \"{}\"\n",
        game_files.display()
    ))
    .unwrap()
}

/// Bring the whole chain up: capability phases + guest modules. The config
/// install is first-write-wins per process, so every call passes the same
/// shared gamedata path (see [`shared_game_files`]).
fn bring_up_all(deploy: &Path, game_files: &Path) -> Vec<guest::ModuleEvent> {
    runtime::shutdown();
    rudroid_unity_loader::host_api::install_config(Some(kit_config(game_files))).unwrap();
    let capabilities = unsafe { load_deploy_dir(deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("capability bring-up");
    std::mem::forget(capabilities); // handles pinned for process life
    unsafe { guest::bring_up(
        // SAFETY mirror of the other fixture tests: the loader never drops
        // live kit members; the Vec is recreated from the same deploy dir.
        &load_deploy_dir(deploy, &Selection::All).expect("reload kit"),
        &guest_plan(game_files),
    ) }
    .expect("guest bring-up")
}

fn event_base(events: &[guest::ModuleEvent], kind: rudroid_common::ModuleKind) -> Option<usize> {
    events
        .iter()
        .find(|event| event.kind == kind)
        .map(|event| event.base)
}

/// Resolve + call a `fn() -> i32` export on the mapped module with `base`
/// (the probe-evidence getters; the ticket-53 exec table backs this).
fn call_counter(base: usize, name: &str) -> i32 {
    let address = runtime::guest_exec_symbol(base, name).expect(&format!("export {name} unresolved"));
    let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { function() }
}

/// Resolve + call `fn(*mut c_void, i32) -> i32` on the mapped module (the
/// probe's FMOD5_System_SetOutput stand-in).
fn call_set_output(base: usize, name: &str, output_type: i32) -> i32 {
    let address = runtime::guest_exec_symbol(base, name).expect(&format!("export {name} unresolved"));
    let function: unsafe extern "C" fn(*mut c_void, i32) -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { function(std::ptr::null_mut(), output_type) }
}

/// Resolve + call `fn(i32) -> i32` (the probe's indexed try-record getters).
fn call_record(base: usize, name: &str, index: i32) -> i32 {
    let address = runtime::guest_exec_symbol(base, name).expect(&format!("export {name} unresolved"));
    let function: unsafe extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { function(index) }
}

fn log_contains(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|line| line.contains(needle))
}

/// Snapshot of the shared sink's length (tests assert only the lines they
/// produced — earlier tests in this binary stay visible in the sink).
fn log_marker() -> usize {
    captured_log().lock().unwrap().len()
}

fn log_slice(marker: usize) -> Vec<String> {
    captured_log().lock().unwrap()[marker..].to_vec()
}

// ---------------------------------------------------------------- tests

/// The full chain: Unity fires, every addon library maps through the ELF
/// provider, the probes execute and record their calls, the FMOD detour
/// re-orders SetOutput through the oracle try-order, GMS/Play Core/FMOD
/// register through the shared session, and the audio policy stands
/// registered on the same registry.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn addon_guest_libraries_map_and_execute_through_the_provider() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _log = captured_log();
    let marker = log_marker();

    let kit = deploy_core_kit("full");
    // The audio policy provider joins THIS kit so the FMOD chain and the
    // host audio policy share one registry (the 55 fixture, reused).
    let audio_provider = build_loader_fixture("audio_policy_capability", "platform_audio_sdl");
    std::fs::copy(
        &audio_provider,
        kit.join("libplatform_audio_sdl.so"),
    )
    .unwrap();

    let game_files = gamedata_full();
    let events = bring_up_all(&kit, &game_files);

    // Order + identity: the reference sequence with the addon kinds.
    let kinds: Vec<rudroid_common::ModuleKind> = events.iter().map(|e| e.kind).collect();
    let position = |kind| kinds.iter().position(|k| *k == kind).expect("kind must have fired");
    assert!(position(rudroid_common::ModuleKind::Main) < position(rudroid_common::ModuleKind::Il2cpp));
    assert!(position(rudroid_common::ModuleKind::Il2cpp) < position(rudroid_common::ModuleKind::Unity));
    for kind in [
        rudroid_common::ModuleKind::Burst,
        rudroid_common::ModuleKind::UnityHelpers,
        rudroid_common::ModuleKind::Xlua,
        rudroid_common::ModuleKind::Fmod,
        rudroid_common::ModuleKind::FmodStudio,
    ] {
        assert!(position(kind) > position(rudroid_common::ModuleKind::Unity), "{kind:?} after Unity");
        assert!(event_base(&events, kind).unwrap() != 0, "{kind:?} carries a real load base");
    }

    // The probes executed as guest code: JNI_OnLoad ran once per library and
    // the loader recorded the version (ticket-52 contract).
    for kind in [
        rudroid_common::ModuleKind::Burst,
        rudroid_common::ModuleKind::UnityHelpers,
        rudroid_common::ModuleKind::Xlua,
    ] {
        let base = event_base(&events, kind).unwrap();
        let getter = match kind {
            rudroid_common::ModuleKind::Burst => "rudroid56_burst_probe_jni_calls",
            rudroid_common::ModuleKind::UnityHelpers => "rudroid56_helpers_probe_jni_calls",
            _ => "rudroid56_xlua_probe_jni_calls",
        };
        assert_eq!(
            call_counter(base, getter),
            1,
            "{kind:?} probe must have executed exactly once"
        );
    }
    let fmod_base = event_base(&events, rudroid_common::ModuleKind::Fmod).unwrap();
    let fmod_event = events
        .iter()
        .find(|e| e.kind == rudroid_common::ModuleKind::Fmod)
        .unwrap();
    assert_eq!(
        fmod_event.jni_on_load,
        Some(JNI_VERSION_1_6),
        "the fmod probe's JNI_OnLoad ran against the shared session"
    );

    // FMOD detour proof: the probe variant FAILS the AUDIOTRACK candidate,
    // so only a LIVE hook produces the oracle try-order (15 -> 18, then
    // NOSOUND 2 -> 0, return 0). An un-hooked direct call would return 18.
    let result = call_set_output(fmod_base, "FMOD5_System_SetOutput", 15);
    assert_eq!(result, 0, "the hook must settle the output (try-order)");
    assert_eq!(call_counter(fmod_base, "rudroid56_fmod_probe_try_count"), 2);
    assert_eq!(call_record(fmod_base, "rudroid56_fmod_probe_try_type", 0), 15, "first candidate");
    assert_eq!(call_record(fmod_base, "rudroid56_fmod_probe_try_result", 0), 18, "AUDIOTRACK fails");
    assert_eq!(call_record(fmod_base, "rudroid56_fmod_probe_try_type", 1), 2, "then NOSOUND");
    assert_eq!(call_record(fmod_base, "rudroid56_fmod_probe_try_result", 1), 0);
    assert_eq!(
        call_counter(fmod_base, "rudroid56_fmod_probe_selected_output"),
        2,
        "the fake FMOD system ended on NOSOUND"
    );
    let _ = fmod_base;

    // Addon logs: honest outcomes per crate.
    let lines = log_slice(marker);
    assert!(log_contains(&lines, "[BURST] module loaded through the ELF provider"));
    assert!(log_contains(&lines, "[UNITYHELP] module loaded through the ELF provider"));
    assert!(log_contains(&lines, "[XLUA] module loaded through the ELF provider"));
    assert!(log_contains(&lines, "[FMOD] hooked FMOD5_System_SetOutput"));
    assert!(log_contains(&lines, "[FMOD] FMOD Java/AudioTrack JNI STUB set registered"));
    assert!(log_contains(&lines, "[FMOD] studio library loaded through the ELF provider"));
    assert!(log_contains(&lines, "[GMS] Google GMS JNI/proxy plugin registered"));
    assert!(log_contains(&lines, "[PLAYCORE] Google Play Core JNI/proxy plugin registered"));
    // The host audio policy is registered in the SAME kit registry (the
    // ticket-45/55 side of the FMOD output chain).
    assert!(log_contains(&lines, "policy registered: id=sdl-adaptive"));
    assert!(runtime::registered_audio_policy().is_some());
}

/// Repeated module events must not re-initialize (oracle attempted_ parity).
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn repeated_module_events_are_idempotent() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Install the shared sink (assertions use log_slice(marker)).
    captured_log();
    let marker = log_marker();
    let kit = deploy_core_kit("idempotent");
    let game_files = gamedata_full();
    let events = bring_up_all(&kit, &game_files);
    let burst_base = event_base(&events, rudroid_common::ModuleKind::Burst).unwrap();
    let unity_base = event_base(&events, rudroid_common::ModuleKind::Unity).unwrap();
    assert_eq!(call_counter(burst_base, "rudroid56_burst_probe_jni_calls"), 1);

    // Replay Unity + Burst through the C ABI: the addon handlers must ignore
    // the repeats and the probe must not execute again.
    let unity_name = rudroid_common::descriptor::leak_cstr("lib/arm64-v8a/libunity.so");
    runtime::fire_module_event(&rudroid_common::registry::RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Unity as u32,
        base: unity_base,
        name: rudroid_common::registry::HostStr::new(unity_name),
    });
    let burst_name = rudroid_common::descriptor::leak_cstr("lib/arm64-v8a/lib_burst_generated.so");
    runtime::fire_module_event(&rudroid_common::registry::RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Burst as u32,
        base: burst_base,
        name: rudroid_common::registry::HostStr::new(burst_name),
    });
    assert_eq!(
        call_counter(burst_base, "rudroid56_burst_probe_jni_calls"),
        1,
        "the repeated events must not execute the guest again"
    );
    let lines = log_slice(marker);
    assert!(log_contains(&lines, "already attempted (idempotent)"));
    assert!(log_contains(&lines, "already initialized (idempotent"));
}

/// Missing guest libraries: the loader logs the skip, the addons log their
/// honest missing-file state, and FMOD keeps the explicit stub posture with
/// the org/fmod Java surface still registered (oracle register_jni parity).
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn missing_guest_libraries_log_and_skip() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Install the shared sink (assertions use log_slice(marker)).
    captured_log();
    let marker = log_marker();
    let kit = deploy_core_kit("missing");
    // Restage the shared gamedata: the reference core set only — NO addon
    // guest libraries at all.
    let game_files = shared_game_files();
    let minimal = gcc_probe(Probe::Minimal);
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        stage(&game_files, name, &minimal);
    }
    let events = bring_up_all(&kit, &game_files);
    assert!(event_base(&events, rudroid_common::ModuleKind::Unity).is_some());
    for kind in [
        rudroid_common::ModuleKind::Burst,
        rudroid_common::ModuleKind::UnityHelpers,
        rudroid_common::ModuleKind::Xlua,
        rudroid_common::ModuleKind::Fmod,
    ] {
        assert!(event_base(&events, kind).is_none(), "{kind:?} must not fire without its file");
    }
    let lines = log_slice(marker);
    assert!(
        log_contains(&lines, "[BURST] guest module is missing: lib/arm64-v8a/lib_burst_generated.so"),
        "burst must log the honest skip: {lines:?}"
    );
    assert!(log_contains(&lines, "[XLUA] guest module is missing"));
    assert!(log_contains(&lines, "[FMOD] guest FMOD libraries are missing"));
    assert!(
        log_contains(&lines, "[FMOD] FMOD Java/AudioTrack JNI STUB set registered"),
        "the org/fmod stub surface registers regardless of guest files (oracle parity)"
    );
    assert!(!log_contains(&lines, "hooked FMOD5_System_SetOutput"));
}

/// The reference plugin gating: without libfmod, libfmodstudio is never
/// attempted ("skipping libfmodstudio after JNI_OnLoad failure" parity —
/// here the gate is the missing fmod module event).
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn fmodstudio_is_gated_on_the_fmod_module() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Install the shared sink (assertions use log_slice(marker)).
    captured_log();
    let marker = log_marker();
    let kit = deploy_core_kit("gated");
    // Restage the shared gamedata: the core set plus libfmodstudio.so
    // WITHOUT libfmod.so.
    let game_files = shared_game_files();
    let minimal = gcc_probe(Probe::Minimal);
    for name in ["libmain.so", "libil2cpp.so", "libunity.so", "libfmodstudio.so"] {
        stage(&game_files, name, &minimal);
    }
    let events = bring_up_all(&kit, &game_files);
    assert!(event_base(&events, rudroid_common::ModuleKind::Fmod).is_none());
    assert!(event_base(&events, rudroid_common::ModuleKind::FmodStudio).is_none());
    // The gate skip is logged on the loader's stderr (the [GUEST] channel —
    // visible in the captured --nocapture run) and observable here through
    // the absence of both events and any studio addon line.
    let lines = log_slice(marker);
    assert!(
        !log_contains(&lines, "studio library loaded"),
        "a gated studio library must never be reported loaded: {lines:?}"
    );
    assert!(
        !log_contains(&lines, "[FMOD] hooked"),
        "no hook can install without the fmod module: {lines:?}"
    );
}

/// A present-but-corrupt guest library fails bring-up explicitly through the
/// ELF provider's diagnostics (never a silent skip, never a fake Mapped).
#[test]
fn corrupt_guest_library_fails_bring_up_explicitly() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let kit = deploy_core_kit("corrupt");
    // Restage the shared gamedata with a CORRUPT burst library (no gcc
    // probe needed: the provider fails the map before any code runs).
    let game_files = shared_game_files();
    let minimal = android_elf_load::fixture::build_minimal_aarch64_so();
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        std::fs::write(
            game_files.join("lib/arm64-v8a").join(name),
            &minimal,
        )
        .unwrap();
    }
    std::fs::write(
        game_files.join("lib/arm64-v8a/lib_burst_generated.so"),
        b"definitely not an ELF image",
    )
    .unwrap();

    runtime::shutdown();
    // Install the config BEFORE any guest module event: this test fires the
    // Unity event (libunity.so is staged), and the addons' handlers read
    // `paths.game_files` through the Host API getter. install_config is
    // first-write-wins per process — a getter call before the first install
    // would pin the process-global CONFIG to None and break every later
    // config-dependent assertion in parallel runs (observed as a flake).
    rudroid_unity_loader::host_api::install_config(Some(kit_config(&game_files))).unwrap();
    let capabilities = unsafe { load_deploy_dir(&kit, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("capability bring-up");
    std::mem::forget(capabilities);
    let error = unsafe { guest::bring_up(
        &load_deploy_dir(&kit, &Selection::All).expect("reload kit"),
        &guest_plan(&game_files),
    ) }
    .expect_err("a corrupt guest library must fail bring-up");
    let message = error.to_string();
    assert!(
        message.contains("lib_burst_generated.so"),
        "the diagnostic must name the corrupt file: {message}"
    );
}

/// A probe that loads but exports no SetOutput: the addon must log the
/// oracle WARNING and never claim a hook (missing-symbol explicit failure).
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn missing_set_output_symbol_fails_the_hook_explicitly() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Install the shared sink (assertions use log_slice(marker)).
    captured_log();
    let marker = log_marker();
    let kit = deploy_core_kit("nosetoutput");
    // Restage the shared gamedata: core set plus the no-SetOutput probe.
    let game_files = shared_game_files();
    let minimal = gcc_probe(Probe::Minimal);
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        stage(&game_files, name, &minimal);
    }
    stage(&game_files, "libfmod.so", &gcc_probe(Probe::FmodNoSetOutput));
    let events = bring_up_all(&kit, &game_files);
    let fmod_event = events
        .iter()
        .find(|e| e.kind == rudroid_common::ModuleKind::Fmod)
        .expect("fmod event fires (the file loads)");
    assert_eq!(fmod_event.jni_on_load, Some(JNI_VERSION_1_6));
    let lines = log_slice(marker);
    assert!(log_contains(&lines, "[FMOD] WARNING: no SetOutput hook installed"));
    assert!(!log_contains(&lines, "[FMOD] hooked"));
}

/// The GMS/Play Core registration is visible from INSIDE the guest: the gcc
/// probe (as libmain.so) walks the shared session's GMS/Play Core surface
/// and returns JNI_VERSION_1_6 only when every step passed.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn guest_sees_the_gms_and_play_core_registration() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Install the shared sink (assertions use log_slice(marker)).
    captured_log();
    let marker = log_marker();
    let kit = deploy_core_kit("gms");
    // Restage the shared gamedata: the GMS/Play Core guest probe as libmain.
    let game_files = shared_game_files();
    stage(&game_files, "libmain.so", &gcc_probe(Probe::GmsGuest));
    stage(&game_files, "libil2cpp.so", &gcc_probe(Probe::Minimal));

    let events = bring_up_all(&kit, &game_files);
    let main_event = events.first().expect("libmain event");
    assert_eq!(main_event.kind, rudroid_common::ModuleKind::Main);
    assert_eq!(
        main_event.jni_on_load,
        Some(JNI_VERSION_1_6),
        "the guest probe must see the registered GMS/Play Core surface \
         through the shared session (step failures return 0x5600|step)"
    );
    let lines = log_slice(marker);
    assert!(log_contains(&lines, "[GMS] Google GMS JNI/proxy plugin registered"));
    assert!(log_contains(&lines, "[PLAYCORE] Google Play Core JNI/proxy plugin registered"));
}

/// The forced-NOSOUND fallback: when every candidate fails, the live hook
/// still returns FMOD OK after trying NOSOUND (oracle parity, probe record).
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn all_candidates_failing_still_settles_like_the_oracle() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Install the shared sink (assertions use log_slice(marker)).
    captured_log();
    let marker = log_marker();
    let kit = deploy_core_kit("allfail");
    // Restage the shared gamedata: core set plus the all-fail fmod probe.
    let game_files = shared_game_files();
    let minimal = gcc_probe(Probe::Minimal);
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        stage(&game_files, name, &minimal);
    }
    stage(&game_files, "libfmod.so", &gcc_probe(Probe::FmodAllFail));
    let events = bring_up_all(&kit, &game_files);
    let fmod_base = event_base(&events, rudroid_common::ModuleKind::Fmod).unwrap();
    let result = call_set_output(fmod_base, "FMOD5_System_SetOutput", 15);
    assert_eq!(result, 0, "the oracle fallback returns OK after forcing NOSOUND");
    // The oracle hook does not deduplicate: 5 candidates (requested=15
    // repeats AUDIOTRACK) + the forced NOSOUND = 6 original calls, all
    // failing on this probe variant.
    assert_eq!(call_counter(fmod_base, "rudroid56_fmod_probe_try_count"), 6);
    let lines = log_slice(marker);
    assert!(log_contains(&lines, "[FMOD] SetOutput all failed (last=18); returning OK for NOSOUND"));
}
