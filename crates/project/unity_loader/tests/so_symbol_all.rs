//! Reference `so_symbol_all` through the Host API: the loader's
//! `so_symbol_all` slot resolves a hidden function that only the guest
//! module file's `.symtab` carries (`so_symbol` misses it), and the
//! unity_2021_3 profile's Swappy invalid-interval fallback installs on that
//! target during a real exec-backend bring-up — the C++ `unity_2021_3.cpp`
//! path (`so_symbol_all` + `hook_address_detour` on the Unity module).
//!
//! Assembly: elf_load and unity_2021_3 cdylibs come from the shared
//! test-support cargo helper (this run's profile and target dir); the
//! guest library is compiled from the profile's tests/fixtures with `-O0` so its
//! exported caller keeps a real branch into the hidden function.

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

use std::ffi::{c_int, c_void, CString};
use std::sync::Mutex;

use rudroid_common::host_api::{HostApiV0, SoSymbolFn};
use rudroid_common::registry::HostStr;
use rudroid_common::ModuleKind;
use rudroid_test_support::cargo::{compile_c_fixture, ensure_workspace_cdylib};
use rudroid_unity_loader::{guest, host_api, load_deploy_dir, runtime, Selection};
use libloading::{Library, Symbol};

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn lookup(slot: SoSymbolFn, base: usize, name: &str) -> usize {
    let name = CString::new(name).unwrap();
    slot(base as *mut c_void, HostStr::new(&name))
}

#[test]
fn symtab_only_swappy_target_is_found_and_detoured() {
    runtime::shutdown();
    host_api::install_config(Some(
        rudroid_host_config::HostConfig::parse(
            "[unity]\nswappy_invalid_interval_fallback = true\n[device]\ndisplayRefreshRate = 144\n",
        )
        .unwrap(),
    )).unwrap();
    host_api::install_log_sink(Box::new(|tag, message| {
        LOGS.lock().unwrap().push(format!("{tag}: {message}"));
    }));

    let deploy = std::env::temp_dir().join(format!("rudroid-symtab-swappy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    for (package, artifact) in [
        ("rudroid-android-elf-load", "android_elf_load"),
        ("rudroid-unity-profile-2021-3", "unity_2021_3"),
    ] {
        let built = ensure_workspace_cdylib(package, artifact);
        std::fs::copy(&built, deploy.join(format!("lib{artifact}.so"))).expect("deploy capability");
    }

    // Guest: synthetic libmain/libil2cpp, the Swappy fixture as libunity.
    let lib_dir = deploy.join("gamedata/lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    let synthetic = android_elf_load::fixture::build_minimal_aarch64_so();
    for name in ["libmain.so", "libil2cpp.so"] {
        std::fs::write(lib_dir.join(name), &synthetic).unwrap();
    }
    let unity = compile_c_fixture(
        "capability/unity/profile_2021_3/tests/fixtures/rudroid_swappy_symtab.c",
        &["-O0"],
        "rudroid_swappy_symtab.so",
    );
    std::fs::copy(&unity, lib_dir.join("libunity.so")).unwrap();

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    // The profile's native phase (same deployed file → same handle as the
    // loader's): it subscribes to the Unity module event.
    let profile = unsafe { Library::new(deploy.join("libunity_2021_3.so")) }.expect("dlopen profile");
    let table: &'static HostApiV0 = host_api::table();
    // SAFETY: phase entry of the same-build kit member.
    let init: Symbol<unsafe extern "C" fn(*const HostApiV0) -> c_int> =
        unsafe { profile.get(b"rudroid_plugin_init") }.expect("init export");
    assert_eq!(unsafe { init(table) }, 0);

    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = \"{}\"\n",
        deploy.join("gamedata").display()
    ))
    .unwrap();
    let plan = guest::GuestPlan::from_config(&config).expect("plan");
    let events = unsafe { guest::bring_up(&capabilities, &plan) }.expect("guest bring-up");
    let base = events
        .iter()
        .find(|event| event.kind == ModuleKind::Unity)
        .expect("Unity module event")
        .base;

    // Dynamic symbols miss the hidden function; the .symtab fallback finds
    // it, and exported names still resolve identically through both.
    assert_eq!(lookup(table.so_symbol, base, "SwappyGL_getSwapIntervalNS"), 0);
    let target = lookup(table.so_symbol_all, base, "SwappyGL_getSwapIntervalNS");
    assert!(target > base, "the .symtab address lives in the Unity image");
    let caller = lookup(table.so_symbol, base, "rudroid_swappy_call");
    assert_ne!(caller, 0);
    assert_eq!(lookup(table.so_symbol_all, base, "rudroid_swappy_call"), caller);
    assert_eq!(lookup(table.so_symbol_all, base, "rudroid_no_such_symbol"), 0);

    let logs = LOGS.lock().unwrap().clone();
    let installed = format!("UNITY2021: installed invalid-interval fallback at {target:#x} (144 Hz)");
    assert!(logs.contains(&installed), "{logs:#?}");
    // SAFETY: exported `unsigned long long (void)` of the mapped guest.
    let call: extern "C" fn() -> u64 = unsafe { std::mem::transmute(caller) };
    assert_eq!(call(), 1_000_000_000 / 144, "UINT64_MAX becomes the 144 Hz interval");
}
