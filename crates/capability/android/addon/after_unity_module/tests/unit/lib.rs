//! INPUT:  private handlers of `src/lib.rs` via `super`, a thread-local stub Host API table
//! OUTPUT:  outcome, presence-check, idempotency, load-identity and init unit tests
//! POS:    drives the C ABI handlers directly; the kit path for every addon is the loader's addon_module_events test

use super::*;
use rudroid_common::registry::{RudroidModuleLoadedCallback, HostStr};
use std::ffi::c_char;

thread_local! {
    static GAME_FILES: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

unsafe extern "C" fn stub_config_get_string(
    key: HostStr<'_>,
    fallback: HostStr<'_>,
    out: *mut c_char,
    out_size: u32,
) -> c_int {
    let key = key.to_cstr().unwrap().to_string_lossy().into_owned();
    let value = if key == "paths.game_files" {
        GAME_FILES.with(|slot| slot.borrow().clone())
    } else {
        String::new()
    };
    // Mirror the host getter: copy the fallback when the value is absent.
    let (bytes, found) = if value.is_empty() {
        (fallback.to_cstr().unwrap().to_bytes(), 0)
    } else {
        (value.as_bytes(), 1)
    };
    let usable = (out_size as usize - 1).min(bytes.len());
    // SAFETY: test callers pass a real `out_size`-byte buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast(), usable);
        *out.add(usable) = 0;
    }
    found
}

