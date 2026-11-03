//! INPUT:  production profile cdylibs, shared loader runtime and a compiled guest spy
//! OUTPUT:  lifecycle, successful-frame events and bootstrap/failure acceptance checks
//! POS:    exercises both engine profiles through actual guest mapping and JNI calls
//! Ticket 53 acceptance: the Unity profile lifecycle actually drives the
//! guest, through the real loader and the real deployed profile cdylibs.
//!
//! - The guest is the self-compiled C spy (`tests/fixtures/rudroid53_spy.c`, gcc,
//!   `-nostdlib`, mapped by the loader through the ticket-49 exec backend).
//!   It stands in for libmain/libunity's registered native bodies and
//!   records every NativeLoader/JNI/bootstrap/frame-loop call with its
//!   arguments into an event buffer the profile echoes into the host log
//!   after the run.
//! - The lifecycle runs in reference order through the runtime runner:
//!   prepare (before the guest modules are mapped) → guest bring-up
//!   (exec-mapped modules + per-module JNI_OnLoad) → bootstrap
//!   (`NativeLoader.load` dispatched into the guest) → run (initJni, the
//!   reference lifecycle sequence, N frames with vsync signalling, clean
//!   exit, preferences flush).
//! - Failure paths exit non-zero with the stage named: a missing required
//!   guest function, a false `NativeLoader.load`, missing Android hooks, a
//!   missing JNI session. Success is never reported before guest code ran.
//!
//! Self-compiled fixtures only — no real libmain/libil2cpp/libunity, no
//! Unity engine and no handheld are involved (see the ticket-53 manifest
//! for the fixture boundary). The registry, the JNI session, the log sink
//! and the profile session slot are process-global, so the in-process
//! tests serialize on ONE mutex (the ticket-52 lesson) and every one of
//! them starts from `runtime::shutdown()`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use rudroid_test_support::cargo::{
    build_loader_fixture, compile_c_fixture, ensure_workspace_cdylib,
};
use rudroid_unity_loader::{
    bring_up, guest, host_api, load_deploy_dir, runtime, GuestPlan, Selection,
};

const BIN: &str = env!("CARGO_BIN_EXE_rudroid-unity-loader");

/// Serializes the in-process tests: the runtime registry, the shared JNI
/// session, the log sink and the profile session slot are process-global.
static SERIAL: Mutex<()> = Mutex::new(());

/// Build a workspace cdylib with this run's profile and kit features;
/// always rebuilds incrementally so a stale artifact can never stand in for
/// the current kit (same-build ids).
fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    ensure_workspace_cdylib(package, artifact)
}

/// Build a loader fixture crate (own workspace, audio-policy precedent).
/// `lib_stem` is the fixture crate's `[lib] name` (the artifact stem, which
/// must equal its descriptor id).
fn build_fixture(name: &str, lib_stem: &str) -> PathBuf {
    build_loader_fixture(name, lib_stem)
}

/// Compile one spy variant (`tests/fixtures/rudroid53_spy.c`, `-nostdlib` like the
/// rudroid52 probe — elf_load binds no undefined imports). Compiled once per
/// process; `extra_flags` produces the failure variants. A missing C
/// compiler fails the test.
fn compile_spy(extra_flags: &[&str], name: &str) -> PathBuf {
    compile_c_fixture(
        "project/unity_loader/tests/fixtures/rudroid53_spy.c",
        extra_flags,
        name,
    )
}

/// One process-global log sink shared by every test in this binary (the
/// sink installs once and ignores later installs).
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

fn deploy_kit(dir: &Path, artifacts: &[&Path]) {
    std::fs::create_dir_all(dir).unwrap();
    for artifact in artifacts {
        std::fs::copy(artifact, dir.join(artifact.file_name().unwrap())).unwrap();
    }
}

/// Which member stands as the kit's `android_base` (exactly one per kit,
/// ticket 31 topology).
enum AndroidBase {
    /// The production framework: registers the ticket-77 runtime hooks
    /// itself during its register-JNI phase.
    Framework,
    /// The ticket-44 probe fixture: registers no hooks, so the kit exercises
    /// the negative arm of the reference prepare gate.
    Probe,
}

