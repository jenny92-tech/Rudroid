//! INPUT:  built SDL/provider fixtures, fixed SDL kit and Xvfb/Mesa display
//! OUTPUT:  dynamic session/audio C API, native-buffer fallback and symbol-parity tests
//! POS:    verifies deployed backend ownership and geometry without linking capabilities in production
//! Ticket 55 acceptance: the host SDL session and the audio policy pump
//! through real deployed libraries.
//!
//! - The audio policy provider fixture registers the reference "sdl-adaptive"
//!   `RudroidAudioPolicy` through the Host API during its early-init phase — the
//!   ticket-45 dynamic registration model. The deployed sdl_runtime
//!   capability's audio C API then pumps synthetic PCM into a file sink under
//!   those policy values; sample vs frame accounting, empty-buffer behavior
//!   and the silent-driver rejection are asserted against the live cdylib.
//! - The session C API is exercised through the loader's dynamic-phase
//!   assembly (`runtime::bring_up_sdl_session`). Without the `sdl` feature
//!   (always run; an SDL kit builds that variant into its own target subdir)
//!   the same entry point must return the capability's explicit "not
//!   compiled in" diagnostic; with Xvfb + Mesa software GLES (SDL kit,
//!   `docker-rust.sh test --sdl`) it must report a real ES session with
//!   renderer strings, a non-background readback pixel and at least one swap.
//!
//! The registry is process-global state, so the tests serialize on a shared
//! mutex and shut the registry down when they are done with it.
//!
//! Assembly contract (tickets 73/75, the 68/71 pattern): cdylibs and fixture
//! capabilities are located the way cargo itself does — CARGO_TARGET_DIR
//! when the invocation set one (relative paths resolve against the crates
//! root), else the workspace `target` — and always rebuilt with THIS run's
//! profile before use, so one plain `cargo test` command works from an empty
//! target dir in either profile and never loads a foreign or stale artifact.

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rudroid_test_support::cargo::{
    assert_kit_features, build_loader_fixture, build_variant, ensure_workspace_cdylib, kit_sdl,
};

// The capability package's lib target is renamed `platform_sdl_runtime`
// (artifact name == capability id), so that is the importable crate name.
use platform_sdl_runtime::c_api::symbols as capability_symbols;
use rudroid_unity_loader::{bring_up, load_deploy_dir, runtime, Selection};

/// Serializes every test in this binary: the registry is process-global.
static SERIAL: Mutex<()> = Mutex::new(());

/// Build a workspace cdylib with this run's profile and the kit features
/// (`RUDROID_KIT_FEATURES`; the SDL capability gets `sdl` exactly when the
/// outer build has it).
fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    assert_kit_features(platform_sdl_runtime::SDL_ENABLED);
    ensure_workspace_cdylib(package, artifact)
}

/// Build a fixture capability (own workspace, like the dummy capability).
fn build_fixture(name: &str, artifact: &str) -> PathBuf {
    build_loader_fixture(name, artifact)
}

/// The SDL capability WITHOUT the `sdl` feature. A no-SDL kit already is
/// that build; an SDL kit builds the variant into its own target subdir so
/// the kit's artifact is never replaced (the variant keeps the kit id on
/// purpose: it must load through this run's loader).
fn sdl_runtime_without_sdl() -> PathBuf {
    if kit_sdl() {
        build_variant("no-sdl", "rudroid-linux-sdl-runtime", &[], "platform_sdl_runtime")
    } else {
        ensure_cdylib("rudroid-linux-sdl-runtime", "platform_sdl_runtime")
    }
}

