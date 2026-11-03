//! Ticket 84 acceptance: ONE load per guest module. The exec backend is the
//! production loader — the event base, the exec symbol table and any
//! constructor/JNI_OnLoad execution all refer to the same image, and the
//! buffer backend no longer accompanies an execution load.
//!
//! Instrumentation: `rudroid_elf_load_stats` (buffer vs exec load-call
//! counters, ticket 84) read from the SAME deployed cdylib the loader
//! dlopens, so the deltas measure the real bring-up path. The red evidence
//! ran on the pre-84 tree: data_loads == kinded-module count (double load);
//! green: data_loads == 0 and exec_loads == kinded count.
//!
//! ARM64 Linux container for the exec path; the fallback (non-Linux)
//! branch is exercised by the rest of the suite on any host.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rudroid_unity_loader::{guest, load_deploy_dir, runtime, Selection};
use rudroid_test_support::cargo::{compile_c_fixture, ensure_workspace_cdylib};
use libloading::{Library, Symbol};

static SERIAL: Mutex<()> = Mutex::new(());

/// Ticket 81 rework: assembly goes through the shared 73/75 helper —
/// CARGO_TARGET_DIR (relative or absolute) and this run's profile, always
/// rebuilt so a stale cdylib can never be picked up.
fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    ensure_workspace_cdylib(package, artifact)
}

/// Snapshot the ticket-84 load counters from the SAME cdylib instance the
/// loader dlopens (same path → same handle → same statics).
struct LoadStats {
    library: Library,
}

impl LoadStats {
    fn open(deployed: &Path) -> LoadStats {
        // SAFETY: the path points at the deployed cdylib; the handle lives
        // as long as this struct.
        let library = unsafe { Library::new(deployed) }.expect("dlopen android_elf_load");
        LoadStats { library }
    }

    fn read(&self) -> (usize, usize) {
        // SAFETY: the export exists in the same-build kit member; out
        // pointers are valid for the call.
        unsafe {
            let stats: Symbol<
                unsafe extern "C" fn(*mut usize, *mut usize),
            > = self.library.get(b"rudroid_elf_load_stats").expect("stats export");
            let mut data = 0usize;
            let mut exec = 0usize;
            stats(&mut data, &mut exec);
            (data, exec)
        }
    }
}

fn synthetic_image() -> Vec<u8> {
    android_elf_load::fixture::build_minimal_aarch64_so()
}

fn gamedata_with(deploy: &Path, entries: &[(&str, PathBuf)]) -> PathBuf {
    let game_files = deploy.join("gamedata");
    let lib_dir = game_files.join("lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    for (name, source) in entries {
        std::fs::copy(source, lib_dir.join(name)).expect("stage module");
    }
    game_files
}