// Logs every line into a thread-local capture (asserted by the tests).
thread_local! {
    static LOG_LINES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

extern "C" fn capture_log(tag: HostStr<'_>, message: HostStr<'_>) {
    let line = format!(
        "[{}] {}",
        tag.to_cstr().unwrap().to_string_lossy(),
        message.to_cstr().unwrap().to_string_lossy()
    );
    LOG_LINES.with(|slot| slot.borrow_mut().push(line));
}

extern "C" fn stub_register_module(
    _kind: u32,
    _callback: Option<RudroidModuleLoadedCallback>,
    _user: *mut c_void,
) -> c_int {
    0
}

fn stub_api() -> HostApiV0 {
    HostApiV0 {
        // SAFETY: the static getter bounds writes, NUL-terminates successes and retains no borrows.
        config_get_string: unsafe { rudroid_common::host_api::ConfigGetStringFn::new(stub_config_get_string) },
        log: capture_log,
        register_module_loaded: stub_register_module,
        ..HostApiV0::stubbed()
    }
}

fn logs() -> Vec<String> {
    LOG_LINES.with(|slot| slot.borrow().clone())
}

fn c_event(kind: ModuleKind, base: usize, name: &'static std::ffi::CStr) -> RudroidModuleEvent {
    RudroidModuleEvent {
        kind: kind as u32,
        base,
        name: HostStr::new(name),
    }
}

/// Sample addon data (the Burst values); the logic is data-independent.
static MODULE: GuestModule = GuestModule {
    library: "lib/arm64-v8a/lib_burst_generated.so",
    log_tag: "BURST",
    kind: ModuleKind::Burst,
    label: "burst",
};
const GUEST_C: &std::ffi::CStr = c"lib/arm64-v8a/lib_burst_generated.so";

/// The handler `user` handle exactly as `plugin_init` builds it.
fn handle(api: HostApiV0) -> *mut c_void {
    STATES.insert(AddonState { api, module: &MODULE })
}

#[test]
fn missing_guest_file_logs_and_skips_instead_of_mapping() {
    let _guard = crate::test_lock();
    reset_state_for_tests();
    GAME_FILES.with(|slot| *slot.borrow_mut() = "/tmp/does-not-exist-rudroid56".to_owned());
    let user = handle(stub_api());
    on_unity_loaded(Some(&c_event(ModuleKind::Unity, 0x1000, c"lib/arm64-v8a/libunity.so")), user);
    assert_eq!(last_outcome(), Some(MappingOutcome::SkippedMissing));
    assert!(
        logs().iter().any(|l| l.contains("guest module is missing") && l.contains(MODULE.library)),
        "the missing-file log must name the library: {:?}",
        logs()
    );
    // The module event is authoritative: if a library appears (or a
    // synthetic event fires) after the presence check said missing, the
    // real load event still maps it.
    on_module_loaded(Some(&c_event(MODULE.kind, 0x2000, GUEST_C)), user);
    assert_eq!(last_outcome(), Some(MappingOutcome::Mapped));
    reset_state_for_tests();
}

#[test]
fn module_event_with_real_base_and_path_maps_once() {
    let _guard = crate::test_lock();
    reset_state_for_tests();
    let user = handle(stub_api());
    on_module_loaded(Some(&c_event(MODULE.kind, 0x7f01000, GUEST_C)), user);
    assert_eq!(last_outcome(), Some(MappingOutcome::Mapped));
    assert!(logs().iter().any(|l| l.contains("base=0x7f01000")), "{:?}", logs());
    // A repeated event must not re-initialize (oracle attempted_).
    on_module_loaded(Some(&c_event(MODULE.kind, 0x7f02000, GUEST_C)), user);
    assert_eq!(last_outcome(), Some(MappingOutcome::Mapped));
    assert!(
        logs().iter().any(|l| l.contains("already initialized (idempotent")),
        "the repeat must be logged as ignored: {:?}",
        logs()
    );
    reset_state_for_tests();
}

#[test]
fn module_event_without_a_base_or_wrong_path_fails_explicitly() {
    let _guard = crate::test_lock();
    reset_state_for_tests();
    let user = handle(stub_api());
    on_module_loaded(Some(&c_event(MODULE.kind, 0, GUEST_C)), user);
    assert!(matches!(
        last_outcome(),
        Some(MappingOutcome::Failed { reason: "module event reported no load base" })
    ));
    reset_state_for_tests();
    on_module_loaded(Some(&c_event(MODULE.kind, 0x3000, c"lib/arm64-v8a/someone_else.so")), user);
    assert!(matches!(
        last_outcome(),
        Some(MappingOutcome::Failed { reason: "module event named an unexpected path" })
    ));
    reset_state_for_tests();
}

#[test]
fn unity_event_idempotency_logs_ignored_repeats() {
    let _guard = crate::test_lock();
    reset_state_for_tests();
    GAME_FILES.with(|slot| *slot.borrow_mut() = "/tmp/does-not-exist-rudroid56".to_owned());
    let user = handle(stub_api());
    let unity = c_event(ModuleKind::Unity, 0x1000, c"lib/arm64-v8a/libunity.so");
    on_unity_loaded(Some(&unity), user);
    on_unity_loaded(Some(&unity), user);
    assert_eq!(last_outcome(), Some(MappingOutcome::SkippedMissing));
    assert!(logs().iter().filter(|l| l.contains("already attempted")).count() == 1);
    reset_state_for_tests();
}

#[test]
fn init_registers_both_subscriptions_and_rejects_bad_tables() {
    let _guard = crate::test_lock();
    reset_state_for_tests();
    let api = stub_api();
    let rc = plugin_init(&api, &MODULE);
    assert_eq!(rc, 0, "stub registration entries accept");
    // Re-init on a fresh state: the stub table accepts again (the loader
    // only ever inits once; the in-process suite re-arms explicitly).
    reset_state_for_tests();
    let rc = plugin_init(&api, &MODULE);
    assert_eq!(rc, 0);
    assert_eq!(plugin_init_raw(None, &MODULE), -1, "a null table is refused");
    reset_state_for_tests();
}

#[test]
fn outcomes_carry_the_honest_states() {
    let _guard = crate::test_lock();
    reset_state_for_tests();
    assert_eq!(last_outcome(), None, "nothing observed before init");
    set_outcome(MappingOutcome::SkippedMissing);
    assert_eq!(last_outcome(), Some(MappingOutcome::SkippedMissing));
    set_outcome(MappingOutcome::Mapped);
    assert_eq!(last_outcome(), Some(MappingOutcome::Mapped));
    set_outcome(MappingOutcome::Failed {
        reason: "module event reported no load base",
    });
    assert!(matches!(
        last_outcome(),
        Some(MappingOutcome::Failed { .. })
    ));
    reset_state_for_tests();
}
