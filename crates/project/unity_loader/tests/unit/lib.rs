//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

use rudroid_test_support::cargo::{build_fixture, ensure_workspace_cdylib};

/// The dummy capability lives outside the workspace so it never ships in
/// a kit; the shared 73/75 assembly helper rebuilds it with this run's
/// profile, kit features and target dir (always rebuilt: the same-build
/// id changes whenever common sources change).
fn ensure_dummy_built() -> PathBuf {
    build_fixture(
        "project/unity_loader/tests/fixtures/dummy_capability/Cargo.toml",
        "rudroid_dummy_capability",
    )
}

#[test]
fn loads_dummy_and_prints_descriptor_fields() {
    let artifact = ensure_dummy_built();
    let deploy = std::env::temp_dir().join(format!("rudroid-loader-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(deploy.join("plugins")).unwrap();
    std::fs::copy(&artifact, deploy.join("plugins").join(artifact.file_name().unwrap())).unwrap();

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load deploy dir");
    assert_eq!(capabilities.len(), 1);
    let d = &capabilities[0].descriptor;
    assert_eq!(d.id, "dummy");
    assert_eq!(d.kind, Kind::Logging);
    assert_eq!(d.target_package, None);
    assert!(d.required_plugins.is_empty());
    assert_eq!(d.build_id, rudroid_common::compiled_build_id());
    // Handles stay open (pinned) for process lifetime.
    std::mem::forget(capabilities);
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn allowlist_selects_only_named_capability() {
    let artifact = ensure_dummy_built();
    let deploy = std::env::temp_dir().join(format!("rudroid-loader3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(&artifact, deploy.join(artifact.file_name().unwrap())).unwrap();

    // Allowlist that names the file stem selects it (reference parity:
    // ids equal file stems); non-matching allowlist is the fatal case.
    let selection = Selection::Allowlist(vec!["rudroid_dummy_capability".to_owned()]);
    let capabilities = unsafe { load_deploy_dir(&deploy, &selection) }.expect("allowlist hit");
    assert_eq!(capabilities.len(), 1);
    assert_eq!(capabilities[0].descriptor.id, "dummy");
    std::mem::forget(capabilities);

    let selection = Selection::Allowlist(vec!["absent".to_owned()]);
    let err = unsafe { load_deploy_dir(&deploy, &selection) }.unwrap_err().to_string();
    assert!(err.contains("absent"), "actionable missing id: {err}");
    let _ = std::fs::remove_dir_all(&deploy);
}

/// One comprehensive Host API + phase test: the log sink is a
/// process-global OnceLock, so sink assertions and the phase-order
/// assertions must live in the same test to avoid interference.
#[test]
fn host_api_sink_and_phase_order() {
    use std::sync::{Arc, Mutex};

    let artifact = ensure_dummy_built();
    let deploy = std::env::temp_dir().join(format!("rudroid-loader4-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(&artifact, deploy.join(artifact.file_name().unwrap())).unwrap();

    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_handle = captured.clone();
    host_api::install_log_sink(Box::new(move |tag, message| {
        sink_handle.lock().unwrap().push(format!("[{tag}] {message}"));
    }));

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load deploy dir");
    assert_eq!(capabilities.len(), 1);
    let capability = &capabilities[0];

    // Init alone: the init phase log reaches the sink.
    unsafe {
        host_api::init_capability(&capability.descriptor.id, capability.library())
            .expect("dummy init succeeds");
    }
    assert!(captured.lock().unwrap().iter().any(|l| l == "[dummy] phase=init"));

    // Full phase run on a fresh load: early -> register_jni -> init.
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("reload");
    phases::run_all(&capabilities).expect("phases");
    let lines = captured.lock().unwrap().clone();
    let phases_seen: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("[dummy] phase="))
        .collect();
    assert_eq!(
        phases_seen,
        ["init", "early", "register_jni", "init"],
        "capability phases must run in reference order, got {lines:?}"
    );
    // Handles stay open (pinned) for process lifetime.
    std::mem::forget(capabilities);

    // Reference load_jni: the logging plugin's early init runs before
    // every other early init, although validation orders android_base
    // first for the later phases.
    let probe = build_fixture(
        "project/unity_loader/tests/fixtures/probe_capability/Cargo.toml",
        "rudroid_probe_capability",
    );
    std::fs::copy(&probe, deploy.join("libandroid_base.so")).unwrap();
    let start = captured.lock().unwrap().len();
    let loaded = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(loaded, None).expect("bring-up");
    let lines = captured.lock().unwrap()[start..].to_vec();
    let position = |needle: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(needle))
            .unwrap_or_else(|| panic!("missing {needle}: {lines:?}"))
    };
    assert!(
        position("[dummy] phase=early") < position("[probe] phase=early"),
        "logging early init runs first: {lines:?}"
    );
    assert!(
        position("[probe] phase=early") < position("[dummy] phase=register_jni"),
        "a whole phase completes before the next: {lines:?}"
    );
    assert!(
        position("[probe] displayWidth=") < position("[dummy] phase=init"),
        "later phases keep the validated android_base-first order: {lines:?}"
    );
    std::mem::forget(capabilities);
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn guest_bring_up_maps_modules_and_fires_events() {
    use std::sync::{Arc, Mutex};

    // Build the android_elf_load cdylib (cargo is available in test
    // runs) and deploy it next to the loader fixture.
    // Ticket 57: the kit id covers the profile, so the member must be
    // built in THIS run's profile or the loader would (correctly) refuse
    // it as a foreign kit member.
    let built = ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load");
    let elf_lib = built.file_name().unwrap();

    let deploy = std::env::temp_dir().join(format!("rudroid-guest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(&built, deploy.join(elf_lib)).expect("deploy android_elf_load");

    // Fake gamedata: the synthetic aarch64 fixture stands in for libmain
    // and libil2cpp.
    let game_files = deploy.join("gamedata");
    let lib_dir = game_files.join("lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    let fixture = android_elf_load::fixture::build_minimal_aarch64_so();
    std::fs::write(lib_dir.join("libmain.so"), &fixture).unwrap();
    std::fs::write(lib_dir.join("libil2cpp.so"), &fixture).unwrap();

    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = \"{}\"\n",
        game_files.display()
    ))
    .unwrap();
    let plan = guest::GuestPlan::from_config(&config).expect("plan");

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let handle = events.clone();
    guest::register_module_loaded(
        rudroid_common::ModuleKind::Main,
        Arc::new(move |event: &guest::ModuleEvent| {
            handle.lock().unwrap().push(format!("{:?}@{:#x}", event.kind, event.base));
        }),
    );

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let fired = unsafe { guest::bring_up(&capabilities, &plan) }.expect("guest bring-up");
    assert_eq!(
        fired.iter().map(|e| e.kind).collect::<Vec<_>>(),
        [rudroid_common::ModuleKind::Main, rudroid_common::ModuleKind::Il2cpp]
    );
    // Registered callbacks fired for their kind.
    let seen = events.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "Main callback fired once: {seen:?}");
    assert!(seen[0].starts_with("Main@"));
    std::mem::forget(capabilities);
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn guest_bring_up_fails_actionably_without_gamedata() {
    // Plan needs game_files key.
    let config = rudroid_host_config::HostConfig::parse("").unwrap();
    assert!(matches!(
        guest::GuestPlan::from_config(&config),
        Err(guest::GuestError::MissingConfigKey(_))
    ));

    // Deployed kit without android_elf_load capability.
    let config = rudroid_host_config::HostConfig::parse("[paths]\ngame_files = \"/tmp\"\n")
        .unwrap();
    let plan = guest::GuestPlan::from_config(&config).unwrap();
    let err = unsafe { guest::bring_up(&[], &plan) }.unwrap_err();
    assert!(err.to_string().contains("android_elf_load"));

    // With the capability present but gamedata missing: names the dir.
    // Ticket 57: deploy THIS run's profile artifact — a debug-built
    // member would be (correctly) refused by a release loader as a
    // foreign kit member. This test builds its own copy (ticket 58: no
    // dependency on another test's artifact).
    let deploy = std::env::temp_dir().join(format!("rudroid-guest2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    let built = ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load");
    let elf_lib = built.file_name().unwrap();
    std::fs::copy(&built, deploy.join(elf_lib)).unwrap();
    let game_files = deploy.join("absent-gamedata");
    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = \"{}\"\n",
        game_files.display()
    ))
    .unwrap();
    let plan = guest::GuestPlan::from_config(&config).unwrap();
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap();
    let err = unsafe { guest::bring_up(&capabilities, &plan) }.unwrap_err().to_string();
    assert!(err.contains("gamedata directory") && err.contains("deploy the guest package"), "{err}");
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn missing_deploy_dir_is_actionable() {
    let err = unsafe { load_deploy_dir(Path::new("/definitely/not/here"), &Selection::All) }
        .unwrap_err();
    assert!(err.to_string().contains("deploy dir"));
}

#[test]
fn plain_file_without_descriptor_is_reported() {
    let deploy = std::env::temp_dir().join(format!("rudroid-loader2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    let name = if cfg!(target_os = "macos") {
        "notacapability.dylib"
    } else {
        "notacapability.so"
    };
    std::fs::write(deploy.join(name), b"not a real library").unwrap();
    let err = unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap_err();
    assert!(err.to_string().contains("cannot load capability"));
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
#[cfg(target_os = "linux")]
fn rejected_library_remains_pinned_after_its_constructor_runs() {
    let root = std::env::temp_dir().join(format!("rudroid-rejected-pin-{}", std::process::id()));
    std::fs::create_dir_all(root.join("kit")).unwrap();
    let marker = root.join("events");
    let source = root.join("rejected.c");
    let library = root.join("kit/librejected.so");
    let program = format!(r#"#include <stdio.h>
static void record(char event) {{ FILE *f = fopen("{}", "a"); if (f) {{ fputc(event, f); fclose(f); }} }}
__attribute__((constructor)) static void started(void) {{ record('C'); }}
__attribute__((destructor)) static void stopped(void) {{ record('D'); }}
const void *rudroid_plugin_descriptor(void) {{ return 0; }}
"#, marker.display());
    std::fs::write(&source, program).unwrap();
    assert!(std::process::Command::new("cc").args(["-shared", "-fPIC"])
        .arg(&source).arg("-o").arg(&library).status().unwrap().success());
    // SAFETY: self-built inert fixture; its only side effect is writing the
    // test marker, and its null descriptor is explicitly handled by the loader.
    assert!(unsafe { load_deploy_dir(&root.join("kit"), &Selection::All) }.is_err());
    assert_eq!(std::fs::read(&marker).unwrap(), b"C", "rejection must not run the destructor and invalidate constructor-published addresses");
    // Leave the marker directory until process exit, when the pinned native
    // destructor runs. No library resource is unloaded by this test.
}