fn deploy_kit(dir: &Path, artifacts: &[(&Path, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (artifact, deployed_name) in artifacts {
        std::fs::copy(artifact, dir.join(deployed_name)).unwrap();
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rudroid-sdl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Dlsym helper on a pinned capability library; a missing symbol fails with
/// an explicit message (the ticket-55 C API must exist in every build).
///
/// # Safety
/// `library` must be a valid open handle; `T` must match the C signature.
unsafe fn dlsym_required<'lib, T>(
    library: &'lib libloading::Library,
    name: &str,
) -> libloading::Symbol<'lib, T> {
    // SAFETY: dlsym on a valid handle; T is the documented call shape.
    unsafe { library.get::<T>(name.as_bytes()) }.unwrap_or_else(|err| {
        panic!("deployed capability misses C symbol `{name}`: {err}")
    })
}

/// Two-phase fetch of the deployed capability's last-error message.
///
/// # Safety
/// `last_error` points into the pinned capability library.
unsafe fn fetch_last_error(
    last_error: unsafe extern "C" fn(*mut c_char, usize) -> usize,
) -> String {
    // SAFETY: the two-phase contract; the buffer is sized by the first call.
    unsafe {
        let need = last_error(std::ptr::null_mut(), 0);
        assert_ne!(need, 0, "an error diagnostic must be pending");
        let mut buffer = vec![0u8; need];
        last_error(buffer.as_mut_ptr() as *mut c_char, need);
        buffer.truncate(need.saturating_sub(1));
        String::from_utf8(buffer).expect("diagnostics are UTF-8")
    }
}

/// The audio-policy experiment: a deployed provider registers the reference
/// policy across the C ABI; the deployed sdl_runtime capability opens a sink
/// under those values and pumps synthetic PCM. Frames, calls and file bytes
/// are asserted from the live cdylib getters.
#[test]
fn audio_policy_registers_dynamically_and_pumps_to_a_file_sink() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let log = captured_log();

    let probe = build_fixture("probe_capability", "rudroid_probe_capability");
    let audio_provider = build_fixture("audio_policy_capability", "platform_audio_sdl");
    let sdl_runtime = ensure_cdylib("rudroid-linux-sdl-runtime", "platform_sdl_runtime");

    let deploy = temp_dir("audio-kit");
    deploy_kit(
        &deploy,
        &[
            (&probe, "libandroid_base.so"),
            (&audio_provider, "libplatform_audio_sdl.so"),
            (&sdl_runtime, "libplatform_sdl_runtime.so"),
        ],
    );
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("bring-up registers the policy");
    std::mem::forget(capabilities); // handles pinned for process life

    // The policy crossed the C ABI from the provider fixture into the shared
    // registry (ticket 45 model — no process-local registry anymore).
    let policy = runtime::registered_audio_policy()
        .expect("the deployed provider's audio policy must be registered");
    assert_eq!(policy.default_open_attempts, 4);
    assert_eq!(policy.named_open_attempts, 5);
    assert_eq!(policy.retry_delay_ms, 250);
    assert_eq!(policy.enumerate_outputs, 1);
    assert_eq!(policy.prefer_non_hdmi, 1);
    assert_eq!(policy.reject_silent_drivers, 1);
    assert!(
        log.lock().unwrap().iter().any(|line| line.contains("policy registered: id=sdl-adaptive")),
        "the provider fixture must log its registration"
    );

    // SAFETY: dlopen of the workspace-built cdylib; pinned for the test.
    let sdl_library = unsafe { libloading::Library::new(&sdl_runtime) }
        .expect("reopen sdl_runtime cdylib");
    // SAFETY: the call shapes mirror the capability's documented C API.
    unsafe {
        let audio_open = dlsym_required::<
            unsafe extern "C" fn(u32, u32, u32, u32, u32, *const c_char) -> *mut c_void,
        >(&sdl_library, capability_symbols::AUDIO_OPEN);
        let audio_write = dlsym_required::<
            unsafe extern "C" fn(*mut c_void, *const f32, usize) -> usize,
        >(&sdl_library, capability_symbols::AUDIO_WRITE);
        let audio_frames = dlsym_required::<
            unsafe extern "C" fn(*const c_void) -> usize,
        >(&sdl_library, capability_symbols::AUDIO_FRAMES);
        let audio_calls = dlsym_required::<
            unsafe extern "C" fn(*const c_void) -> usize,
        >(&sdl_library, capability_symbols::AUDIO_CALLS);
        let audio_destroy = dlsym_required::<
            unsafe extern "C" fn(*mut c_void),
        >(&sdl_library, capability_symbols::AUDIO_DESTROY);

        // The policy rejects silent drivers, so the file sink is the honest
        // sink for the registered "sdl-adaptive" values.
        let sink_path = deploy.join("rudroid55-audio.raw");
        let c_path = std::ffi::CString::new(sink_path.to_str().unwrap()).unwrap();
        let pump = audio_open(48_000, 2, policy.default_open_attempts, policy.retry_delay_ms, policy.reject_silent_drivers as u32, c_path.as_ptr());
        assert!(!pump.is_null(), "file sink must open under the policy");

        // 12 interleaved stereo samples = 6 frames; a non-zero sweep so the
        // file content proves real data flowed.
        let samples: Vec<f32> = (0..12).map(|i| (i as f32) * 0.125 - 0.5).collect();
        let frames = audio_write(pump, samples.as_ptr(), samples.len());
        assert_eq!(frames, 6, "12 samples / 2 channels = 6 frames");
        assert_eq!(audio_frames(pump), 6);
        assert_eq!(audio_calls(pump), 1, "one non-empty write = one sink call");

        // Empty buffer: no frames, no sink call — the acceptance contract.
        assert_eq!(audio_write(pump, samples.as_ptr(), 0), 0);
        assert_eq!(audio_frames(pump), 6, "empty buffer must not add frames");
        assert_eq!(audio_calls(pump), 1, "empty buffer must not call the sink");

        audio_destroy(pump);
    }
    let bytes = std::fs::read(deploy.join("rudroid55-audio.raw")).expect("file sink persisted");
    assert_eq!(bytes.len(), 12 * 4, "one LE f32 per sample written");
    assert!(bytes.iter().any(|b| *b != 0), "samples must be non-zero");
    let written: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    assert!(
        written
            .iter()
            .enumerate()
            .all(|(i, s)| *s == (i as f32) * 0.125 - 0.5),
        "the file must carry exactly the synthetic PCM"
    );

    runtime::shutdown();
    let _ = std::fs::remove_dir_all(&deploy);
}

/// A partial frame (samples not divisible by channels) is rejected with an
/// explicit diagnostic and never counted — via the deployed C API.
#[test]
fn audio_partial_frame_is_rejected_through_the_c_api() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let sdl_runtime = ensure_cdylib("rudroid-linux-sdl-runtime", "platform_sdl_runtime");
    // SAFETY: dlopen of the workspace-built cdylib; pinned for the test.
    let library = unsafe { libloading::Library::new(&sdl_runtime) }.expect("reopen");
    // SAFETY: documented call shapes.
    unsafe {
        let last_error = dlsym_required::<
            unsafe extern "C" fn(*mut c_char, usize) -> usize,
        >(&library, capability_symbols::LAST_ERROR);
        let audio_open = dlsym_required::<
            unsafe extern "C" fn(u32, u32, u32, u32, u32, *const c_char) -> *mut c_void,
        >(&library, capability_symbols::AUDIO_OPEN);
        let audio_write = dlsym_required::<
            unsafe extern "C" fn(*mut c_void, *const f32, usize) -> usize,
        >(&library, capability_symbols::AUDIO_WRITE);
        let audio_frames = dlsym_required::<
            unsafe extern "C" fn(*const c_void) -> usize,
        >(&library, capability_symbols::AUDIO_FRAMES);

        // Dummy sink (no path, silent drivers allowed): the counting sink.
        let pump = audio_open(48_000, 2, 4, 250, 0, std::ptr::null());
        assert!(!pump.is_null(), "dummy sink opens when the policy allows it");
        let samples = [0.25f32; 3];
        assert_eq!(audio_write(pump, samples.as_ptr(), 3), 0, "3 % 2 -> rejected");
        let message = fetch_last_error(*last_error);
        assert!(
            message.contains("not a whole number of frames"),
            "explicit sample-vs-frame diagnostic: {message}"
        );
        assert_eq!(audio_frames(pump), 0, "a rejected write counts nothing");
    }
}

