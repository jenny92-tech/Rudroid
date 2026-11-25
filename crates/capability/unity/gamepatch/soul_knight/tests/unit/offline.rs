//! Policy regressions; no game binaries, managed execution or device needed.
use super::*;

#[test]
fn source_wide_selection_requires_exact_no_near_and_preserves_other_results() {
    use arm64_detour::DetourError;
    let attempts = std::cell::Cell::new(0);
    let wide = || { attempts.set(attempts.get() + 1); Ok(99usize) };
    assert_eq!(select_no_near(0x1000, Ok(7), wide), Ok(7));
    for error in [DetourError::NoNearTrampoline { target: 0x1004 }, DetourError::UnsupportedTarget,
        DetourError::Unaligned(0x1001), DetourError::NullDestination,
        DetourError::Protection("fixture protection failure".into()),
        DetourError::UnsupportedPrologue { instruction: 0x94000000, pc_offset: 0x1000 },
        DetourError::ScratchRegisterConflict { instruction: 0xd2800011, pc_offset: 0x1000 }] {
        assert_eq!(select_no_near(0x1000, Err(error.clone()), wide), Err(error));
    }
    assert_eq!(attempts.get(), 0);
    assert_eq!(select_no_near(0x1000, Err(DetourError::NoNearTrampoline { target: 0x1000 }), wide), Ok(99));
    assert_eq!(attempts.get(), 1);
}

#[test]
fn webview_metadata_budget_reserves_a_bounded_terminal_marker() {
    let mut budget = WebViewMetadataBudget::default();
    for _ in 0..63 { assert!(budget.accept("WebViewMetadata declaration")); }
    assert!(!budget.accept("WebViewMetadata overflow"));
    assert!(budget.truncated);
    let mut budget = WebViewMetadataBudget::default();
    assert!(!budget.accept(&"x".repeat(1537)));
    assert_eq!((budget.lines, budget.bytes), (0, 0));
    let mut budget = WebViewMetadataBudget::default();
    while budget.accept(&"x".repeat(1536)) {}
    assert!(budget.bytes + 128 <= 24 * 1024);
    assert!(budget.lines < 63);
}

#[test]
fn webview_metadata_method_scope_is_fixed_and_never_selects_business_calls() {
    assert!(webview_method_selected(0, "get_WebviewSetting"));
    assert!(webview_method_selected(0, "set_WebviewSetting"));
    assert!(webview_method_selected(2, "SetWebViewDefaultSetting"));
    assert!(webview_method_selected(3, "GetOrCreateUniWebView"));
    for index in 0..4 {
        assert!(!webview_method_selected(index, "Bootstrap"));
        assert!(!webview_method_selected(index, "ShowOrCreateWebview"));
        assert!(!webview_method_selected(index, "SetSdkInited"));
    }
    assert!(!webview_method_selected(1, ".ctor"));
}

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const ID: &str = "rudroid-offline-80700-0123456789abcdef0123456789abcdef";
static NEXT: AtomicUsize = AtomicUsize::new(0);
static REMOTE_CALLS: AtomicUsize = AtomicUsize::new(0);
static UPDATE_CALLS: AtomicUsize = AtomicUsize::new(0);

