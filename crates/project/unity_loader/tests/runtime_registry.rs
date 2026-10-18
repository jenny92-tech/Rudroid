//! INPUT:  production cdylibs, shared runtime registry and serialized test fixtures
//! OUTPUT:  dynamic registration, callback reentry, lifecycle identity and shutdown acceptance
//! POS:    verifies cross-capability shared state without statically linking capabilities
//! Ticket 45 acceptance: the shared runtime registry across real deployed
//! libraries.
//!
//! - The catalog provider registers from the deployed `reply_catalog` .so
//!   (early_init) and the deployed `android_base` .so (framework) queries it
//!   through the Host API during register_jni — shared state crossing the C
//!   ABI, never a static link. Missing provider degrades to a logged miss.
//! - Module events cross back with verifiable identity; callbacks run
//!   outside all registry locks and may re-enter (subscribe/query).
//! - Guest symbol callbacks may query and withdraw their own module
//!   bindings; the in-flight handle remains process-pinned.
//! - Singular registrations (profile/policy/VM slot) accept exactly one.
//! - A capability whose init fails aborts bring-up with a structured error
//!   and the registry shuts down, so its subscription can never fire.
//!
//! The registry is process-global state, so the tests serialize on a shared
//! mutex and each tolerates whatever a previous test left behind.
//!
//! Assembly contract (tickets 73/75, the 68/71 pattern): the cdylibs and
//! fixtures are located the way cargo itself does — CARGO_TARGET_DIR when
//! the invocation set one (relative paths resolve against the crates root),
//! else the workspace `target` — and always rebuilt with THIS run's profile
//! before use, so one plain `cargo test` command works from an empty target
//! dir in either profile and can never load a foreign or stale artifact.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rudroid_common::registry::HostStr;
use rudroid_test_support::cargo::{build_loader_fixture, ensure_workspace_cdylib};
use rudroid_unity_loader::{bring_up, load_deploy_dir, runtime, Selection};

/// Serializes every test in this binary: the registry is process-global.
static SERIAL: Mutex<()> = Mutex::new(());

const SYMBOL_BASE: usize = 0x1000;
const SYMBOL_HANDLE: usize = 0x2000;
const SYMBOL_ADDRESS: usize = 0x3000;
static SYMBOL_WORKER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

#[test]
fn player_maintenance_events_are_mapping_free_and_allow_reentrant_queries() {
    let _serial = SERIAL.lock().unwrap();
    runtime::shutdown();
    static EVENTS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    extern "C" fn record(event: Option<&rudroid_common::registry::RudroidModuleEvent>, _: *mut std::ffi::c_void) {
        let event = event.unwrap();
        assert_eq!(event.base, 0);
        assert!(event.name.to_cstr().unwrap().to_bytes().starts_with(b"runtime/unity_"));
        let _ = runtime::shared_vm(); // Registry lock must be released.
        EVENTS.lock().unwrap().push(event.kind);
    }
    EVENTS.lock().unwrap().clear();
    for kind in [rudroid_common::ModuleKind::UnityMaintenance, rudroid_common::ModuleKind::UnityStopping, rudroid_common::ModuleKind::UnityFrame] {
        assert_eq!(runtime::register_module_loaded(kind as u32, record, std::ptr::null_mut()), 0);
        assert_eq!(runtime::fire_runtime_event(kind as u32), 0);
    }
    assert_ne!(runtime::fire_runtime_event(rudroid_common::ModuleKind::Unity as u32), 0);
    assert_eq!(*EVENTS.lock().unwrap(), [101, 102, 103]);
    runtime::shutdown();
    assert_eq!(runtime::fire_runtime_event(103), 0);
    assert_eq!(*EVENTS.lock().unwrap(), [101, 102, 103]);
    runtime::shutdown();
}

