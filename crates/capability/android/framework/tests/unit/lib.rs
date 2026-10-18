//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use rudroid_reply_catalog_format::Catalog;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

/// Serializes the ticket-52 join tests: the fake JNI knobs, the event
/// log, the shared-VM slot and the collected Host-API log are
/// PROCESS-GLOBAL statics (the same shape as the loader's once-installed
/// log sink and the runtime registry), so parallel tests would overwrite
/// each other's knobs and interleave each other's events. Every test
/// that touches them takes this lock EXACTLY ONCE at its top (the
/// ticket-52 development lesson: never re-lock inside a helper).
static SERIAL: Mutex<()> = Mutex::new(());

/// Catalog-backed fallback over the shared in-repo source table — the
/// same shape the runtime wiring will install across the dynamic
/// boundary (ticket 45). Test-only: nothing is compiled into the
/// capability (ticket 43).
fn catalog_fallback() -> CatalogFallback {
    let catalog = Catalog::from_source();
    Box::new(move |class: &str, method: &str, sig: &str| {
        catalog.query(class, method, sig).cloned()
    })
}

#[test]
fn init_registers_documented_class_set() {
    let mut framework = Framework::new();
    for surface in minimal_class_set() {
        framework.register_class(surface);
    }
    let names: Vec<&str> = framework.classes().iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        [
            "android/media/AudioManager",
            "android/media/AudioDeviceInfo",
            "android/view/Choreographer",
            "android/content/SharedPreferences",
        ]
    );
}

#[test]
fn unbound_methods_resolve_via_injected_catalog() {
    let framework = Framework::with_fallback(catalog_fallback());
    // No binding for AudioManager: the catalog fixed reply answers.
    assert_eq!(
        framework.call("android/media/AudioManager", "getStreamVolume", "(I)I"),
        Outcome::Catalog(ReplyValue::Int(100))
    );
}

#[test]
fn bound_methods_take_precedence_over_catalog() {
    let mut framework = Framework::with_fallback(catalog_fallback());
    framework.register_class(ClassSurface {
        name: "android/media/AudioManager",
        methods: vec![BoundMethod {
            method: "getStreamVolume",
            sig: "(I)I",
            reply: ReplyValue::Int(7),
        }],
    });
    assert_eq!(
        framework.call("android/media/AudioManager", "getStreamVolume", "(I)I"),
        Outcome::Bound(ReplyValue::Int(7)),
        "explicit binding wins over the catalog reply (100)"
    );
    // Other methods on the same class still fall through to the catalog.
    assert_eq!(
        framework.call("android/media/AudioManager", "isBluetoothA2dpOn", "()Z"),
        Outcome::Catalog(ReplyValue::Bool(false))
    );
}

#[test]
fn full_miss_is_visible() {
    // No fallback installed: every unbound call misses.
    let framework = Framework::new();
    assert_eq!(framework.call("some/Cls", "nope", "()V"), Outcome::Miss);
    // With the catalog installed, an absent key still misses.
    let framework = Framework::with_fallback(catalog_fallback());
    assert_eq!(framework.call("some/Cls", "nope", "()V"), Outcome::Miss);
}

#[test]
fn fallback_can_be_installed_after_construction() {
    let mut framework = Framework::new();
    assert_eq!(
        framework.call("android/media/AudioManager", "getStreamVolume", "(I)I"),
        Outcome::Miss
    );
    framework.set_fallback(catalog_fallback());
    assert!(framework.fallback().is_some());
    assert_eq!(
        framework.call("android/media/AudioManager", "getStreamVolume", "(I)I"),
        Outcome::Catalog(ReplyValue::Int(100))
    );
}

#[test]
fn reregistering_replaces_not_duplicates() {
    let mut framework = Framework::new();
    framework.register_class(ClassSurface {
        name: "a/B",
        methods: vec![],
    });
    framework.register_class(ClassSurface {
        name: "a/B",
        methods: vec![BoundMethod {
            method: "m",
            sig: "()V",
            reply: ReplyValue::Int(1),
        }],
    });
    assert_eq!(
        framework
            .classes()
            .iter()
            .filter(|c| c.name == "a/B")
            .count(),
        1
    );
}