#[test]
fn offline_policy_closes_cloud_restore_and_preserves_the_other_seven_entries() {
    assert_eq!(OFFLINE_ENTRIES.len(), 8);
    assert_eq!(OFFLINE_ENTRIES.iter().find(|(name, _)| *name == c"FallbackCloudPackageRestore")
        .map(|(_, enabled)| *enabled), Some(false));
    for name in [c"MultiPlayer", c"MultiPlayerRemote", c"Sandbox", c"Mall", c"CheckIn", c"Review", c"NewAnnouncement"] {
        assert_eq!(OFFLINE_ENTRIES.iter().find(|(actual, _)| *actual == name)
            .map(|(_, enabled)| *enabled), Some(true), "{}", name.to_string_lossy());
    }
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rudroid-soul-offline-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn game(&self) -> PathBuf {
        let path = self.0.join("original"); std::fs::create_dir_all(&path).unwrap(); path
    }
    fn roots(profile: &Path) -> Vec<PathBuf> {
        ["data", "files", "external", "cache"].map(|p| profile.join(p)).to_vec()
    }
}
impl Drop for Directory {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[test]
fn disabled_mode_does_not_read_files_or_register_callbacks() {
    extern "C" fn disabled(_: rudroid_common::registry::HostStr<'_>, fallback: i32) -> i32 {
        assert_eq!(fallback, 0); 0
    }
    let host = HostApiV0 { config_get_bool: disabled, ..HostApiV0::stubbed() };
    assert!(arm(&host).is_ok()); // Every unrelated stub would panic if called.
}

#[test]
fn enabled_mode_requires_explicit_il2cpp_producer_before_file_access() {
    extern "C" fn enabled(_: rudroid_common::registry::HostStr<'_>, _: i32) -> i32 { 1 }
    extern "C" fn no_capability(_: rudroid_common::registry::HostStr<'_>) -> i32 { 0 }
    let host = HostApiV0 { config_get_bool: enabled, has_capability: no_capability,
        ..HostApiV0::stubbed() };
    assert_eq!(arm(&host), Err("offline mode requires explicitly selected il2cpp_patch".to_owned()));
}

#[test]
fn independent_profile_is_stable_and_does_not_import_or_replace_a_save() {
    let directory = Directory::new();
    let game = directory.game();
    let original = game.join("existing.bin");
    std::fs::write(&original, b"original fixture").unwrap();
    let profile = directory.0.join("profile");
    let writable = Directory::roots(&profile);
    prepare_profile(&profile, &game, ID, &writable).unwrap();
    let generated = writable[0].join("generated.bin");
    std::fs::write(&generated, b"new local fixture").unwrap();
    prepare_profile(&profile, &game, ID, &writable).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"original fixture");
    assert_eq!(std::fs::read(&generated).unwrap(), b"new local fixture");
    assert!(prepare_profile(&profile, &game,
        "rudroid-offline-80700-ffffffffffffffffffffffffffffffff", &writable).is_err());
    assert_eq!(std::fs::read(&generated).unwrap(), b"new local fixture");
}

#[test]
fn an_existing_unmarked_directory_is_not_adopted() {
    let directory = Directory::new();
    let game = directory.game();
    let profile = directory.0.join("unmarked");
    std::fs::create_dir_all(&profile).unwrap();
    let previous = profile.join("previous.bin");
    std::fs::write(&previous, b"preserve me").unwrap();
    assert!(prepare_profile(&profile, &game, ID, &Directory::roots(&profile)).is_err());
    assert!(!profile.join(MARKER).exists());
    assert_eq!(std::fs::read(previous).unwrap(), b"preserve me");
}

#[test]
fn profile_and_writable_roots_cannot_adopt_original_inputs_or_escape() {
    let directory = Directory::new();
    let game = directory.game();
    assert!(prepare_profile(&game, &game, ID, &Directory::roots(&game)).is_err());
    assert!(!game.join(MARKER).exists());
    let nested = game.join("nested-profile");
    assert!(prepare_profile(&nested, &game, ID, &Directory::roots(&nested)).is_err());
    assert!(!nested.exists(), "reject before creating a directory inside original inputs");
    let profile = directory.0.join("profile");
    let outside = directory.0.join("outside").join("new");
    let mut writable = Directory::roots(&profile); writable[0] = outside.clone();
    assert!(prepare_profile(&profile, &game, ID, &writable).is_err());
    assert!(!outside.exists(), "do not create an escaped writable directory first");
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_without_writing_the_destination() {
    let directory = Directory::new();
    let game = directory.game();
    let profile = directory.0.join("profile");
    let mut writable = Directory::roots(&profile);
    prepare_profile(&profile, &game, ID, &writable).unwrap();
    std::os::unix::fs::symlink(&game, profile.join("escape")).unwrap();
    writable[0] = profile.join("escape").join("new-save");
    assert!(prepare_profile(&profile, &game, ID, &writable).is_err());
    assert!(!game.join("new-save").exists());
}

#[test]
fn local_identity_and_file_read_bounds_are_exact() {
    assert!(local_id(ID));
    for id in ["", "existing-account", "rudroid-offline-80700-0123456789abcdef",
        "rudroid-offline-80700-0123456789abcdef0123456789abcdeF",
        "rudroid-offline-80700-0123456789abcdef0123456789abcdef\n"] {
        assert!(!local_id(id));
    }
    let directory = Directory::new();
    let path = directory.0.join("manifest-fixture");
    std::fs::write(&path, b"fixed").unwrap();
    assert_eq!(bounded_file(&path, 5).unwrap(), b"fixed");
    assert!(bounded_file(&path, 4).is_err());
    assert!(bounded_file(&path, 6).is_err());
}

#[test]
fn network_gate_requires_a_distinct_namespace_with_no_external_interface() {
    let header = "Inter-|   Receive                                                |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n";
    let loopback = "    lo: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n";
    let devices = format!("{header}{loopback}");
    assert!(isolated_network("net:[2]", "net:[1]", &devices));
    assert!(!isolated_network("net:[1]", "net:[1]", &devices));
    assert!(!isolated_network("net:[01]", "net:[1]", &devices));
    for invalid in ["", "mnt:[2]", "net:[]", "net:[0]", "net:[-1]", "net:[18446744073709551616]"] {
        assert!(!isolated_network(invalid, "net:[1]", &devices));
    }
    for invalid in [String::new(), header.to_owned(), format!("{header}malformed"),
        format!("{header}{loopback}eth0: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"),
        format!("{header}{loopback}{loopback}"),
        format!("{header}lo: 0 0 0\n")] {
        assert!(!isolated_network("net:[2]", "net:[1]", &invalid));
    }
}

#[test]
fn offline_trace_defaults_quiet_without_advancing_any_budget() {
    let trace = OfflineTrace::default();
    assert!(trace.provider(0, 0).is_none());
    assert!(trace.title(0).is_none());
    assert!(trace.observe(SceneObservation::Stage("update manager-not-cached")).is_none());
    assert!(trace.heartbeat(false).is_none());
    assert!(!trace.wants_scene());
    assert!(!trace.wants_manager());
    assert!(trace.observe_manager(ManagerObservation::Stage("manager-update owner-mismatch")).is_none());
    assert_eq!(trace.providers.load(Ordering::Relaxed), 0);
    assert_eq!(trace.title.load(Ordering::Relaxed), 0);
    assert_eq!(trace.frames.load(Ordering::Relaxed), 0);
    assert_eq!(trace.scene.lock().unwrap().lines, 0);
    assert_eq!(trace.manager.lock().unwrap().lines, 0);
}

#[test]
fn passive_manager_observations_deduplicate_and_stop_at_sixteen_changes() {
    let trace = OfflineTrace { enabled: true, ..OfflineTrace::default() };
    let first = ManagerObservation::Stage("manager-update manager-not-cached");
    assert!(trace.observe_manager(first.clone()).is_some());
    assert!(trace.observe_manager(first).is_none());
    for index in 1..16 {
        let line = trace.observe_manager(ManagerObservation::State { current_type: format!("FixtureState{index}"),
            stopped: index % 2 == 0, ready: [true, false, true] }).unwrap();
        assert!(line.contains("init=true resources=false data=true"));
    }
    assert!(!trace.wants_manager());
    assert!(trace.observe_manager(ManagerObservation::Stage("manager-update observation-unavailable")).is_none());
    assert_eq!(trace.manager.lock().unwrap().lines, 16);
    assert_eq!(trace.scene.lock().unwrap().lines, 0, "passive output does not consume scene diagnostics");
    assert_eq!(trace.providers.load(Ordering::Relaxed), 0);
    assert_eq!(trace.title.load(Ordering::Relaxed), 0, "observing cannot claim/invoke Title");
}

#[test]
fn offline_provider_trace_has_only_fifteen_fixed_once_labels() {
    let trace = OfflineTrace { enabled: true, ..OfflineTrace::default() };
    let mut labels = Vec::new();
    for source in [0, 1, 2, 3, 5] {
        for phase in 0..3 {
            labels.push(trace.provider(source, phase).unwrap());
            assert!(trace.provider(source, phase).is_none());
        }
    }
    assert_eq!(labels.len(), 15);
    assert!(labels.iter().all(|label| label.starts_with("provider ")));
    assert!(trace.provider(4, 0).is_none(), "state Update is not a remote provider");
    assert!(trace.provider(0, 3).is_none());
}

#[test]
fn offline_scene_observation_deduplicates_and_caps_without_claiming_entry() {
    let trace = OfflineTrace { enabled: true, ..OfflineTrace::default() };
    let scene = SceneOnce::default();
    let first = SceneObservation::Stage("update manager-not-cached");
    assert!(trace.observe(first.clone()).is_some());
    assert!(trace.observe(first).is_none());
    for index in 1..16 {
        assert!(trace.observe(SceneObservation::State { current_type: format!("FixtureState{index}"),
            ready: [true, false, false], receiver_current: false }).is_some());
    }
    assert!(!trace.wants_scene());
    assert!(trace.observe(SceneObservation::Stage("update observation-unavailable")).is_none());
    assert_eq!(trace.scene.lock().unwrap().lines, 16);
    assert!(!scene.claimed(), "diagnostic state is independent of the scene gate");
    assert_eq!(scene.run(Ok(Some(())), |_| Err("original managed failure")), Err("original managed failure"));
}

#[test]
fn offline_heartbeat_works_without_managed_runtime_and_has_two_checkpoints() {
    let trace = OfflineTrace { enabled: true, ..OfflineTrace::default() };
    trace.update_hits.store(7, Ordering::Relaxed); trace.update_returns.store(6, Ordering::Relaxed);
    trace.manager_hits.store(9, Ordering::Relaxed); trace.manager_returns.store(8, Ordering::Relaxed);
    assert!(trace.provider(2, 0).is_some());
    assert!(trace.provider(5, 0).is_some());
    assert_eq!(trace.title(0), Some("Title invoke-attempt"));
    assert_eq!(trace.title(1), Some("Title invoke-returned"));
    assert!(trace.title(1).is_none());
    let lines: Vec<_> = (0..1500).filter_map(|_| trace.heartbeat(true)).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("frame=120 "));
    assert!(lines[1].contains("frame=1200 "));
    for line in lines {
        assert!(line.contains("update-hit=7 update-return=6 source-seen=20"));
        assert!(line.contains("title-claimed=true title-attempted=true title-returned=true"));
        assert!(line.contains("manager-hit=9 manager-return=8"));
    }
}

struct SceneRoots(Arc<AtomicUsize>);
impl Drop for SceneRoots {
    fn drop(&mut self) { self.0.fetch_add(1, Ordering::Relaxed); }
}

#[test]
fn scene_entry_waits_without_claiming_and_propagates_invalid_observations() {
    let scene = SceneOnce::default();
    assert_eq!(scene.run(Ok(None::<()>), |_| panic!("not ready")), Ok(false));
    for reason in ["wrong owner", "stopping", "readiness metadata mismatch", "invalid Boolean"] {
        assert_eq!(scene.run::<()>(Err(reason), |_| panic!("invalid gate")), Err(reason));
        assert!(!scene.claimed());
    }
    let mut calls = 0;
    assert_eq!(scene.run(Ok(Some(())), |_| { calls += 1; Ok(()) }), Ok(true));
    assert_eq!(scene.run(Ok(Some(())), |_| { calls += 1; Ok(()) }), Ok(false));
    assert_eq!(calls, 1);
}

#[test]
fn scene_entry_claim_precedes_reentrant_invocation_and_roots_outlive_the_call() {
    let scene = SceneOnce::default();
    let dropped = Arc::new(AtomicUsize::new(0));
    assert_eq!(scene.run(Ok(Some(SceneRoots(dropped.clone()))), |roots| {
        assert!(scene.claimed());
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(scene.run(Ok(Some(SceneRoots(dropped.clone()))), |_| panic!("scene reentered")), Ok(false));
        assert_eq!(dropped.load(Ordering::Relaxed), 1, "rejected nested roots are released");
        assert_eq!(roots.0.load(Ordering::Relaxed), 1, "outer roots remain live");
        Ok(())
    }), Ok(true));
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
}

#[test]
fn scene_entry_exception_releases_roots_and_is_never_retried() {
    let scene = SceneOnce::default();
    let dropped = Arc::new(AtomicUsize::new(0));
    assert_eq!(scene.run(Ok(Some(SceneRoots(dropped.clone()))), |_| Err("managed invocation failed")),
        Err("managed invocation failed"));
    assert!(scene.claimed());
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert_eq!(scene.run(Ok(Some(SceneRoots(dropped.clone()))), |_| panic!("failed scene retried")), Ok(false));
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
}

unsafe extern "C-unwind" fn original_update(receiver: Ptr, method: Ptr) {
    assert_eq!(receiver as usize, 0x1100);
    assert_eq!(method as usize, 0x2200);
    UPDATE_CALLS.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn state_update_keeps_the_original_call_and_arguments_without_an_offline_runtime() {
    // No managed object is dereferenced: this callable sentinel proves the
    // original executes before optional runtime/failure/scene gating.
    // SAFETY: exact C-unwind Void(instance, MethodInfo) test function.
    unsafe { UPDATE_ORIGINAL.set(original_update as UpdateSource as usize); }
    UPDATE_CALLS.store(0, Ordering::Relaxed);
    // SAFETY: sentinel consumes these two values only as numeric identities.
    unsafe { update_hook(0x1100usize as Ptr, 0x2200usize as Ptr); }
    assert_eq!(UPDATE_CALLS.load(Ordering::Relaxed), 1);
    UPDATE_ORIGINAL.clear();
}

#[test]
fn passive_manager_hook_preserves_original_arguments_and_unwind_without_a_runtime() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C-unwind" fn original(receiver: Ptr, method: Ptr) {
        assert_eq!(method as usize, 0x4400);
        CALLS.fetch_add(1, Ordering::Relaxed);
        if receiver as usize == 0x3301 { panic!("original Update unwind sentinel"); }
        assert_eq!(receiver as usize, 0x3300);
    }
    // SAFETY: exact original C-unwind ABI; both numeric arguments are consumed
    // only by this sentinel, with no managed-object dereference or attachment.
    unsafe {
        MANAGER_UPDATE_ORIGINAL.set(original as UpdateSource as usize);
        manager_update_hook(0x3300usize as Ptr, 0x4400usize as Ptr);
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: same sentinel ABI; only its Rust unwind is caught by the test.
        unsafe { manager_update_hook(0x3301usize as Ptr, 0x4400usize as Ptr) };
    });
    MANAGER_UPDATE_ORIGINAL.clear();
    assert!(result.is_err(), "the observer cannot swallow the original unwind");
    assert_eq!(CALLS.load(Ordering::Relaxed), 2);
}

