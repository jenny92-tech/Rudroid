//! Ticket 52 acceptance: the minimal dynamic JNI session loop.
//!
//! - Guest probe (automatically compiled C .so using the real jni.h) exports `JNI_OnLoad`; the
//!   loader bring-up maps it through the android_elf_load exec backend and
//!   calls `JNI_OnLoad` with the shared session's `JavaVM*`.
//! - `android_jni_vm` registers that session in the runtime registry's
//!   shared-VM slot during its early-init phase; `android_base` (framework)
//!   and `reply_catalog` (addon representative) join the SAME session and
//!   pull the catalog's known value through the JNI function table.
//! - The probe itself walks GetEnv -> FindClass -> GetStaticMethodID ->
//!   CallStaticIntMethodA (catalog fixed reply 100 across libraries) plus
//!   the four explicit-failure semantics (unknown method, pending
//!   exception, type mismatch, wrong signature), string and global/local
//!   reference operations; it returns JNI_VERSION_1_6 only when every step
//!   passed, so the return code is the chain's proof (step codes 0x5200|n
//!   name the broken link).
//!
//! Self-compiled fixture only — no real libmain/libil2cpp, no Unity and no
//! handheld are involved (see ticket-52 manifest for the boundary).

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rudroid_unity_loader::{bring_up, guest, load_deploy_dir, runtime, Selection};

/// JNI_VERSION_1_6: what a fully successful probe run returns.
const JNI_VERSION_1_6: i32 = 0x0001_0006;

/// Serializes every test in this binary: the runtime registry and the JNI
/// session are process-global.
static SERIAL: Mutex<()> = Mutex::new(());

fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    rudroid_test_support::cargo::ensure_workspace_cdylib(package, artifact)
}

/// Compile against the repository's real jni.h, so table indirection and
/// prototypes are checked independently of the Rust slot helpers.
fn jni_probe() -> PathBuf {
    rudroid_test_support::cargo::compile_c_fixture(
        "capability/android/jni_vm/tests/fixtures/rudroid52_probe.c",
        &["-Werror=incompatible-pointer-types"],
        "rudroid52_probe.so",
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

/// The dynamic-path fixture: the gcc-compiled JNI probe stands in for
/// libmain.so, the synthetic minimal aarch64 image (no JNI_OnLoad) for
/// libil2cpp.so — covering both the "module with JNI_OnLoad" and "module
/// without one" branches of the reference sequence.
fn gamedata_with_probe(deploy: &Path, probe: &Path) -> PathBuf {
    let game_files = deploy.join("gamedata");
    let lib_dir = game_files.join("lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    std::fs::copy(probe, lib_dir.join("libmain.so")).expect("deploy probe as libmain");
    std::fs::write(
        lib_dir.join("libil2cpp.so"),
        android_elf_load::fixture::build_minimal_aarch64_so(),
    )
    .expect("deploy synthetic libil2cpp");
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

#[test]
fn jni_session_minimal_dynamic_loop() {
    let probe = jni_probe();
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let log = captured_log();
    // Fresh registry: whatever a previous test in this binary registered
    // must not stand in for this kit's own bring-up.
    runtime::shutdown();

    let elf_load = ensure_cdylib("rudroid-android-elf-load", "android_elf_load");
    let jni_vm = ensure_cdylib("rudroid-android-jni-vm", "android_jni_vm");
    let framework = ensure_cdylib("rudroid-android-framework", "android_base");
    let catalog = ensure_cdylib(
        "rudroid-android-reply-catalog",
        "rudroid_android_reply_catalog",
    );

    let deploy = std::env::temp_dir().join(format!("rudroid-t52-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    deploy_kit(&deploy, &[&elf_load, &jni_vm, &framework, &catalog]);
    let game_files = gamedata_with_probe(&deploy, &probe);

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("capability bring-up");

    // The session is registered by android_jni_vm's early-init phase.
    let vm = runtime::shared_vm();
    assert!(
        !vm.is_null(),
        "no JNI session is registered (shared-VM slot is null): guest \
         JNI_OnLoad has no JavaVM to run against"
    );
    {
        let lines = log.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[android_jni_vm] session created")),
            "the session capability must log its registration: {lines:?}"
        );
    }

    let events = unsafe { guest::bring_up(&capabilities, &guest_plan(&game_files)) }
        .expect("guest bring-up with a registered session");
    assert_eq!(events.len(), 2, "libmain + libil2cpp mapped");
    assert_eq!(
        events[0].kind,
        rudroid_common::ModuleKind::Main,
        "load order is the reference order"
    );
    let probe_log = log.lock().unwrap().clone();
    assert_eq!(
        events[0].jni_on_load,
        Some(JNI_VERSION_1_6),
        "JNI_OnLoad ran against the shared session and every probe step \
         passed (GetEnv, GetVersion, FindClass, string round trip, method \
         lookup, CallStaticIntMethodA == 100 catalog reply, bool call, type \
         mismatch + pending semantics, unknown method, wrong signature, \
         ThrowNew, global/local reference lifecycle, GetJavaVM identity, \
         blocked-while-pending, framework URI/orientation/system CA queries); logs={:?}",
        probe_log
    );
    assert_eq!(
        events[1].kind,
        rudroid_common::ModuleKind::Il2cpp
    );
    assert_eq!(
        events[1].jni_on_load, None,
        "a module without the export is mapped without a JNI call"
    );

    // Framework and addon joined the SAME session from inside their own
    // deployed libraries.
    {
        let lines = log.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[android_base] session join ok volume=100")),
            "the framework capability must join the shared session: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[reply_catalog] session join ok volume=100")),
            "the catalog capability (addon representative) must join the \
             shared session: {lines:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn jni_onload_without_a_session_fails_guest_bring_up() {
    let probe = jni_probe();
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // Drop every registration — above all the shared-VM slot another test
    // may have filled — so this kit really runs without a session.
    runtime::shutdown();

    let elf_load = ensure_cdylib("rudroid-android-elf-load", "android_elf_load");
    let framework = ensure_cdylib("rudroid-android-framework", "android_base");
    let catalog = ensure_cdylib(
        "rudroid-android-reply-catalog",
        "rudroid_android_reply_catalog",
    );

    let deploy = std::env::temp_dir().join(format!("rudroid-t52-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    // Deliberately NO android_jni_vm: the probe's JNI_OnLoad must not run.
    deploy_kit(&deploy, &[&elf_load, &framework, &catalog]);
    let game_files = gamedata_with_probe(&deploy, &probe);

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None)
        .expect("capability phases degrade without the session capability");
    assert!(runtime::shared_vm().is_null(), "no session may be registered");

    let error = unsafe { guest::bring_up(&capabilities, &guest_plan(&game_files)) }
        .expect_err("a guest JNI_OnLoad without a session must fail bring-up");
    let message = error.to_string();
    assert!(
        message.contains("JNI_OnLoad"),
        "the failure names the guest entry point: {message}"
    );
    if cfg!(target_os = "linux") {
        assert!(
            message.contains("no JNI session is registered"),
            "on Linux (exec backend available) the missing session is the \
             exact, explicit reason: {message}"
        );
    }
    let _ = std::fs::remove_dir_all(&deploy);
}
