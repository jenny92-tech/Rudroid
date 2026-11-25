//! Synthetic preload-order coverage for the explicit guest dependency chain.

use std::sync::Mutex;

use rudroid_unity_loader::guest;

static SERIAL: Mutex<()> = Mutex::new(());

fn invalid_preload_config(path: &str) -> String {
    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = \".\"\nguest_preload = [\"{path}\"]\n"
    ))
    .unwrap();
    guest::GuestPlan::from_config(&config)
        .expect_err("invalid preload path must be rejected")
        .to_string()
}

#[test]
fn guest_preload_config_is_ordered_and_rejects_unsafe_paths() {
    let config = rudroid_host_config::HostConfig::parse(
        "[paths]\n\
         game_files = \".\"\n\
         guest_preload = [\n\
           \"lib/arm64-v8a/libanort.so\",\n\
           \"lib/arm64-v8a/libu8t.so\",\n\
         ]\n",
    )
    .unwrap();
    let plan = guest::GuestPlan::from_config(&config).unwrap();
    assert_eq!(
        plan.preload,
        ["lib/arm64-v8a/libanort.so", "lib/arm64-v8a/libu8t.so",]
    );

    for path in [
        "",
        "../libanort.so",
        "/libanort.so",
        "./libanort.so",
        "lib/../libanort.so",
    ] {
        let error = invalid_preload_config(path);
        assert!(
            error.contains("configured guest preload"),
            "{path:?}: {error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn bring_up_rejects_preload_escapes_before_capability_lookup() {
    use std::os::unix::fs::symlink;

    let root =
        std::env::temp_dir().join(format!("rudroid-preload-boundary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let game = root.join("game");
    std::fs::create_dir_all(game.join("lib")).unwrap();
    let outside = root.join("outside.so");
    std::fs::write(&outside, b"outside").unwrap();
    symlink(&outside, game.join("lib/escape.so")).unwrap();

    let plan = |relative: &str| guest::GuestPlan {
                reserved_paths: vec![],
        defer_jni_on_load: vec![],
        game_files: game.clone(),
        preload: vec![relative.to_owned()],
        entries: Vec::new(),
    };

    // Validation precedes capability resolution, so an empty deployment is
    // enough to prove a directly constructed plan cannot bypass the check.
    let error = unsafe { guest::bring_up(&[], &plan("lib/escape.so")) }
        .expect_err("a symlink outside game_files must fail")
        .to_string();
    assert!(error.contains("outside game_files"), "{error}");
    assert!(!error.contains("android_elf_load capability"), "{error}");

    std::fs::create_dir(game.join("lib/not-file.so")).unwrap();
    let error = unsafe { guest::bring_up(&[], &plan("lib/not-file.so")) }
        .expect_err("a directory is not a preload file")
        .to_string();
    assert!(error.contains("not a regular file"), "{error}");

    let error = unsafe { guest::bring_up(&[], &plan("lib/./escape.so")) }
        .expect_err("an interior dot component must fail")
        .to_string();
    assert!(error.contains("without `.` or `..` components"), "{error}");

    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU; use docker-rust.sh test"]
fn preload_constructors_run_before_the_normal_plan() {
    panic!("requires the ARM64 guest execution backend");
}

#[test]
fn duplicate_and_normal_plan_preloads_reject_before_capability_lookup() {
    let root = std::env::temp_dir().join(format!("rudroid-preload-duplicates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("support.so"), b"synthetic preload").unwrap();
    std::fs::write(lib.join("normal.so"), b"synthetic normal plan").unwrap();
    let plan = |preload: &[&str], entries| guest::GuestPlan {
                reserved_paths: vec![],
        defer_jni_on_load: vec![],
        game_files: root.clone(),
        preload: preload.iter().map(|name| (*name).to_owned()).collect(),
        entries,
    };
    let entry = || guest::GuestEntry {
        kind: None,relative_path: "lib/normal.so",required: false,gates: vec![],
    };
    for (candidate, expected) in [
        (plan(&["lib/support.so", "lib/support.so"], Vec::new()), "duplicates"),
        (plan(&["lib/normal.so"], vec![entry()]), "normal bring-up plan"),
    ] {
        // Empty deployment proves the entire check precedes any mapper or
        // constructor; fake bytes must never reach ELF parsing either.
        let error = unsafe { guest::bring_up(&[], &candidate) }.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("android_elf_load capability"), "{error}");
    }
    let distinct = plan(&["lib/support.so"], vec![entry()]);
    let error = unsafe { guest::bring_up(&[], &distinct) }.unwrap_err().to_string();
    assert!(error.contains("android_elf_load capability"), "distinct files pass preflight: {error}");
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn preload_symlink_aliases_reject_duplicates_and_normal_plan_overlap() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!("rudroid-preload-aliases-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(root.join("lib/source.so"), b"synthetic preload").unwrap();
    symlink("source.so", root.join("lib/alias.so")).unwrap();
    let plan = |preload: &[&str], entries| guest::GuestPlan {
                reserved_paths: vec![],
        defer_jni_on_load: vec![],
        game_files: root.clone(),
        preload: preload.iter().map(|name| (*name).to_owned()).collect(),
        entries,
    };
    let normal = guest::GuestEntry {
        kind: None,relative_path: "lib/alias.so",required: false,gates: vec![],
    };
    for (candidate, expected) in [
        (plan(&["lib/source.so", "lib/alias.so"], Vec::new()), "duplicates"),
        (plan(&["lib/source.so"], vec![normal]), "normal bring-up plan"),
    ] {
        let error = unsafe { guest::bring_up(&[], &candidate) }.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("android_elf_load capability"), "{error}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[test]
fn preload_constructors_run_before_the_normal_plan() {
    use rudroid_common::ModuleKind;
    use rudroid_test_support::cargo::{compile_c_fixture, ensure_workspace_cdylib};
    use rudroid_unity_loader::{load_deploy_dir, runtime, Selection};

    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    runtime::shutdown();
    let root = std::env::temp_dir().join(format!("rudroid-preload-order-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let deploy = root.join("kit");
    let lib_dir = root.join("game/lib/arm64-v8a");
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::create_dir_all(&lib_dir).unwrap();
    let elf = ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load");
    std::fs::copy(&elf, deploy.join("libandroid_elf_load.so")).unwrap();
    for (source, output, flags) in [
        (
            "project/unity_loader/tests/fixtures/preload_anort.c",
            "libanort.so",
            &["-Wl,-soname,libanort.so"][..],
        ),
        (
            "project/unity_loader/tests/fixtures/preload_u8t.c",
            "libu8t.so",
            &["-Wl,-soname,libu8t.so"][..],
        ),
        (
            "project/unity_loader/tests/fixtures/preload_probe.c",
            "guest.so",
            &[][..],
        ),
    ] {
        let built = compile_c_fixture(source, flags, output);
        std::fs::copy(built, lib_dir.join(output)).unwrap();
    }

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap();
    let plan = guest::GuestPlan {
                reserved_paths: vec![],
        defer_jni_on_load: vec![],
        game_files: root.join("game"),
        preload: vec![
            "lib/arm64-v8a/libanort.so".to_owned(),
            "lib/arm64-v8a/libu8t.so".to_owned(),
        ],
        entries: vec![guest::GuestEntry {
            kind: Some(ModuleKind::Main),
            relative_path: "lib/arm64-v8a/guest.so",
            required: true,
            gates: vec![],
        }],
    };
    let events = unsafe { guest::bring_up(&capabilities, &plan) }.expect("ordered preload chain");
    assert_eq!(
        events.len(),
        1,
        "preloads are support libraries, not module events"
    );
    let probe = runtime::guest_exec_symbol(events[0].base, "rudroid_preload_probe")
        .expect("normal module export");
    // SAFETY: the symbol belongs to the still-mapped guest image and has the
    // fixture's documented C signature.
    let probe: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(probe) };
    assert_eq!(unsafe { probe() }, 7);
}