#[test]
fn platform_source_requires_one_complete_readable_executable_mapping() {
    let maps = "1000-2000 r--p 0 00:00 0\n2000-3000 r-xp 0 00:00 0\n3000-4000 r-xp 0 00:00 0\n";
    assert!(executable_source_range(maps, 0x2000, 32));
    assert!(!executable_source_range(maps, 0x1000, 32));
    assert!(!executable_source_range(maps, 0x2ff0, 32), "split mappings reject");
    assert!(!executable_source_range(maps, 0x2000, 0));
    assert!(!executable_source_range(maps, usize::MAX - 1, 32));
    assert!(!executable_source_range("broken mapping", 0x2000, 32));
}

unsafe extern "C-unwind" fn remote_reference(_: Ptr, _: Ptr) -> Ptr {
    REMOTE_CALLS.fetch_add(1, Ordering::Relaxed); 1usize as Ptr
}
unsafe extern "C-unwind" fn remote_time(_: i32, _: Ptr) -> Ptr {
    REMOTE_CALLS.fetch_add(1, Ordering::Relaxed); 1usize as Ptr
}
unsafe extern "C-unwind" fn remote_login(_: Ptr, _: Ptr) {
    REMOTE_CALLS.fetch_add(1, Ordering::Relaxed);
}
unsafe extern "C-unwind" fn remote_android_id(_: Ptr) -> Ptr {
    REMOTE_CALLS.fetch_add(1, Ordering::Relaxed); 1usize as Ptr
}