#[test]
fn hooks_are_callable_from_tests() {
    let calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let handle = calls.clone();
    let mut hooks = AndroidHooks::default();
    let flush_handle = handle.clone();
    hooks.register(
        Box::new(move || flush_handle.lock().unwrap().push("flush")),
        Box::new(move || handle.lock().unwrap().push("vsync")),
    );
    hooks.flush_preferences();
    hooks.signal_vsync();
    hooks.flush_preferences();
    assert_eq!(&*calls.lock().unwrap(), &["flush", "vsync", "flush"]);
}

#[test]
fn unset_hooks_are_safe_noops() {
    let hooks = AndroidHooks::default();
    hooks.flush_preferences();
    hooks.signal_vsync();
}

// ---- ticket 52: the shared-session join ---------------------------------

/// The hooks POD the stub `register_android_hooks` received, as raw fn
/// addresses (0 = None) — ticket 77 records what crossed the C ABI.
static REGISTERED_HOOKS: Mutex<(usize, usize)> = Mutex::new((0, 0));
/// Knob: make the stub registry reject the hooks registration (the
/// first-write-wins duplicate case, ticket 77's lenient arm).
static REJECT_HOOKS: Mutex<bool> = Mutex::new(false);

/// A leaked HostApiV0 (the entry takes the loader's process-lifetime
/// table) whose shared_vm answers `vm` and whose log collects.
fn join_host(
    vm: *mut c_void,
) -> (
    &'static rudroid_common::host_api::HostApiV0,
    Arc<Mutex<Vec<String>>>,
) {
    use rudroid_common::registry::{RudroidAndroidHooks, HostStr};
    use std::ffi::c_char;

    // `usize` keeps the statics `Send` (raw pointers are not). The
    // OnceLock type itself comes from the crate-level import.
    static SLOT: OnceLock<Mutex<usize>> = OnceLock::new();
    static LOG: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
    *SLOT.get_or_init(|| Mutex::new(0)).lock().unwrap() = vm as usize;
    let log = LOG.get_or_init(|| Arc::new(Mutex::new(Vec::new()))).clone();
    // Join tests run under the tests-module SERIAL lock and clear both
    // shared surfaces here, so every test asserts only its own events.
    log.lock().unwrap().clear();
    *REGISTERED_HOOKS.lock().unwrap() = (0, 0);
    *REJECT_HOOKS.lock().unwrap() = false;

    extern "C" fn shared_vm() -> Option<rudroid_common::jni_env::JavaVm> {
        // SAFETY: join_host installs the pinned fake JNI table for these tests.
        unsafe { rudroid_common::jni_env::JavaVm::from_raw(*SLOT.get().unwrap().lock().unwrap() as *mut c_void) }
    }
    extern "C" fn collect_log(tag: HostStr<'_>, message: HostStr<'_>) {
        let tag = tag.to_cstr().unwrap().to_string_lossy().into_owned();
        let message = message.to_cstr().unwrap().to_string_lossy().into_owned();
        LOG.get()
            .unwrap()
            .lock()
            .unwrap()
            .push(format!("[{tag}] {message}"));
    }
    extern "C" fn catalog_query_int(
        _c: HostStr<'_>,
        _m: HostStr<'_>,
        _s: HostStr<'_>,
        out: Option<&mut i64>,
    ) -> c_int {
        *out.unwrap() = 100;
        1
    }
    extern "C" fn stub_get_bool(_k: HostStr<'_>, fallback: c_int) -> c_int {
        fallback
    }
    extern "C" fn stub_get_f64(_k: HostStr<'_>, fallback: f64) -> f64 {
        fallback
    }
    extern "C" fn stub_get_string(
        _k: HostStr<'_>,
        _fallback: HostStr<'_>,
        _out: *mut c_char,
        _out_size: u32,
    ) -> c_int {
        0
    }
    extern "C" fn stub_register_hooks(hooks: Option<&RudroidAndroidHooks>) -> c_int {
        if *REJECT_HOOKS.lock().unwrap() {
            // REGISTRY_REJECTED: the slot already holds a registration
            // (the ticket-45 first-write-wins contract).
            return 1;
        }
        let hooks = hooks.expect("the framework passes its own POD");
        *REGISTERED_HOOKS.lock().unwrap() = (
            hooks.flush_preferences.map(|f| f as usize).unwrap_or(0),
            hooks.signal_vsync.map(|f| f as usize).unwrap_or(0),
        );
        0
    }
    let host = rudroid_common::host_api::HostApiV0 {
        config_get_bool: stub_get_bool,
        config_get_f64: stub_get_f64,
        // SAFETY: the static stub returns a miss without retaining or touching buffers.
        config_get_string: unsafe { rudroid_common::host_api::ConfigGetStringFn::new(stub_get_string) },
        log: collect_log,
        register_android_hooks: stub_register_hooks,
        catalog_query_int,
        shared_vm,
        ..rudroid_common::host_api::HostApiV0::stubbed()
    };
    (Box::leak(Box::new(host)), log)
}