/// Kit for one profile: mapping, session, exactly ONE android_base member,
/// catalog and exactly one unity profile cdylib. `hooks_fixture` deploys the
/// ticket-53 fixture member as an EXTRA member — since ticket 77 it always
/// loses the first-write-wins race to the production framework, and only the
/// framework's hook events may be asserted (the ticket-77 contract test
/// pins this).
struct Kit {
    deploy: PathBuf,
    config: String,
}

fn build_kit(
    profile_artifact: &Path,
    spy: &Path,
    android_base: AndroidBase,
    hooks_fixture: bool,
    with_session: bool,
) -> Kit {
    let elf_load = ensure_cdylib("rudroid-android-elf-load", "android_elf_load");
    let framework = ensure_cdylib("rudroid-android-framework", "android_base");
    let catalog = ensure_cdylib(
        "rudroid-android-reply-catalog",
        "rudroid_android_reply_catalog",
    );
    let hooks = build_fixture("hooks_capability", "android_hooks_fixture");
    let probe = build_fixture("probe_capability", "rudroid_probe_capability");
    let jni_vm = ensure_cdylib("rudroid-android-jni-vm", "android_jni_vm");
    let android_base_member: &Path = match android_base {
        AndroidBase::Framework => &framework,
        AndroidBase::Probe => &probe,
    };

    // Unique per call: parallel tests must never share (or clean up each
    // other's) deploy dirs.
    static KIT_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let deploy = std::env::temp_dir().join(format!(
        "rudroid-t53-{}-{}-{}",
        profile_artifact.file_stem().unwrap().to_string_lossy(),
        std::process::id(),
        KIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&deploy);
    let mut members: Vec<&Path> = vec![&elf_load, android_base_member, &catalog, profile_artifact];
    if hooks_fixture {
        members.push(&hooks);
    }
    if with_session {
        // The loader sorts members by kind, so deploy-dir order is irrelevant.
        members.push(&jni_vm);
    }
    deploy_kit(&deploy, &members);

    let game_files = deploy.join("gamedata");
    let lib_dir = game_files.join("lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    std::fs::copy(spy, lib_dir.join("libmain.so")).expect("deploy spy as libmain");
    std::fs::copy(spy, lib_dir.join("libil2cpp.so")).expect("deploy spy as libil2cpp");

    // NOTE: `install_config` is first-write-wins per process, so every
    // in-process kit shares ONE [unity] value set (`context_type = 7` is
    // only CONSUMED by the unity_6 flags gate; 2021_3 never passes it).
    // Each test still passes its own parsed config to `GuestPlan`, so the
    // per-kit game_files path is always the live one.
    let config = format!(
        "[paths]\ngame_files = {:?}\n[unity]\nsmoke_frame_limit = 3\n\
         render_sleep_us = 1000\ncontext_type = 7\n",
        game_files.display()
    );
    Kit { deploy, config }
}

impl Kit {
    /// Consume the kit WITHOUT removing its deploy dir: the CLI tests need
    /// the on-disk kit to outlive this builder.
    fn keep(self) -> (PathBuf, String) {
        let kit = std::mem::ManuallyDrop::new(self);
        (kit.deploy.clone(), kit.config.clone())
    }
}

impl Drop for Kit {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.deploy);
    }
}

/// Run the full reference sequence in-process against the deployed kit:
/// phases → prepare → guest bring-up → bootstrap → run. Returns the fired
/// module events (their `jni_on_load` values prove both modules' guest
/// JNI_OnLoad ran against the shared session).
fn run_lifecycle(kit: &Kit) -> Result<Vec<guest::ModuleEvent>, Box<dyn std::error::Error>> {
    let config = rudroid_host_config::HostConfig::parse(&kit.config)?;
    host_api::install_config(Some(config.clone())).unwrap();
    let capabilities = unsafe { load_deploy_dir(&kit.deploy, &Selection::All) }?;
    let capabilities = bring_up(capabilities, None)?;
    runtime::prepare_unity_runtime(&capabilities)?;
    let plan = GuestPlan::from_config(&config)?;
    let events = unsafe { guest::bring_up(&capabilities, &plan) }?;
    runtime::bootstrap_unity_runtime(&capabilities, "lib/arm64-v8a")?;
    assert_eq!(runtime::run_unity_runtime(&capabilities)?, runtime::UnityRunCompletion::Stopped);
    // Handles stay open for the process life (pinned kit, ticket 46).
    std::mem::forget(capabilities);
    Ok(events)
}