/// Loader-side dynamic-phase diagnostic: the capability is deployed but
/// built without the `sdl` feature; the loader must surface the
/// capability's own explicit diagnostic. Runs in every matrix (the SDL kit
/// builds the no-SDL variant itself).
#[test]
fn session_without_the_sdl_feature_is_an_explicit_diagnostic() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let probe = build_fixture("probe_capability", "rudroid_probe_capability");
    let sdl_runtime = sdl_runtime_without_sdl();
    let deploy = temp_dir("no-sdl-kit");
    deploy_kit(
        &deploy,
        &[
            (&probe, "libandroid_base.so"),
            (&sdl_runtime, "libplatform_sdl_runtime.so"),
        ],
    );
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("phases run");
    let sdl = capabilities.iter().find(|c| c.descriptor.id == "platform_sdl_runtime").unwrap();
    unsafe {
        let activate = dlsym_required::<unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn()>) -> i32>(sdl.library(), capability_symbols::GUEST_ACTIVATE);
        let source = dlsym_required::<unsafe extern "C" fn(*const c_char, *mut c_void) -> usize>(sdl.library(), capability_symbols::GUEST_SYMBOL);
        let native_buffer = dlsym_required::<unsafe extern "C" fn(i64, i64, i32, i32) -> *mut c_void>(sdl.library(), capability_symbols::SESSION_CREATE_NATIVE_BUFFER);
        assert_eq!(activate(std::ptr::null_mut(), None), -1);
        assert_eq!(source(c"glClearColor".as_ptr(), std::ptr::null_mut()), 0);
        assert!(native_buffer(64, 64, 3, 2).is_null());
    }
    let config = rudroid_host_config::HostConfig::parse(
        "[device]\ndisplayWidth = 64\ndisplayHeight = 64\n",
    )
    .unwrap();
    let err = runtime::bring_up_sdl_session(&capabilities, &config)
        .expect_err("a feature-less SDL capability cannot create a session");
    let message = err.to_string();
    assert!(
        message.contains("`sdl` feature"),
        "the diagnostic must name the missing feature: {message}"
    );
    let _ = std::fs::remove_dir_all(&deploy);
}

