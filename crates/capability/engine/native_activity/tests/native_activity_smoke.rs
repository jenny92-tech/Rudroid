//! INPUT:  zero-Unity kit (probe android_base, hooks, elf_load, engine profile) and C guest fixture
//! OUTPUT:  EngineProfile prepare→guest→bootstrap→run observing fixture create/destroy flags
//! POS:    proves host-core EngineProfile path without any Unity capability
#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Mutex;

use rudroid_host_config::HostConfig;
use rudroid_host_core::{
    bring_up, guest, host_api, load_deploy_dir, runtime, GuestPlan, Selection,
};
use rudroid_test_support::cargo::{
    build_loader_fixture, compile_c_fixture, ensure_workspace_cdylib,
};

static SERIAL: Mutex<()> = Mutex::new(());

fn stage(dir: &Path, artifact: &Path, name: &str) {
    std::fs::copy(artifact, dir.join(name)).unwrap();
}

#[test]
fn zero_unity_kit_runs_native_activity_entrypoint() {
    let _guard = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let elf = ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load");
    let profile = ensure_workspace_cdylib(
        "rudroid-engine-native-activity",
        "engine_native_activity",
    );
    let probe = build_loader_fixture("probe_capability", "rudroid_probe_capability");
    let hooks = build_loader_fixture("hooks_capability", "android_hooks_fixture");
    let guest_so = compile_c_fixture(
        "capability/engine/native_activity/tests/fixtures/fake_native_activity.c",
        &[],
        "libfake_native_activity.so",
    );

    let deploy = std::env::temp_dir().join(format!(
        "rudroid-na-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    stage(&deploy, &elf, "libandroid_elf_load.so");
    stage(&deploy, &probe, "libandroid_base.so");
    stage(&deploy, &hooks, "libandroid_hooks_fixture.so");
    stage(&deploy, &profile, "libengine_native_activity.so");

    for name in ["libandroid_base.so", "libandroid_elf_load.so", "libengine_native_activity.so", "libandroid_hooks_fixture.so"] {
        assert!(
            !name.contains("unity") && !name.contains("il2cpp") && !name.contains("gamepatch"),
            "smoke kit must stay free of Unity members: {name}"
        );
    }

    let game = deploy.join("gamedata");
    let lib = game.join("lib/arm64-v8a");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::copy(&guest_so, lib.join("libgame.so")).unwrap();

    let config_text = format!(
        "[paths]\ngame_files = {:?}\n[native_activity]\nlibrary = \"lib/arm64-v8a/libgame.so\"\n",
        game.display()
    );
    let config = HostConfig::parse(&config_text).unwrap();
    host_api::install_config(Some(config.clone())).unwrap();

    // SAFETY: smoke owns the temporary same-kit deploy directory for process life.
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let ids: Vec<_> = capabilities.iter().map(|c| c.descriptor.id.as_str()).collect();
    assert!(
        ids.iter().all(|id| !id.contains("unity") && *id != "il2cpp_patch" && !id.contains("gamepatch")),
        "capability ids must exclude Unity/il2cpp/gamepatch: {ids:?}"
    );
    assert!(ids.contains(&"engine_native_activity"));
    assert!(ids.contains(&"android_base"));
    assert!(ids.contains(&"android_elf_load"));

    let capabilities = bring_up(capabilities, None).expect("phases");
    if let Some(profile) = runtime::registered_profile() {
        if let Some(validate) = profile.validate_config {
            assert_eq!(validate(Some(host_api::table())), 0);
        }
    } else {
        panic!("engine_native_activity did not register its EngineProfile");
    }

    runtime::prepare_engine_runtime(&capabilities).expect("prepare");
    let plan = GuestPlan::from_config(&config).expect("guest plan");
    assert!(
        plan.entries.iter().any(|e| e.kind == Some(rudroid_common::ModuleKind::Main)),
        "plan must include Main from engine build_guest_plan: {:?}",
        plan.entries
    );
    let events = unsafe { guest::bring_up(&capabilities, &plan) }.expect("guest");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, rudroid_common::ModuleKind::Main);

    runtime::bootstrap_engine_runtime(&capabilities, "lib/arm64-v8a").expect("bootstrap");
    let completion = runtime::run_engine_runtime(&capabilities).expect("run");
    assert_eq!(completion, runtime::EngineRunCompletion::Stopped);

    let base = events[0].base;
    let created = runtime::guest_exec_symbol(base, "rudroid_na_probe_created").expect("created probe");
    let destroyed =
        runtime::guest_exec_symbol(base, "rudroid_na_probe_destroyed").expect("destroyed probe");
    // SAFETY: fixture exports `int(void)` counters in the mapped guest image.
    let created: extern "C" fn() -> i32 = unsafe { std::mem::transmute(created) };
    let destroyed: extern "C" fn() -> i32 = unsafe { std::mem::transmute(destroyed) };
    assert_eq!(created(), 1, "ANativeActivity_onCreate must run");
    assert_eq!(destroyed(), 1, "guest onDestroy must run");

    std::mem::forget(capabilities);
    let _ = std::fs::remove_dir_all(&deploy);
}