/// Assert `needles` appear in the log in the given relative order.
fn assert_log_sequence(log: &Arc<Mutex<Vec<String>>>, needles: &[&str]) {
    let lines = log.lock().unwrap().clone();
    let mut search_from = 0;
    for needle in needles {
        let found = lines[search_from..]
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("log line `{needle}` missing or out of order in {lines:?}"));
        search_from += found + 1;
    }
}

/// One Unity engine profile under the shared lifecycle contract. Only the
/// fields below may differ between engine versions; every other observable
/// step is asserted identically for each row.
struct ProfileCase {
    package: &'static str,
    artifact: &'static str,
    tag: &'static str,
    registered: &'static str,
    player_class: &'static str,
    init_signature: &'static str,
    /// The spy's record of the initJni dispatch (shape + forwarded int).
    init_event: &'static str,
    pad_init_signature: &'static std::ffi::CStr,
}

const UNITY_2021_3: ProfileCase = ProfileCase {
    package: "rudroid-unity-profile-2021-3",
    artifact: "unity_2021_3",
    tag: "UNITY2021",
    registered: "Unity 2021.3 JNI and early engine profile registered",
    player_class: "com/unity3d/player/UnityPlayer",
    init_signature: "(Landroid/content/Context;)V",
    init_event: "kind=INIT_JNI_CTX(3) a=0x0 b=0x1",
    pad_init_signature: c"(Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;",
};

const UNITY_6: ProfileCase = ProfileCase {
    package: "rudroid-unity-profile-6",
    artifact: "unity_6",
    tag: "UNITY6",
    registered: "Unity 6 early engine profile registered",
    player_class: "com/unity3d/player/UnityPlayerForActivityOrService",
    init_signature: "(Landroid/content/Context;I)V",
    // The kit's `context_type = 7` reaches the int-argument shape.
    init_event: "kind=INIT_JNI_CTX_INT(4) a=0x7 b=0x1",
    pad_init_signature: c"(Lcom/unity3d/player/UnityPlayer;Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;",
};

/// Every engine profile in the workspace. A new Unity version adds a row
/// here (enforced by `every_workspace_unity_profile_is_under_the_contract`)
/// and inherits the whole contract instead of a thinner copy.
const PROFILES: [&ProfileCase; 2] = [&UNITY_2021_3, &UNITY_6];