/// Probe the lock on a worker before re-entering from the callback itself.
/// A regression returns a miss after a timeout instead of hanging cargo;
/// the test joins the worker after the lookup has released its lock.
unsafe extern "C" fn reentrant_symbol_lookup(
    handle: *const std::ffi::c_void,
    name: *const std::ffi::c_char,
) -> usize {
    if handle as usize != SYMBOL_HANDLE || name.is_null() {
        return 0;
    }
    // SAFETY: the lookup owns this NUL-terminated string for the call.
    if unsafe { std::ffi::CStr::from_ptr(name) } != c"found" {
        return 0;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = sender.send(runtime::guest_exec_bases());
    });
    *SYMBOL_WORKER.lock().unwrap() = Some(worker);
    let Ok(bases) = receiver.recv_timeout(std::time::Duration::from_secs(5)) else {
        return 0;
    };
    if !bases.contains(&SYMBOL_BASE) {
        return 0;
    }
    // Direct re-entry, including mutation: withdrawing lookup identity
    // cannot destroy the pinned handle used by this in-flight callback.
    runtime::unregister_guest_exec_module(SYMBOL_BASE);
    if runtime::guest_exec_bases().contains(&SYMBOL_BASE) {
        return 0;
    }
    SYMBOL_ADDRESS
}

#[test]
fn guest_symbol_callbacks_reenter_without_holding_the_module_registry_lock() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    assert_eq!(runtime::guest_exec_symbol(SYMBOL_BASE, "found"), None);
    assert_eq!(runtime::guest_exec_symbol_all(SYMBOL_BASE, "found"), None);
    unsafe { runtime::register_guest_exec_backend(reentrant_symbol_lookup) };
    unsafe { runtime::register_guest_exec_symbol_all(reentrant_symbol_lookup) };

    for lookup in [runtime::guest_exec_symbol, runtime::guest_exec_symbol_all] {
        unsafe { runtime::register_guest_exec_module(SYMBOL_BASE, SYMBOL_HANDLE as *mut std::ffi::c_void) };
        assert_eq!(lookup(SYMBOL_BASE, "missing"), None, "a backend miss remains a miss");
        assert_eq!(lookup(SYMBOL_BASE + 1, "found"), None, "an unknown base never calls the backend");
        let address = lookup(SYMBOL_BASE, "found");
        SYMBOL_WORKER.lock().unwrap().take().unwrap().join().unwrap();
        assert_eq!(address, Some(SYMBOL_ADDRESS), "the callback must acquire the registry lock");
        assert_eq!(lookup(SYMBOL_BASE, "found"), None, "callback withdrawal prevents future lookups");
    }
    runtime::shutdown();
}

fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    ensure_workspace_cdylib(package, artifact)
}

/// Build a fixture capability (own workspace, like the dummy capability).
/// `name` is the `tests/fixtures` directory; the crate is `rudroid-<name>`.
fn build_fixture(name: &str) -> PathBuf {
    build_loader_fixture(name, &format!("rudroid_{name}"))
}

/// One process-global log sink shared by every test in this binary (the sink
/// installs once and ignores later installs).
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

/// C ABI callback recording the exact event identity it observed.
extern "C" fn recording_callback(
    event: Option<&rudroid_common::registry::RudroidModuleEvent>,
    user: *mut std::ffi::c_void,
) {
    // SAFETY: the registry fires with a valid event; `user` is the shared
    // Vec handle installed by the test.
    let event = *event.unwrap();
    let name = event.name.to_cstr().unwrap()
        .to_string_lossy()
        .into_owned();
    let recorded = unsafe { &*(user as *const Mutex<Vec<String>>) };
    recorded
        .lock()
        .unwrap()
        .push(format!("kind={} base={:#x} name={name}", event.kind, event.base));
}

/// Re-entrant callback: queries the registry and subscribes again from
/// inside the fire. Must not deadlock.
extern "C" fn reentrant_callback(
    event: Option<&rudroid_common::registry::RudroidModuleEvent>,
    user: *mut std::ffi::c_void,
) {
    // SAFETY: see `recording_callback`.
    let event = *event.unwrap();
    let recorded = unsafe { &*(user as *const Mutex<Vec<String>>) };
    recorded
        .lock()
        .unwrap()
        .push(format!("reentrant kind={}", event.kind));
    // Registry re-entry from a callback: must not deadlock.
    let _ = runtime::shared_vm();
    runtime::register_module_loaded(
        rudroid_common::ModuleKind::Il2cpp as u32,
        recording_callback,
        user,
    );
}