fn synthetic_stage(deploy: &Path) -> PathBuf {
    let image = synthetic_image();
    let dir = deploy.join("synthetic");
    std::fs::create_dir_all(&dir).unwrap();
    let mut staged = Vec::new();
    for name in ["libmain.so", "libil2cpp.so", "libunity.so"] {
        let path = dir.join(name);
        std::fs::write(&path, &image).expect("write synthetic module");
        staged.push((name, path));
    }
    gamedata_with(deploy, &staged)
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
fn single_image_loads_are_the_only_loads() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let elf_load = ensure_cdylib("rudroid-android-elf-load", "android_elf_load");
    let deploy = std::env::temp_dir().join(format!("rudroid-t84-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(&elf_load, deploy.join("libandroid_elf_load.so")).expect("deploy elf_load");
    let game_files = synthetic_stage(&deploy);

    let stats = LoadStats::open(&deploy.join("libandroid_elf_load.so"));
    let (data_before, exec_before) = stats.read();

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let events = unsafe { guest::bring_up(&capabilities, &guest_plan(&game_files)) }
        .expect("guest bring-up");

    let (data_after, exec_after) = stats.read();
    assert_eq!(events.len(), 3, "three kinded modules fire events");
    assert_eq!(
        exec_after - exec_before,
        3,
        "exactly one exec load per kinded module"
    );
    assert_eq!(
        data_after - data_before,
        0,
        "the buffer backend must NOT accompany an execution load (ticket 84)"
    );
    for event in &events {
        assert_ne!(event.base, 0, "event base is a real exec image base");
        // Address identity: the event base resolves symbols in the SAME
        // image the loader registered for execution.
        let symbol =
            runtime::guest_exec_symbol(event.base, "rudroid_answer").expect("export via event base");
        assert!(
            symbol >= event.base,
            "export address must live inside the event's own image \
             (base={:#x}, symbol={symbol:#x})",
            event.base
        );
    }
}

/// A `.init_array` constructor runs exactly once in the single image (the
/// ticket-79 constructor fixture, compiled here with the container's gcc).
#[test]
fn constructor_runs_once_in_the_single_image() {
    let init_so = compile_c_fixture(
        "project/unity_loader/tests/fixtures/rudroid79_init.c",
        &[],
        "rudroid79_init.so",
    );
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let elf_load = ensure_cdylib("rudroid-android-elf-load", "android_elf_load");
    let deploy = std::env::temp_dir().join(format!("rudroid-t84-ctor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(&elf_load, deploy.join("libandroid_elf_load.so")).expect("deploy elf_load");
    let init_path = init_so;
    let synthetic = deploy.join("synthetic-minimal");
    std::fs::create_dir_all(&synthetic).unwrap();
    std::fs::write(synthetic.join("libil2cpp.so"), synthetic_image()).expect("stage libil2cpp");
    std::fs::write(synthetic.join("libunity.so"), synthetic_image()).expect("stage libunity");
    let game_files = gamedata_with(
        &deploy,
        &[
            ("libmain.so", init_path),
            ("libil2cpp.so", synthetic.join("libil2cpp.so")),
            ("libunity.so", synthetic.join("libunity.so")),
        ],
    );

    let stats = LoadStats::open(&deploy.join("libandroid_elf_load.so"));
    let (data_before, exec_before) = stats.read();

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let events = unsafe { guest::bring_up(&capabilities, &guest_plan(&game_files)) }
        .expect("guest bring-up");
    assert_eq!(events.len(), 3, "libmain + libil2cpp + libunity");
    let base = events[0].base;

    let (data_after, exec_after) = stats.read();
    assert_eq!(data_after - data_before, 0, "single image: no data load");
    assert_eq!(exec_after - exec_before, 3, "single image: one exec load per module");

    // The constructor ran during that one load (slot1 = slot0 + 1 = 41); a
    // second image would hold linker zeros in ITS copy, and a double exec
    // load would double-increment through separate statics — the value 41
    // pins "exactly one exec image, constructor once".
    let getter = |name: &str| {
        let address = runtime::guest_exec_symbol(base, name).expect("export resolves");
        // SAFETY: the export is the probe's getter on the exec image.
        let f: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        unsafe { f() }
    };
    assert_eq!(getter("rudroid_slot0"), 40, "data loaded verbatim");
    assert_eq!(
        getter("rudroid_slot1"),
        41,
        "constructor ran exactly once in the single image"
    );
}

/// A failed later module must not take earlier images with it: the error is
/// explicit, exactly one image (the first module) stays registered and
/// resolvable (ownership contract), and the failed module publishes no
/// half-initialized registration.
#[test]
fn failed_module_keeps_earlier_images_pinned() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let elf_load = ensure_cdylib("rudroid-android-elf-load", "android_elf_load");
    let deploy = std::env::temp_dir().join(format!("rudroid-t84-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(&elf_load, deploy.join("libandroid_elf_load.so")).expect("deploy elf_load");

    let game_files = synthetic_stage(&deploy);
    // Corrupt the SECOND module (required, kinded): the first maps fine.
    std::fs::write(
        game_files.join("lib/arm64-v8a/libil2cpp.so"),
        b"not an elf image at all",
    )
    .expect("corrupt libil2cpp");

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let error = unsafe { guest::bring_up(&capabilities, &guest_plan(&game_files)) }
        .expect_err("corrupt required module must fail bring-up");
    assert!(error.to_string().contains("libil2cpp"), "{error}");

    // Exactly the FIRST module's image is registered; the failed module
    // published nothing.
    let bases = runtime::guest_exec_bases();
    assert_eq!(bases.len(), 1, "only the first module stays registered");
    assert!(
        runtime::guest_exec_symbol(bases[0], "rudroid_answer").is_some(),
        "the surviving image must stay resolvable at its registered base"
    );
}