/// The full reference sequence for one profile, observed through the
/// capability/runner log and the spy's own event buffer.
fn assert_lifecycle_contract(case: &ProfileCase) {
    let spy = compile_spy(&[], "rudroid53_spy.so");
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let log = captured_log();
    log.lock().unwrap().clear();

    let profile = ensure_cdylib(case.package, case.artifact);
    let kit = build_kit(&profile, &spy, AndroidBase::Framework, false, true);
    static FRAMES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    FRAMES.store(0, std::sync::atomic::Ordering::Relaxed);
    extern "C" fn frame(_: Option<&rudroid_common::registry::RudroidModuleEvent>, _: *mut std::ffi::c_void) {
        FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    assert_eq!(runtime::register_module_loaded(rudroid_common::ModuleKind::UnityFrame as u32, frame, std::ptr::null_mut()), 0);
    let events = run_lifecycle(&kit)
        .unwrap_or_else(|error| panic!("{} lifecycle against the live spy: {error}", case.artifact));
    assert_eq!(FRAMES.load(std::sync::atomic::Ordering::Relaxed), 3, "one event per successful render");
    assert_eq!(events.len(), 2, "libmain + libil2cpp mapped");
    let env = host_api::table().java_vm().unwrap().get_env(0x10006).unwrap();
    let unity = env.find_class(c"com/unity3d/player/UnityPlayer");
    let activity = env.get_static_object_field(unity,
        env.get_static_field_id(unity, c"currentActivity", c"Landroid/app/Activity;"));
    assert!(!activity.is_null(), "{} publishes its Activity", case.artifact);
    let activity_class = env.find_class(c"com/unity3d/player/UnityPlayerActivity");
    let context_class = env.find_class(c"android/content/Context");
    assert!(env.is_instance_of(activity, activity_class));
    assert!(env.is_instance_of(activity, context_class));
    let player = env.get_object_field(activity, env.get_field_id(activity_class,
        c"mUnityPlayer", c"Lcom/unity3d/player/UnityPlayer;"));
    let player_class = std::ffi::CString::new(case.player_class).unwrap();
    let player_class = env.find_class(&player_class);
    assert!(!player.is_null());
    assert!(env.is_instance_of(player, player_class));
    let pad = env.find_class(c"com/unity3d/player/PlayAssetDeliveryUnityWrapper");
    assert!(env.static_method(pad, c"init", case.pad_init_signature).is_some());
    assert!(!env.exception_check());
    for local in [activity, unity, pad, activity_class, context_class, player, player_class] { env.delete_local_ref(local); }
    assert_eq!(
        events[0].jni_on_load,
        Some(0x0001_0006),
        "libmain's guest JNI_OnLoad ran"
    );
    assert_eq!(
        events[1].jni_on_load,
        Some(0x0001_0006),
        "libil2cpp's guest JNI_OnLoad ran"
    );

    // register → prepare → (guest mapping + per-module JNI_OnLoad happen
    // between prepare and bootstrap — asserted on the CLI's full stderr
    // below and structurally here: bootstrap can only resolve the guest
    // main module through the module subscription prepare registered)
    // → NativeLoader.load → initJni → lifecycle natives → N frames →
    // clean exit → preferences flush.
    let tag = case.tag;
    let artifact = case.artifact;
    let expected = [
        format!("[{tag}] {}", case.registered),
        format!("[{tag}] active engine profile={artifact} player={}", case.player_class),
        format!("[PROFILE] unity profile prepared: {artifact}"),
        format!("[{tag}] calling NativeLoader.load(lib/arm64-v8a)"),
        format!("[{tag}] NativeLoader.load(lib/arm64-v8a) returned true"),
        format!("[PROFILE] unity profile bootstrapped: {artifact}"),
        format!("[{tag}] calling initJni ({})", case.init_signature),
        format!("[{tag}] calling nativeRecreateGfxState"),
        format!("[{tag}] calling nativeRestartActivityIndicator"),
        format!("[{tag}] calling nativeSendSurfaceChangedEvent"),
        format!("[{tag}] calling nativeResume"),
        format!("[{tag}] calling nativeFocusChanged(true)"),
        format!("[{tag}] nativeRender returned true; entering render loop"),
        format!("[{tag}] render loop complete: 3 frames rendered (player stopped)"),
        "[android_base] flush_preferences".to_owned(),
        format!("[PROFILE] unity profile run complete: {artifact}"),
    ];
    assert_log_sequence(log, &expected.iter().map(String::as_str).collect::<Vec<_>>());
    // The spy's own event buffer, echoed by the profile: the exact call
    // sequence WITH arguments — JNI_OnLoad version, the dir string the
    // guest read through GetStringUTFChars, the initJni shape, the
    // lifecycle order and the per-frame vsync→render pairs.
    let init = format!("spy event[2] {}", case.init_event);
    assert_log_sequence(
        log,
        &[
            "spy event[0] kind=JNI_ONLOAD(1) a=0x10006",
            "spy event[1] kind=NATIVE_LOADER_LOAD(2) a=0x1 b=0x0 text=\"lib/arm64-v8a\"",
            init.as_str(),
            "spy event[3] kind=RECREATE_GFX(5) a=0x0 b=0x0",
            "spy event[4] kind=RESTART_INDICATOR(6)",
            "spy event[5] kind=SURFACE_CHANGED(7)",
            "spy event[6] kind=RESUME(9)",
            "spy event[7] kind=FOCUS_CHANGED(8) a=0x1",
            "spy event[8] kind=SIGNAL_VSYNC(11) a=0x1",
            "spy event[9] kind=RENDER(10) a=0x1",
            "spy event[10] kind=SIGNAL_VSYNC(11) a=0x2",
            "spy event[11] kind=RENDER(10) a=0x2",
            "spy event[12] kind=SIGNAL_VSYNC(11) a=0x3",
            "spy event[13] kind=RENDER(10) a=0x3",
        ],
    );
    // Three frames ⇒ three vsync signals and three renders, no more.
    let lines = log.lock().unwrap().clone();
    assert_eq!(lines.iter().filter(|l| l.contains("kind=SIGNAL_VSYNC")).count(), 3);
    assert_eq!(lines.iter().filter(|l| l.contains("kind=RENDER(")).count(), 3);
    assert_eq!(lines.iter().filter(|l| l.contains("kind=PAUSE(")).count(), 0,
        "terminal destruction must not suspend native producers first");
    assert_eq!(lines.iter().filter(|l| l.contains("kind=DONE(")).count(), 1,
        "nativeDone must still execute exactly once");
}

#[test]
fn profile_lifecycle_contract_unity_2021_3() {
    assert_lifecycle_contract(&UNITY_2021_3);
}

#[test]
fn profile_lifecycle_contract_unity_6() {
    assert_lifecycle_contract(&UNITY_6);
}

/// Every `capability/unity/profile_*` workspace member must have a
/// `PROFILES` row, so a new engine version cannot ship with a weaker
/// lifecycle test than the existing ones.
#[test]
fn every_workspace_unity_profile_is_under_the_contract() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace = std::fs::read_to_string(crates.join("Cargo.toml")).unwrap();
    let members: Vec<&str> = workspace
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix("\","))
        .filter(|member| member.starts_with("capability/unity/profile_"))
        .collect();
    assert!(members.len() >= PROFILES.len(), "workspace profiles: {members:?}");
    for member in members {
        let manifest = std::fs::read_to_string(crates.join(member).join("Cargo.toml")).unwrap();
        let package = manifest
            .lines()
            .find_map(|line| line.trim().strip_prefix("name = \"")?.strip_suffix('"'))
            .unwrap_or_else(|| panic!("{member} has no package name"));
        assert!(
            PROFILES.iter().any(|case| case.package == package),
            "{package} ({member}) has no lifecycle contract row in PROFILES"
        );
    }
}

