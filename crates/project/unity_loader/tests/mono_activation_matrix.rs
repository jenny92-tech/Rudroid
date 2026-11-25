//! INPUT:  real deployed unity_2022_3 + optional unity_mono cdylibs and HostConfig variants
//! OUTPUT:  regression proof for the Mono activation matrix and pure array-size contract
//! POS:    verifies capability-only, config-only, partial-section and unknown-field gates
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rudroid_common::ModuleKind;
use rudroid_host_config::HostConfig;
use rudroid_test_support::cargo::ensure_workspace_cdylib;
use rudroid_unity_common::mono_abi::MonoServiceV1;
use rudroid_unity_loader::{bring_up, host_api, load_deploy_dir, runtime, GuestPlan, Selection};

fn stage_kit(dir: &Path, members: &[&Path]) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    for artifact in members {
        std::fs::copy(artifact, dir.join(artifact.file_name().unwrap())).unwrap();
    }
}

#[test]
fn mono_activation_matrix_matches_contract() {
    let Ok(case) = std::env::var("RUDROID_TEST_MONO_MATRIX_CASE") else {
        // Build the workspace cdylibs once in the parent and hand staged kit
        // directories to isolated child processes (HostConfig is OnceLock).
        let elf_load = ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load");
        let jni_vm = ensure_workspace_cdylib("rudroid-android-jni-vm", "android_jni_vm");
        let framework = ensure_workspace_cdylib("rudroid-android-framework", "android_base");
        let catalog = ensure_workspace_cdylib(
            "rudroid-android-reply-catalog",
            "rudroid_android_reply_catalog",
        );
        let unity = ensure_workspace_cdylib("rudroid-unity-profile-2022-3", "unity_2022_3");
        let mono = ensure_workspace_cdylib("rudroid-unity-mono", "unity_mono");

        let root = std::env::temp_dir().join(format!(
            "rudroid-mono-matrix-kits-{}",
            std::process::id()
        ));
        let kit_with_mono = root.join("with_mono");
        let kit_without_mono = root.join("without_mono");
        stage_kit(
            &kit_with_mono,
            &[&elf_load, &jni_vm, &framework, &catalog, &unity, &mono],
        );
        stage_kit(
            &kit_without_mono,
            &[&elf_load, &jni_vm, &framework, &catalog, &unity],
        );

        for (case, kit) in [
            ("capability_without_config", &kit_with_mono),
            ("config_without_capability", &kit_without_mono),
            ("partial_section_fails_closed", &kit_with_mono),
            ("unknown_field_fails_closed", &kit_with_mono),
        ] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "mono_activation_matrix_matches_contract"])
                .env("RUDROID_TEST_MONO_MATRIX_CASE", case)
                .env("RUDROID_TEST_MONO_MATRIX_KIT", kit)
                .status()
                .unwrap();
            assert!(status.success(), "case failed: {case}");
        }
        let _ = std::fs::remove_dir_all(&root);
        return;
    };

    let deploy = PathBuf::from(std::env::var("RUDROID_TEST_MONO_MATRIX_KIT").unwrap());
    let root = std::env::temp_dir().join(format!(
        "rudroid-mono-matrix-{case}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let game_files = root.join("gamedata");
    std::fs::create_dir_all(&game_files).unwrap();

    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log_sink = logs.clone();
    host_api::install_log_sink(Box::new(move |tag, message| {
        log_sink.lock().unwrap().push(format!("[{tag}] {message}"));
    }));

    let extra_toml = match case.as_str() {
        "capability_without_config" => "[il2cpp_hooks]\nextra_probe = 'Sym_Extra'\n",
        "config_without_capability" => "[unity.mono]\nruntime = '/tmp/libmonobdwgc-2.0.so'\n",
        "partial_section_fails_closed" => "[unity.mono]\nruntime = '/tmp/libmonobdwgc-2.0.so'\n",
        "unknown_field_fails_closed" => "[unity.mono]\ntypo = true\n",
        other => panic!("unexpected case: {other}"),
    };

    let config = HostConfig::parse(&format!(
        "[paths]\ngame_files = {:?}\n{extra_toml}",
        game_files.to_str().unwrap()
    ))
    .unwrap();
    host_api::install_config(Some(config.clone())).unwrap();

    let loaded = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(loaded, None).expect("bring_up");
    let api = host_api::table();
    let profile = runtime::registered_profile().expect("unity_2022_3 registered profile");
    let validate = profile
        .validate_config
        .expect("unity profile exports validate_config");

    match case.as_str() {
        "capability_without_config" => {
            assert_eq!(validate(Some(api)), 0, "logs: {:?}", logs.lock().unwrap());
            // Pure array-size contract: non-empty TOML table returns 0.
            assert_eq!(api.config_len(c"il2cpp_hooks"), 0);
            // Deploying unity_mono alone must NOT strip Il2cpp from the guest plan.
            let plan = GuestPlan::from_config(&config).expect("guest plan");
            assert!(
                plan.entries.iter().any(|e| e.kind == Some(ModuleKind::Il2cpp)),
                "Il2cpp must remain in plan without [unity.mono]: {:?}",
                plan.entries
            );
            assert!(plan.reserved_paths.is_empty());
            // Service is registered, and start_bridge is a clean no-op without config.
            let svc = api
                .query_service_typed::<MonoServiceV1>()
                .expect("unity_mono registered MonoServiceV1");
            assert_eq!((svc.start_bridge.expect("start_bridge"))(Some(api)), 0);
        }
        "config_without_capability" => {
            assert_ne!(validate(Some(api)), 0);
            let joined = logs.lock().unwrap().join("\n");
            assert!(
                joined.contains("requires the optional unity_mono runtime capability"),
                "expected missing-capability log, got:\n{joined}"
            );
        }
        "partial_section_fails_closed" => {
            assert_ne!(validate(Some(api)), 0);
            let joined = logs.lock().unwrap().join("\n");
            assert!(
                joined.contains("requires paths.android_package_code"),
                "expected partial-section rejection log, got:\n{joined}"
            );
        }
        "unknown_field_fails_closed" => {
            assert_ne!(validate(Some(api)), 0);
            let joined = logs.lock().unwrap().join("\n");
            assert!(
                joined.contains("unknown field typo"),
                "expected unknown-field rejection log, got:\n{joined}"
            );
        }
        _ => unreachable!(),
    }

    std::mem::forget(capabilities);
    let _ = std::fs::remove_dir_all(&root);
}