#[test]
fn register_jni_skips_the_join_with_a_logged_note_without_a_session() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (host, log) = join_host(std::ptr::null_mut());
    let code = rudroid_plugin_register_jni(Some(host));
    assert_eq!(code, 0, "missing session degrades like a missing provider");
    let lines = log.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("no JNI session registered") && l.contains("android_jni_vm")),
        "the skip must be explicit, not silent: {lines:?}"
    );
}

#[test]
fn register_jni_joins_the_session_and_pulls_the_catalog_value() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let calls = jni_fake::reset(100);
    let (host, log) = join_host(jni_fake::vm_ptr());
    let code = rudroid_plugin_register_jni(Some(host));
    assert_eq!(code, 0);
    let calls = calls.lock().unwrap().clone();
    assert_eq!(
        &calls[..5],
        [
            "find_class android/media/AudioManager",
            "new_global_ref 0x521",
            "delete_local_ref 0x521",
            "get_static_method_id getStreamVolume(I)I",
            "call_static_int",
        ],
        "join order: class -> global anchor -> local cleanup -> method -> call: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c == "register_natives 3"),
        "Context bindings use the shared VM"
    );
    assert!(
        calls.iter().any(|c| c == "register_natives 1"),
        "Process bindings use the shared VM"
    );
    assert!(
        calls.iter().any(|c| c == "call_static_void"),
        "Activity's parent is defined in the shared VM"
    );
    let lines = log.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("session join ok volume=100")),
        "the join logs the catalog value pulled through the JNI table: {lines:?}"
    );
}

#[test]
fn register_jni_fails_the_phase_when_the_session_misbehaves() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    jni_fake::reset(100);
    jni_fake::set_fail_find(true);
    let (host, log) = join_host(jni_fake::vm_ptr());
    let code = rudroid_plugin_register_jni(Some(host));
    assert_eq!(
        code, -3,
        "a broken session must fail the phase, not degrade"
    );
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("session join FAILED")),
        "failures are logged with their step: {lines:?}",
        lines = log.lock().unwrap()
    );

    jni_fake::reset(7);
    let (host, log) = join_host(jni_fake::vm_ptr());
    let code = rudroid_plugin_register_jni(Some(host));
    assert_eq!(
        code, -3,
        "a wrong catalog value through the session is a failure"
    );
    assert!(
        log.lock().unwrap().iter().any(|l| l.contains("returned 7")),
        "the mismatch names the value it got"
    );
}

// ---- ticket 77: the production Android hooks registration ---------------