#[test]
fn missing_required_guest_function_fails_the_run() {
    let spy = compile_spy(&["-DRUDROID_SPY_OMIT_RENDER"], "rudroid53_spy_no_render.so");
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    for case in PROFILES {
        runtime::shutdown();
        let log = captured_log();
        log.lock().unwrap().clear();

        let profile = ensure_cdylib(case.package, case.artifact);
        let kit = build_kit(&profile, &spy, AndroidBase::Framework, false, true);
        let error = run_lifecycle(&kit).expect_err("a guest without nativeRender cannot run");
        let message = error.to_string();
        assert!(
            message.contains("failed at the run stage"),
            "{}: the failure names the stage: {message}",
            case.artifact
        );
        let lines = log.lock().unwrap().clone();
        let missing = lines
            .iter()
            .find(|l| l.contains("required lifecycle JNI method unavailable"))
            .unwrap_or_else(|| panic!("{}: the profile names the missing functions", case.artifact));
        assert!(
            lines.iter().any(|l| l.contains("nativeRender()Z")),
            "{}: render is the missing export: {missing}",
            case.artifact
        );
    }
}

#[test]
fn native_loader_false_return_fails_bootstrap() {
    let spy = compile_spy(&["-DRUDROID_SPY_LOAD_FAILS"], "rudroid53_spy_load_fails.so");
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    for case in PROFILES {
        runtime::shutdown();
        let log = captured_log();
        log.lock().unwrap().clear();

        let profile = ensure_cdylib(case.package, case.artifact);
        let kit = build_kit(&profile, &spy, AndroidBase::Framework, false, true);
        let error = run_lifecycle(&kit).expect_err("a false NativeLoader.load must fail");
        let message = error.to_string();
        assert!(
            message.contains("failed at the bootstrap stage") && message.contains("NativeLoader.load"),
            "{}: the failure names the stage and the guest refusal: {message}",
            case.artifact
        );
        assert_log_sequence(log, &[format!("[{}] NativeLoader.load returned false", case.tag).as_str()]);
        // The spy recorded the call (it RAN — and answered false); the run and
        // its render loop never happened.
        let lines = log.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("kind=NATIVE_LOADER_LOAD(2) a=0x0")),
            "{}: the spy recorded the refused call with its false return",
            case.artifact
        );
        assert!(
            !lines.iter().any(|l| l.contains("calling initJni")),
            "{}: initJni never runs after a failed bootstrap",
            case.artifact
        );
    }
}

