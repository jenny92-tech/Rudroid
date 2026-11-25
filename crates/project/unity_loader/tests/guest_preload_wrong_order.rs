//! A reversed explicit preload list must fail at its first unsatisfied import.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rudroid_unity_loader::guest;

static SERIAL: Mutex<()> = Mutex::new(());
static MODULE_EVENT_FIRED: AtomicBool = AtomicBool::new(false);

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU; use docker-rust.sh test"]
fn reversed_preload_order_fails_before_module_events() {
    panic!("requires the ARM64 guest execution backend");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[test]
fn reversed_preload_order_fails_before_module_events() {
    use rudroid_common::ModuleKind;
    use rudroid_test_support::cargo::{compile_c_fixture, ensure_workspace_cdylib};
    use rudroid_unity_loader::{load_deploy_dir, runtime, Selection};

    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    runtime::shutdown();
    let root = std::env::temp_dir().join(format!("rudroid-preload-wrong-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let deploy = root.join("kit");
    let lib_dir = root.join("game/lib/arm64-v8a");
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::create_dir_all(&lib_dir).unwrap();
    let elf = ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load");
    std::fs::copy(&elf, deploy.join("libandroid_elf_load.so")).unwrap();
    for (source, output) in [
        (
            "project/unity_loader/tests/fixtures/preload_anort.c",
            "libanort.so",
        ),
        (
            "project/unity_loader/tests/fixtures/preload_u8t.c",
            "libu8t.so",
        ),
        (
            "project/unity_loader/tests/fixtures/preload_probe.c",
            "guest.so",
        ),
    ] {
        let built = compile_c_fixture(source, &[], output);
        std::fs::copy(built, lib_dir.join(output)).unwrap();
    }

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap();
    guest::register_module_loaded(
        ModuleKind::Main,
        std::sync::Arc::new(|_| {
            MODULE_EVENT_FIRED.store(true, Ordering::SeqCst);
        }),
    );
    let plan = guest::GuestPlan {
                reserved_paths: vec![],
        defer_jni_on_load: vec![],
        game_files: root.join("game"),
        preload: vec![
            "lib/arm64-v8a/libu8t.so".to_owned(),
            "lib/arm64-v8a/libanort.so".to_owned(),
        ],
        entries: vec![guest::GuestEntry {
            kind: Some(ModuleKind::Main),
            relative_path: "lib/arm64-v8a/guest.so",
            required: true,
            gates: vec![],
        }],
    };
    let error = unsafe { guest::bring_up(&capabilities, &plan) }
        .expect_err("u8t cannot load before its provider")
        .to_string();
    assert!(error.contains("libu8t.so"), "{error}");
    assert!(error.contains("rudroid_anort_ready"), "{error}");
    assert!(
        !MODULE_EVENT_FIRED.load(Ordering::SeqCst),
        "the normal module event must not fire after a preload failure"
    );
}