/// Register-JNI must register BOTH production hooks through the existing
/// `register_android_hooks` C ABI (reference android_base parity), log
/// the registration, and the registered pointers must be the observable
/// implementations: `flush_preferences` logs the flush point (no
/// persistence layer yet), `signal_vsync` bumps the atomic frame counter
/// without per-frame logging unless diagnostics were explicitly enabled.
#[test]
fn register_jni_registers_both_production_hooks_through_the_c_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _calls = jni_fake::reset(100);
    let (host, log) = join_host(jni_fake::vm_ptr());
    let frames_before = VSYNC_FRAMES.load(Ordering::SeqCst);
    let code = rudroid_plugin_register_jni(Some(host));
    assert_eq!(code, 0);
    let (flush, vsync) = *REGISTERED_HOOKS.lock().unwrap();
    assert!(flush != 0 && vsync != 0, "both hooks must cross the C ABI");

    // Fire the registered production hooks through the recorded pointers
    // — the same pointers the loader later invokes (run exit flush, any
    // vsync signal). Each vsync bump must land in the counter.
    let flush_fn: unsafe extern "C" fn() = unsafe { std::mem::transmute(flush) };
    let vsync_fn: unsafe extern "C" fn() = unsafe { std::mem::transmute(vsync) };
    unsafe { flush_fn() };
    let logs_before = log.lock().unwrap().len();
    for _ in 0..10_000 {
        unsafe { vsync_fn() };
    }
    assert_eq!(
        log.lock().unwrap().len(),
        logs_before,
        "default vsync must not call the logger"
    );
    let frames_after = VSYNC_FRAMES.load(Ordering::SeqCst);
    assert_eq!(
        frames_after - frames_before,
        10_000,
        "each signal_vsync invocation bumps the atomic frame counter"
    );

    // Diagnostics remain available; exercise the same hook with logging
    // enabled and restore the default before leaving this serial test.
    LOG_VSYNC_FRAMES.store(true, Ordering::Relaxed);
    unsafe { vsync_fn() };
    unsafe { vsync_fn() };
    LOG_VSYNC_FRAMES.store(false, Ordering::Relaxed);
    let lines = log.lock().unwrap().clone();
    let registered_position = lines
        .iter()
        .position(|l| l.contains("android runtime hooks registered"))
        .unwrap_or_else(|| panic!("the registration is logged: {lines:?}"));
    assert!(
        lines[registered_position].contains("flush_preferences + signal_vsync"),
        "the registration names both hooks: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("[android_base] flush_preferences")
                && l.contains("dirty shared preference stores flushed")),
        "the flush hook logs the shared-store persistence point: {lines:?}"
    );
    // The two vsync log lines carry strictly increasing frame numbers.
    let frames: Vec<u64> = lines
        .iter()
        .filter_map(|l| {
            l.split("frame ")
                .nth(1)?
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect();
    assert_eq!(frames.len(), 2, "only explicitly enabled frames are logged");
    assert!(
        frames.windows(2).all(|w| w[0] < w[1]),
        "signal_vsync logs strictly increasing frame numbers: {frames:?}"
    );
    // Reference order: the hooks registration happens inside the
    // register-JNI phase, before the session join reports.
    let join_position = lines
        .iter()
        .position(|l| l.contains("session join ok"))
        .expect("the session join ran");
    assert!(
        registered_position < join_position,
        "hooks are registered before the session join reports: {lines:?}"
    );
}

/// A rejected registration (the slot already holds one — only reachable
/// on an in-process re-bring-up) keeps the phase green and logs that the
/// first registration stays; the registry's first-write-wins semantics
/// are untouched (ticket 77 design constraint 1).
#[test]
fn register_jni_keeps_the_phase_green_when_the_hooks_slot_is_taken() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (host, log) = join_host(std::ptr::null_mut());
    *REJECT_HOOKS.lock().unwrap() = true;
    let code = rudroid_plugin_register_jni(Some(host));
    assert_eq!(
        code, 0,
        "a rejection keeps the first registration; the phase stays green"
    );
    let lines = log.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("already registered") && l.contains("first-write-wins")),
        "the keep-first decision is logged, never silent: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("hooks registered:")),
        "the success line must not appear for a rejected registration: {lines:?}"
    );
}