#[test]
fn prepare_requires_registered_android_hooks() {
    let spy = compile_spy(&[], "rudroid53_spy.so");
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let _ = captured_log();

    let profile = ensure_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    // Ticket 77 assembly: the android_base member is the ticket-44 probe
    // fixture, which registers NO runtime hooks (the production framework
    // would register them). The reference prepare gate must refuse — the
    // "android_base plugin is missing or failed" arm — before the guest is
    // touched.
    let kit = build_kit(&profile, &spy, AndroidBase::Probe, false, true);
    let error = run_lifecycle(&kit).expect_err("prepare without hooks must fail");
    let message = error.to_string();
    assert!(
        message.contains("failed at the prepare stage")
            && message.contains("android_base plugin is missing or failed"),
        "the reference gate message: {message}"
    );
}

/// Ticket 77 contract: a hooks fixture member deployed ALONGSIDE the
/// production framework always loses the first-write-wins race (android_base
/// is the phase-first registrar). Its duplicate registration is rejected
/// WITHOUT failing its phase, the kit passes the prepare gate on the
/// framework's hooks, and only the framework's hook events are observable —
/// the fixture's hooks are never invoked. Exactly one side wins; the tests
/// assert only the framework's events.
#[test]
fn late_hooks_fixture_loses_the_first_write_race_to_the_framework() {
    let spy = compile_spy(&[], "rudroid53_spy.so");
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let log = captured_log();
    log.lock().unwrap().clear();

    let profile = ensure_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    let kit = build_kit(&profile, &spy, AndroidBase::Framework, true, true);
    run_lifecycle(&kit).expect("the kit with the extra fixture member runs");

    let lines = log.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("android runtime hooks registered")),
        "the production framework registered the hooks: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| {
            l.contains("registration rejected")
                && l.contains("production android_base framework owns the runtime hooks")
        }),
        "the fixture logs its rejection explicitly instead of failing: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("[android_base] flush_preferences")),
        "the framework's flush hook is the one invoked at run exit: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("[android_hooks_fixture] Android runtime hooks registered")),
        "the fixture must not claim a successful registration: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("[android_hooks_fixture] flush_preferences invoked")),
        "the fixture's hooks are never invoked after losing the race: {lines:?}"
    );
}

#[test]
fn register_jni_requires_the_shared_session() {
    let spy = compile_spy(&[], "rudroid53_spy.so");
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    for case in PROFILES {
        runtime::shutdown();
        let _ = captured_log();

        let profile = ensure_cdylib(case.package, case.artifact);
        // No android_jni_vm: the profile refuses to register (reference: the
        // registration phase requires the JNI runtime) and the phase fails.
        let kit = build_kit(&profile, &spy, AndroidBase::Framework, false, false);
        let config = rudroid_host_config::HostConfig::parse(&kit.config).unwrap();
        host_api::install_config(Some(config.clone())).unwrap();
        let capabilities = unsafe { load_deploy_dir(&kit.deploy, &Selection::All) }.unwrap();
        let error = bring_up(capabilities, None).expect_err("registration without a session fails");
        let message = error.to_string();
        assert!(
            message.contains("RegisterJni") && message.contains(case.artifact),
            "the phase failure names the capability: {message}"
        );
    }
}

// ---------------------------------------------------------------- CLI exit codes

struct CliKit {
    dir: PathBuf,
    config: PathBuf,
}

fn build_cli_kit(profile_artifact: &Path, spy: &Path) -> CliKit {
    let (dir, config) =
        build_kit(profile_artifact, spy, AndroidBase::Framework, false, true).keep();
    let config_path = dir.join("host.toml");
    std::fs::write(&config_path, &config).unwrap();
    CliKit {
        dir,
        config: config_path,
    }
}

fn cli_command(kit: &CliKit) -> Command {
    let mut command = Command::new(BIN);
    command.arg(&kit.dir).arg("--config").arg(&kit.config)
        .env_remove("RUDROID_SKIP_GUEST");
    command
}