#[test]
fn catalog_and_phase_flow_across_deployed_libraries() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let log = captured_log();
    let framework = ensure_cdylib("rudroid-android-framework", "android_base");
    let catalog = ensure_cdylib(
        "rudroid-android-reply-catalog",
        "rudroid_android_reply_catalog",
    );

    // 1. Kit WITHOUT the catalog capability: the framework's register_jni
    //    phase queries the registry, finds no provider and logs a miss.
    let deploy = std::env::temp_dir().join(format!("rudroid-runtime-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    deploy_kit(&deploy, &[&framework]);
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    bring_up(capabilities, None).expect("bring-up degrades without a provider");
    {
        let lines = log.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l == "[android_base] catalog getStreamVolume hit=0 value=-1"),
            "missing provider must surface as a logged miss, not a crash: {lines:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&deploy);

    // 2. Kit WITH the catalog: reply_catalog's early_init registers the
    //    provider, framework's register_jni (next phase) queries it and gets
    //    the fixed reply 100 — shared state across two deployed libraries.
    let deploy = std::env::temp_dir().join(format!("rudroid-runtime-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    deploy_kit(&deploy, &[&framework, &catalog]);
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load kit");
    let capabilities = bring_up(capabilities, None).expect("bring-up with catalog provider");
    // Handles stay pinned: the provider's function pointers live in the
    // catalog library and the registry holds them for process life.
    std::mem::forget(capabilities);
    {
        let lines = log.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l == "[android_base] catalog getStreamVolume hit=1 value=100"),
            "framework .so must query the provider registered by the catalog .so: {lines:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&deploy);

    // 3. Module events across the C ABI: exact identity, and a callback may
    //    re-enter the registry (query + subscribe) without deadlocking. The
    //    nested subscription takes effect from the NEXT fire.
    let recorded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let callback: rudroid_common::registry::RudroidModuleLoadedCallback = reentrant_callback;
    assert_eq!(
        runtime::register_module_loaded(
            rudroid_common::ModuleKind::Main as u32,
            callback,
            Arc::as_ptr(&recorded) as *mut std::ffi::c_void,
        ),
        0
    );
    let main_name = rudroid_common::descriptor::leak_cstr("lib/arm64-v8a/libmain.so");
    let il2cpp_name = rudroid_common::descriptor::leak_cstr("lib/arm64-v8a/libil2cpp.so");
    let main_event = rudroid_common::registry::RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Main as u32,
        base: 0x1234000,
        name: HostStr::new(main_name),
    };
    let il2cpp_event = rudroid_common::registry::RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Il2cpp as u32,
        base: 0x1235000,
        name: HostStr::new(il2cpp_name),
    };
    runtime::fire_module_event(&main_event);
    runtime::fire_module_event(&il2cpp_event);
    let seen = recorded.lock().unwrap().clone();
    assert_eq!(
        seen[0],
        "reentrant kind=1",
        "first fire runs the original callback only: {seen:?}"
    );
    assert!(
        seen.iter().skip(1).any(|l| l.starts_with("kind=2 base=0x1235000 name=lib/arm64-v8a/libil2cpp.so")),
        "nested subscription fires from the next fire on, with exact identity: {seen:?}"
    );
}

/// Sentinel probe provider (ticket 60): answers one int query with 4242 and
/// counts its destroys, so the test can prove the existing provider is neither
/// freed by a rejected registration attempt nor destroyed twice at shutdown.
static PROBE_DESTROYED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

extern "C" fn probe_query_int(
    user: *mut std::ffi::c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> std::ffi::c_int {
    let _ = user;
    if let Some(out) = out {
        *out = 4242;
    }
    1
}

extern "C" fn probe_destroy(user: *mut std::ffi::c_void) {
    PROBE_DESTROYED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if !user.is_null() {
        // SAFETY: the test created exactly one Box for this user slot.
        drop(unsafe { Box::from_raw(user as *mut u64) });
    }
}

#[test]
fn rejected_early_init_rejects_and_keeps_the_registered_provider() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown(); // deterministic empty registry
    PROBE_DESTROYED.store(0, std::sync::atomic::Ordering::SeqCst);

    let catalog = ensure_cdylib(
        "rudroid-android-reply-catalog",
        "rudroid_android_reply_catalog",
    );
    // SAFETY: dlopen of a workspace-built cdylib; handle pinned for the test.
    let lib = unsafe { libloading::Library::new(&catalog) }.expect("dlopen catalog cdylib");
    let early_init: libloading::Symbol<
        unsafe extern "C" fn(*const rudroid_common::host_api::HostApiV0) -> std::ffi::c_int,
    > = unsafe { lib.get(b"rudroid_plugin_early_init") }.expect("early_init export");
    let api = rudroid_unity_loader::host_api::table();

    // Case 1 — host actively rejects: the probe provider fills the slot, so
    // the catalog capability's early_init must fail non-zero WITHOUT freeing
    // or disturbing the already-registered provider.
    let staged = Box::into_raw(Box::new(0xC0FFEE_u64)) as *mut std::ffi::c_void;
    let probe = rudroid_common::registry::CatalogProviderV0 {
        user: staged,
        query_int: Some(probe_query_int),
        query_bool: None,
        destroy: Some(probe_destroy),
    };
    assert_eq!(runtime::register_catalog_provider(Some(&probe)), 0);
    let rejected = unsafe { early_init(api) };
    assert_ne!(
        rejected, 0,
        "early_init must report the host's rejection as a non-zero code"
    );
    assert_eq!(
        PROBE_DESTROYED.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the rejected registration must not destroy the existing provider"
    );
    let mut out = 0i64;
    assert_eq!(
        runtime::catalog_query_int(
            HostStr::new(c"probe/Class"),
            HostStr::new(c"volume"),
            HostStr::new(c"()I"),
            Some(&mut out)
        ),
        1
    );
    assert_eq!(out, 4242, "the pre-existing provider must stay queryable");
    runtime::shutdown();
    assert_eq!(
        PROBE_DESTROYED.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "shutdown destroys the probe provider exactly once"
    );

    // Case 2 — duplicate early_init: the first call registers the real
    // catalog provider, the second is rejected, and the first one keeps
    // answering queries.
    assert_eq!(
        unsafe { early_init(api) },
        0,
        "first early_init against an empty registry succeeds"
    );
    assert_ne!(
        unsafe { early_init(api) },
        0,
        "a second early_init must be rejected without disturbing the first provider"
    );
    let mut out = 0i64;
    assert_eq!(
        runtime::catalog_query_int(
            HostStr::new(c"android/media/AudioManager"),
            HostStr::new(c"getStreamVolume"),
            HostStr::new(c"(I)I"),
            Some(&mut out)
        ),
        1
    );
    assert_eq!(out, 100, "the originally registered provider still answers");
    runtime::shutdown();
    assert_eq!(
        runtime::catalog_query_int(
            HostStr::new(c"android/media/AudioManager"),
            HostStr::new(c"getStreamVolume"),
            HostStr::new(c"(I)I"),
            Some(&mut out)
        ),
        0,
        "after shutdown queries miss cleanly instead of calling a destroyed provider"
    );
}

#[test]
fn singular_registrations_and_failed_init_shutdown() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    // Singular slots: first registration wins, duplicates are rejected, and
    // the host keeps its own copy of the POD.
    let profile = rudroid_common::registry::RudroidUnityProfile {
        id: HostStr::new(rudroid_common::descriptor::leak_cstr("unity_2021_3")),
        player_class: HostStr::null(),
        player_constructor_signature: HostStr::null(),
        init_jni_signature: HostStr::null(),
        flags: rudroid_common::registry::RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG,
        init_jni_int_arg: 1,
        prepare: None,
        bootstrap: None,
        run: None,
    };
    assert_eq!(runtime::register_unity_profile(Some(&profile)), 0);
    assert_eq!(
        runtime::register_unity_profile(Some(&profile)),
        1,
        "second profile registration is rejected"
    );
    let stored = runtime::registered_profile().expect("profile copy retained");
    assert_eq!(stored.flags, rudroid_common::registry::RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG);

    let policy = rudroid_common::registry::RudroidAudioPolicy {
        id: HostStr::new(rudroid_common::descriptor::leak_cstr("sdl_audio")),
        default_open_attempts: 3,
        named_open_attempts: 2,
        retry_delay_ms: 100,
        enumerate_outputs: 1,
        prefer_non_hdmi: 1,
        reject_silent_drivers: 0,
    };
    assert_eq!(runtime::register_audio_policy(Some(&policy)), 0);
    assert_eq!(runtime::register_audio_policy(Some(&policy)), 1);
    let stored = runtime::registered_audio_policy().expect("policy copy retained");
    assert_eq!((stored.default_open_attempts, stored.retry_delay_ms), (3, 100));

    // Shared VM/session slot: first write wins, readers see the same pointer.
    // A process-pinned invoke table that fails every VM operation cleanly.
    unsafe extern "C" fn failed_vm(_: *mut std::ffi::c_void) -> i32 { -1 }
    unsafe extern "C" fn failed_attach(_: *mut std::ffi::c_void, _: *mut *mut std::ffi::c_void, _: *mut std::ffi::c_void) -> i32 { -1 }
    unsafe extern "C" fn failed_env(_: *mut std::ffi::c_void, _: *mut *mut std::ffi::c_void, _: i32) -> i32 { -1 }
    let table = Box::leak(Box::new([0, 0, 0, failed_vm as usize, failed_attach as usize,
        failed_vm as usize, failed_env as usize, failed_attach as usize]));
    let token = Box::leak(Box::new(table.as_ptr())) as *mut *const usize as *mut std::ffi::c_void;
    // SAFETY: the handle and correctly typed invocation slots are leaked and
    // never yield an environment, so callers cannot obtain dangling JNI state.
    let vm = unsafe { rudroid_common::jni_env::JavaVm::from_raw(token) }.unwrap();
    runtime::register_shared_vm(vm);
    assert_eq!(runtime::shared_vm(), token, "readers observe the same session");

    // Failed init: the failing fixture registers a module subscription in
    // early_init, then init returns 5. bring-up must abort with a structured
    // error naming capability/phase/code and shut the registry down. The
    // probe capability is in the kit to satisfy the one-android_base
    // topology validation before any phase runs.
    let failing = build_fixture("failing_capability");
    let probe = build_fixture("probe_capability");
    let deploy = std::env::temp_dir().join(format!("rudroid-runtime-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    deploy_kit(&deploy, &[&probe, &failing]);
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load failing kit");
    let err = bring_up(capabilities, None).expect_err("init failure must abort bring-up");
    let message = err.to_string();
    assert!(
        message.contains("failing") && message.contains("Init") && message.contains("5"),
        "structured failure must name capability, phase and code: {message}"
    );

    // No dangling callbacks: fire after shutdown must not reach the fixture.
    runtime::fire_module_event(&rudroid_common::registry::RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::Main as u32,
        base: 0,
        name: HostStr::new(rudroid_common::descriptor::leak_cstr("lib/arm64-v8a/libmain.so")),
    });
    let counter = unsafe {
        let library = libloading::Library::new(&failing).expect("reopen failing fixture");
        let get: libloading::Symbol<unsafe extern "C" fn() -> u32> = library
            .get(b"rudroid_failing_dangling_events")
            .expect("fixture exports its counter");
        get()
    };
    assert_eq!(counter, 0, "subscription must not fire after init failure + shutdown");
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn frame_subscription_reentry_applies_to_the_next_snapshot() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    use std::sync::atomic::{AtomicUsize, Ordering};
    static LATE: AtomicUsize = AtomicUsize::new(0);
    static EARLY: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn late(_: Option<&rudroid_common::registry::RudroidModuleEvent>, _: *mut std::ffi::c_void) {
        LATE.fetch_add(1, Ordering::Relaxed);
    }
    extern "C" fn early(_: Option<&rudroid_common::registry::RudroidModuleEvent>, _: *mut std::ffi::c_void) {
        if EARLY.fetch_add(1, Ordering::Relaxed) == 0 {
            assert_eq!(runtime::register_module_loaded(103, late, std::ptr::null_mut()), 0);
        }
    }
    EARLY.store(0, Ordering::Relaxed);
    LATE.store(0, Ordering::Relaxed);
    assert_eq!(runtime::register_module_loaded(103, early, std::ptr::null_mut()), 0);
    assert_eq!(runtime::fire_runtime_event(103), 0);
    assert_eq!(LATE.load(Ordering::Relaxed), 0);
    assert_eq!(runtime::fire_runtime_event(103), 0);
    assert_eq!(EARLY.load(Ordering::Relaxed), 2);
    assert_eq!(LATE.load(Ordering::Relaxed), 1);
    runtime::shutdown();
}