#[test]
fn source_failures_return_no_success_and_never_call_remote_originals() {
    // Install callable sentinels in the exact slots formerly used by fallback.
    // No live IL2CPP runtime/VM is needed to exercise the failure policy.
    // SAFETY: Rust test functions have these exact types and stay mapped.
    unsafe {
        CONFIG_ORIGINAL.set(remote_reference as RefSource as usize);
        MANIFEST_ORIGINAL.set(remote_reference as RefSource as usize);
        TIME_ORIGINAL.set(remote_time as TimeSource as usize);
        LOGIN_ORIGINAL.set(remote_login as LoginSource as usize);
        ANDROID_ID_ORIGINAL.set(remote_android_id as PlatformStringSource as usize);
    }
    REMOTE_CALLS.store(0, Ordering::Relaxed);
    for reason in ["constructor failed", "metadata mismatch", "wrong owner", "stopping", "Title scene runtime invoke failed"] {
        assert!(finish_source::<Ptr>(Err(reason)).is_none());
        assert!(FAILED.load(Ordering::Acquire));
    }
    // SAFETY: the failure path never dereferences these deliberately null arguments.
    unsafe {
        assert!(configuration_hook(std::ptr::null_mut(), std::ptr::null_mut()).is_null());
        assert!(manifest_hook(std::ptr::null_mut(), std::ptr::null_mut()).is_null());
        assert!(time_hook(0, std::ptr::null_mut()).is_null());
        login_hook(std::ptr::null_mut(), std::ptr::null_mut());
        assert!(android_id_hook(std::ptr::null_mut()).is_null());
    }
    assert_eq!(REMOTE_CALLS.load(Ordering::Relaxed), 0);
    CONFIG_ORIGINAL.clear(); MANIFEST_ORIGINAL.clear(); TIME_ORIGINAL.clear(); LOGIN_ORIGINAL.clear();
    ANDROID_ID_ORIGINAL.clear();
}