fn run_cli(kit: &CliKit) -> (i32, String, String) {
    let output = cli_command(kit).output().expect("spawn rudroid-unity-loader");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn cli_recovers_initial_false_then_accepts_engine_stop_for_both_profiles() {
    let spy = compile_spy(
        &["-DRUDROID_SPY_RENDER_FALSE_BEFORE=2", "-DRUDROID_SPY_RENDER_STOP_AT=5"],
        "rudroid53_spy_engine_stop.so",
    );
    for case in PROFILES {
        let profile = ensure_cdylib(case.package, case.artifact);
        let kit = build_cli_kit(&profile, &spy);
        let text = std::fs::read_to_string(&kit.config).unwrap()
            .replace("smoke_frame_limit = 3", "smoke_frame_limit = 8");
        std::fs::write(&kit.config, format!("{text}\nrender_false_retry_cap=3\nrender_false_retry_sleep_us=0\n")).unwrap();
        let started = std::time::Instant::now();
        let (code, stdout, stderr) = run_cli(&kit);
        assert_eq!(code, 0, "{}: {stderr}", case.artifact);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "engine stop must not wait out a retry budget");
        assert!(stdout.contains("result=started"), "{stdout}");
        assert!(stderr.contains("nativeRender requested player stop"), "{stderr}");
        assert!(stderr.contains("render loop complete: 2 frames rendered"), "{stderr}");
        assert_eq!(stderr.matches("kind=RENDER(").count(), 5, "initial retries then terminal render: {stderr}");
        assert_eq!(stderr.matches("kind=DONE(").count(), 1, "nativeDone exactly once: {stderr}");
        assert!(!stderr.contains("kind=PAUSE("), "terminal teardown must not pause the mixer: {stderr}");
        assert!(stderr.contains("flush_preferences"), "{stderr}");
    }
}