/// Loader-side dynamic phase against a real Xvfb + Mesa software GLES
/// display (SDL kit matrix): the session must be a real ES context with a
/// verified pipeline — this is the anti-dummy, anti-Core-GL assertion.
#[test]
#[cfg_attr(
    not(rudroid_kit_sdl),
    ignore = "needs the SDL kit under Xvfb: bash crates/scripts/docker-rust.sh test --sdl"
)]
fn loader_dynamic_phase_creates_a_verified_es_session() {
    assert!(
        std::env::var_os("DISPLAY").is_some(),
        "the SDL kit tests need a display (docker-rust.sh test --sdl runs Xvfb)"
    );
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let probe = build_fixture("probe_capability", "rudroid_probe_capability");
    let sdl_runtime = ensure_cdylib("rudroid-linux-sdl-runtime", "platform_sdl_runtime");
    let deploy = temp_dir("display-kit");
    deploy_kit(
        &deploy,
        &[
            (&probe, "libandroid_base.so"),
            (&sdl_runtime, "libplatform_sdl_runtime.so"),
        ],
    );
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("phases run");
    // `capabilities` stays alive for the whole call: dropping never dlcloses
    // (ticket 46), and the session lives inside the pinned capability.
    let config = rudroid_host_config::HostConfig::parse(
        "[device]\ndisplayWidth = 1280\ndisplayHeight = 720\n",
    )
    .unwrap();
    let report = runtime::bring_up_sdl_session(&capabilities, &config)
        .expect("the dynamic phase must create a verified ES session")
        .expect("the kit deploys platform_sdl_runtime");
    assert_eq!(
        (report.window_w, report.window_h),
        (1280, 720),
        "the session must use the configured size"
    );
    assert_eq!(report.profile_mask, 0x0004, "the granted profile must be ES");
    assert!(
        report.gl_version.starts_with("OpenGL ES"),
        "version string must identify an ES context: {}",
        report.gl_version
    );
    assert_ne!(report.video_driver, "dummy", "dummy video is not a display");
    assert!(report.swap_count >= 1, "at least one real swap");
    assert!(
        report.readback_non_background,
        "the drawn triangle must survive the pixel readback"
    );
    eprintln!(
        "[EVIDENCE] driver={} window={}x{} GLES {}.{} version={:?} renderer={:?} \
         vendor={:?} swaps={} readback={}",
        report.video_driver, report.window_w, report.window_h, report.major,
        report.minor, report.gl_version, report.gl_renderer, report.gl_vendor,
        report.swap_count, report.readback_non_background
    );
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
#[cfg_attr(not(rudroid_kit_sdl), ignore = "needs the SDL kit under Xvfb")]
fn unsupported_native_presentation_falls_back_without_changing_guest_geometry() {
    const NAME: &str = "unsupported_native_presentation_falls_back_without_changing_guest_geometry";
    const CHILD: &str = "RUDROID_NATIVE_PRESENT_FALLBACK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, "1").env("SDL_VIDEODRIVER", "x11").output().unwrap();
        assert!(result.status.success(), "{}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("requires Wayland; active SDL driver is x11"), "{stderr}");
        assert!(stderr.contains("retaining internal presentation at the same guest size"), "{stderr}");
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let probe = build_fixture("probe_capability", "rudroid_probe_capability");
    let sdl = ensure_cdylib("rudroid-linux-sdl-runtime", "platform_sdl_runtime");
    let deploy = temp_dir("native-fallback-kit");
    deploy_kit(&deploy, &[(&probe, "libandroid_base.so"), (&sdl, "libplatform_sdl_runtime.so")]);
    // SAFETY: only freshly built trusted fixture/kit libraries, pinned by the loader.
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap();
    let capabilities = bring_up(capabilities, None).unwrap();
    let config = rudroid_host_config::HostConfig::parse(
        "[device]\ndisplayWidth=1280\ndisplayHeight=720\n[gpu]\npresentationBackend='wayland_native'\nrenderWidth=640\nrenderHeight=360\nrenderScaleSharp=false\n",
    ).unwrap();
    let report = runtime::bring_up_sdl_session(&capabilities, &config).unwrap().unwrap();
    assert_eq!(report.video_driver, "x11");
    assert_eq!((report.window_w, report.window_h), (1280, 720));
    assert!(report.readback_non_background && report.swap_count > 0);
    let api = rudroid_unity_loader::host_api::table();
    assert_eq!(api.config_i64(c"device.displayWidth", 0), 1280);
    assert_eq!(api.config_i64(c"gpu.renderWidth", 0), 640);
    assert_eq!(api.config_i64(c"gpu.renderHeight", 0), 360);
    let _ = std::fs::remove_dir_all(&deploy);
}

/// The outer build's real SDL feature (this binary links the capability)
/// must match `RUDROID_KIT_FEATURES`, which every nested build replays —
/// e.g. a plain `cargo test --features rudroid-linux-sdl-runtime/sdl`
/// without the variable fails here instead of passing on mislabelled kits.
#[test]
fn outer_sdl_feature_matches_the_declared_kit_features() {
    assert_kit_features(platform_sdl_runtime::SDL_ENABLED);
}

/// The loader's mirrored C API names must never drift from the capability's
/// own constants (test-only link; production dlsyms the deployed cdylib).
#[test]
fn loader_sdl_api_names_match_capability_constants() {
    for (loader, capability) in [
        (runtime::sdl_api::GRAPHICS_FEATURES, capability_symbols::GRAPHICS_FEATURES),
        (runtime::sdl_api::SESSION_DESTROY, capability_symbols::SESSION_DESTROY),
        (runtime::sdl_api::SESSION_CREATE_NATIVE_BUFFER, capability_symbols::SESSION_CREATE_NATIVE_BUFFER),
        (runtime::sdl_api::SESSION_CREATE_VULKAN, capability_symbols::SESSION_CREATE_VULKAN),
        (runtime::sdl_api::GUEST_ACTIVATE, capability_symbols::GUEST_ACTIVATE),
        (runtime::sdl_api::RENDER_CONFIGURE, capability_symbols::RENDER_CONFIGURE),
        (runtime::sdl_api::AUDIO_POLICY_CONFIGURE, capability_symbols::AUDIO_POLICY_CONFIGURE),
        (runtime::sdl_api::GUEST_SYMBOL, capability_symbols::GUEST_SYMBOL),
    ] { assert_eq!(loader, capability); }
    assert_eq!(
        runtime::sdl_api::SESSION_CREATE,
        capability_symbols::SESSION_CREATE
    );
    assert_eq!(
        runtime::sdl_api::SESSION_REPORT,
        capability_symbols::SESSION_REPORT
    );
    assert_eq!(runtime::sdl_api::LAST_ERROR, capability_symbols::LAST_ERROR);
}

/// One process-global log sink shared by every test in this binary (the sink
/// installs once and ignores later installs).
fn captured_log() -> &'static Arc<Mutex<Vec<String>>> {
    use std::sync::OnceLock;
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