#[test]
fn cli_fast_exit_requires_engine_stop_and_successful_preference_flush() {
    let variants: &[(&[&str], &str, bool, i32)] = &[
        (&["-DRUDROID_SPY_RENDER_STOP_AT=5"], "fast_stop", true, 0),
        (&["-DRUDROID_SPY_RENDER_STOP_AT=5", "-DRUDROID_SPY_FLUSH_REFUSE"], "fast_save_failure", false, 0),
        (&[], "fast_frame_limit", false, 0),
        (&["-DRUDROID_SPY_RENDER_FALSE_BEFORE=100"], "fast_start_failure", false, 1),
    ];
    for &(flags, name, fast, expected_code) in variants {
        let spy=compile_spy(flags,&format!("rudroid53_spy_{name}.so"));
        for case in PROFILES {
            let profile=ensure_cdylib(case.package,case.artifact);
            let kit=build_cli_kit(&profile,&spy);
            let text=std::fs::read_to_string(&kit.config).unwrap()
                .replace("smoke_frame_limit = 3","smoke_frame_limit = 8");
            std::fs::write(&kit.config,format!("{text}\nfast_exit=true\nrender_false_retry_cap=1\nrender_false_retry_sleep_us=0\n")).unwrap();
            let (code,stdout,stderr)=run_cli(&kit);
            assert_eq!(code,expected_code,"{name}/{}: {stderr}",case.artifact);
            assert_eq!(stdout.contains("result=process-exit saved=true"),fast,"{name}: {stdout} {stderr}");
            assert_eq!(stderr.matches("kind=DONE(").count(),usize::from(!fast),"{name}: {stderr}");
            assert_eq!(stderr.contains("requesting immediate process exit"),fast,"{name}: {stderr}");
            assert!(!stderr.contains("kind=PAUSE("),"{name}: {stderr}");
            if name=="fast_save_failure" {
                assert!(stderr.contains("preference flush failed; retaining full teardown"),"{stderr}");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn cli_failure_after_guest_entry_cleans_services_without_replaying_finalizers() {
    let preload = compile_c_fixture(
        "project/unity_loader/tests/fixtures/process_finalizer.c", &[], "process_finalizer.so",
    );
    let profile = ensure_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    for (flags, name, expected) in [
        (&["-DRUDROID_SPY_LOAD_FAILS"][..], "rudroid53_spy_finalizer_load_failure.so", "failed at the bootstrap stage"),
        (&["-DRUDROID_SPY_RENDER_FALSE_BEFORE=100"][..], "rudroid53_spy_finalizer_start_failure.so", "first nativeRender still false"),
    ] {
        let spy = compile_spy(flags, name);
        let kit = build_cli_kit(&profile, &spy);
        let text = std::fs::read_to_string(&kit.config).unwrap();
        std::fs::write(&kit.config, format!("{text}\nrender_false_retry_cap=2\nrender_false_retry_sleep_us=0\n")).unwrap();
        let marker = kit.dir.join("retired-finalizer");
        let output = cli_command(&kit).env("LD_PRELOAD", &preload)
            .env("RUDROID_FINALIZER_MARKER", &marker).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "failure status must survive: {stderr}");
        assert!(stderr.contains(expected), "{stderr}");
        assert!(stderr.contains("cleanup completed"), "{stderr}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("result=started"));
        assert!(!marker.exists(), "failed launch must not replay retired native finalizers");

        // Prove the preloaded callback really runs on an ordinary pre-guest
        // failure; an unloaded or inert fixture cannot fake the assertion above.
        let control = cli_command(&kit).arg("--unexpected-argument")
            .env("LD_PRELOAD", &preload).env("RUDROID_FINALIZER_MARKER", &marker)
            .output().unwrap();
        assert_eq!(control.status.code(), Some(2));
        assert_eq!(std::fs::read(&marker).unwrap(), b"replayed");
    }
}

#[test]
fn cli_run_reports_a_started_engine_and_exits_zero() {
    let spy = compile_spy(&[], "rudroid53_spy.so");
    let profile = ensure_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    let kit = build_cli_kit(&profile, &spy);
    let (code, stdout, stderr) = run_cli(&kit);
    assert_eq!(code, 0, "a clean N-frame smoke exits 0; stderr: {stderr}");
    assert!(
        stdout.contains("result=started"),
        "the started banner: {stdout}"
    );
    assert!(
        stderr.contains("NativeLoader.load(lib/arm64-v8a) returned true")
            && stderr.contains("render loop complete: 3 frames rendered"),
        "the lifecycle evidence is on stderr: {stderr}"
    );
    // Reference bring-up order on the full stderr: prepare runs BEFORE the
    // guest main module is mapped, bootstrap and run after it.
    let prepared = stderr
        .find("[PROFILE] unity profile prepared: unity_2021_3")
        .expect("prepare logged on stderr");
    let mapped = stderr
        .find("[GUEST] exec-mapped lib/arm64-v8a/libmain.so")
        .expect("guest mapping logged on stderr");
    let bootstrapped = stderr
        .find("[PROFILE] unity profile bootstrapped")
        .expect("bootstrap logged on stderr");
    assert!(
        prepared < mapped && mapped < bootstrapped,
        "reference order: prepare → guest modules → bootstrap"
    );
}

#[test]
fn cli_run_fails_non_zero_when_the_guest_function_is_missing() {
    let spy = compile_spy(&["-DRUDROID_SPY_OMIT_RENDER"], "rudroid53_spy_no_render.so");
    let profile = ensure_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    let kit = build_cli_kit(&profile, &spy);
    let (code, stdout, stderr) = run_cli(&kit);
    assert_ne!(
        code, 0,
        "a missing required guest function must exit non-zero"
    );
    assert!(
        !stdout.contains("result=started"),
        "a failed run never prints the started banner"
    );
    assert!(
        stderr.contains("failed at the run stage") && stderr.contains("nativeRender()Z"),
        "stderr names the stage and the missing function: {stderr}"
    );
}

#[test]
fn cli_run_fails_non_zero_when_native_loader_returns_false() {
    let spy = compile_spy(&["-DRUDROID_SPY_LOAD_FAILS"], "rudroid53_spy_load_fails.so");
    let profile = ensure_cdylib("rudroid-unity-profile-2021-3", "unity_2021_3");
    let kit = build_cli_kit(&profile, &spy);
    let (code, stdout, stderr) = run_cli(&kit);
    assert_ne!(code, 0, "a refused NativeLoader.load must exit non-zero");
    assert!(
        !stdout.contains("result=started"),
        "bootstrap failed: the engine is never reported started"
    );
    assert!(
        stderr.contains("failed at the bootstrap stage") && stderr.contains("returned false"),
        "stderr names the stage and the guest refusal: {stderr}"
    );
    assert!(
        stderr.contains("quiescing service producers")
            && stderr.contains("joining core workers")
            && stderr.contains("cleanup completed"),
        "bootstrap failure still closes shared services"
    );
    assert!(
        !stderr.contains("pausing player") && !stderr.contains("destroying native player"),
        "an uninitialized player must not receive nativePause/nativeDone"
    );
}
