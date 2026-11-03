//! INPUT:  private items of `src/engine.rs` via `super`, a fake IL2CPP export table bound through `Il2cpp::bind`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/engine.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::il2cpp_abi::GcHandleAbi;
use rudroid_common::registry::HostStr;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
static BAD_BYREF: AtomicU32 = AtomicU32::new(0);
static REENTER_STOP: AtomicBool = AtomicBool::new(false);
static UNBOX_CALLS: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicBool = AtomicBool::new(false);
static EXCEPTION: AtomicBool = AtomicBool::new(false);
static ROOTS: AtomicU32 = AtomicU32::new(0);
static ROOT_CALLS: AtomicU32 = AtomicU32::new(0);
static NOTIFY_ARGC: AtomicU32 = AtomicU32::new(0);
static NOTIFICATIONS: AtomicU32 = AtomicU32::new(0);
static BAD_RETURN: AtomicBool = AtomicBool::new(false);
static COLLECTED: AtomicU32 = AtomicU32::new(0);
static MAJOR: AtomicU32 = AtomicU32::new(2021);
/// Drops the GC-handle root export and the optional GC/measurement exports.
static STRIPPED: AtomicBool = AtomicBool::new(false);
static VERSION_2021: [u16; 11] = [50, 48, 50, 49, 46, 51, 46, 49, 54, 102, 49];
static VERSION_6000: [u16; 11] = [54, 48, 48, 48, 46, 48, 46, 54, 49, 102, 49];
static VERSION_ADJACENT: [u16; 11] = [54, 48, 48, 48, 46, 48, 46, 54, 50, 102, 49];
static ASSEMBLY: [usize; 1] = [1];
static TRUE: u8 = 1;
static FALSE: u8 = 0;
unsafe extern "C" fn domain() -> Ptr {
    1usize as Ptr
}
unsafe extern "C" fn assemblies(_: Ptr, count: *mut usize) -> *const Ptr {
    *count = 1;
    ASSEMBLY.as_ptr().cast()
}
unsafe extern "C" fn image(_: Ptr) -> Ptr {
    1usize as Ptr
}
unsafe extern "C" fn class(_: Ptr, namespace: *const c_char, name: *const c_char) -> Ptr {
    assert_eq!(CStr::from_ptr(namespace), c"UnityEngine");
    let id = match CStr::from_ptr(name).to_bytes() {
        b"Resources" => 10,
        b"AsyncOperation" => 11,
        b"Application" => 12,
        b"ApplicationMemoryUsage" => 13,
        _ => 0,
    };
    id as Ptr
}
unsafe extern "C" fn method(class: Ptr, name: *const c_char, argc: i32) -> Ptr {
    match (class as usize, CStr::from_ptr(name).to_bytes(), argc) {
        (10, b"UnloadUnusedAssets", 0) => 20usize as Ptr,
        (11, b"get_isDone", 0) => 21usize as Ptr,
        (12, b"get_unityVersion", 0) => 23usize as Ptr,
        (12, b"CallLowMemory", n) if n == NOTIFY_ARGC.load(SeqCst) as i32 => 22usize as Ptr,
        _ => std::ptr::null_mut(),
    }
}
unsafe extern "C" fn flags(method: Ptr, _: *mut u32) -> u32 {
    if method as usize == 21 {
        0
    } else {
        0x10
    }
}
unsafe extern "C" fn argc(method: Ptr) -> u32 {
    if method as usize == 22 {
        NOTIFY_ARGC.load(SeqCst)
    } else {
        0
    }
}
unsafe extern "C" fn returns(method: Ptr) -> Ptr {
    match method as usize {
        20 => {
            if BAD_RETURN.load(SeqCst) {
                99usize as Ptr
            } else {
                11usize as Ptr
            }
        }
        21 => 2usize as Ptr,
        23 => 14usize as Ptr,
        _ => 1usize as Ptr,
    }
}
unsafe extern "C" fn param(_: Ptr, _: u32) -> Ptr {
    13usize as Ptr
}
unsafe extern "C" fn kind(ty: Ptr) -> i32 {
    ty as i32
}
unsafe extern "C" fn class_from_type(ty: Ptr) -> Ptr {
    ty
}
unsafe extern "C" fn invoke(method: Ptr, object: Ptr, args: *mut Ptr, error: *mut Ptr) -> Ptr {
    if EXCEPTION.swap(false, SeqCst) {
        *error = 99usize as Ptr;
        return std::ptr::null_mut();
    }
    *error = std::ptr::null_mut();
    match method as usize {
        20 => {
            assert!(object.is_null());
            100usize as Ptr
        }
        21 => {
            if REENTER_STOP.swap(false, SeqCst) {
                let event = rudroid_common::registry::RudroidModuleEvent {
                    kind: rudroid_common::ModuleKind::UnityStopping as u32,
                    base: 0,
                    name: HostStr::new(c"runtime/unity_stopping"),
                };
                crate::lifecycle(Some(&event), std::ptr::null_mut());
            }
            assert_eq!(object as usize, 100);
            if DONE.load(SeqCst) {
                &TRUE as *const u8 as Ptr
            } else {
                &FALSE as *const u8 as Ptr
            }
        }
        23 => 200usize as Ptr,
        22 => {
            if NOTIFY_ARGC.load(SeqCst) == 1 {
                assert!(!args.is_null());
                assert_eq!(*(*args).cast::<i32>(), 4);
            } else {
                assert!(args.is_null());
            }
            NOTIFICATIONS.fetch_add(1, SeqCst);
            std::ptr::null_mut()
        }
        _ => panic!("unexpected method"),
    }
}
// Legacy and modern exports use their actual, distinct C prototypes.
unsafe extern "C" fn root(object: Ptr, pinned: bool) -> u32 {
    assert_eq!(object as usize, 100);
    assert!(!pinned);
    ROOT_CALLS.fetch_add(1, SeqCst);
    ROOTS.fetch_add(1, SeqCst);
    7
}
unsafe extern "C" fn target(root: u32) -> Ptr {
    assert_eq!(root, 7);
    100usize as Ptr
}
unsafe extern "C" fn free(root: u32) {
    assert_eq!(root, 7);
    assert_eq!(ROOTS.fetch_sub(1, SeqCst), 1);
}
const WIDE_HANDLE: usize = 0x1234_0000_0007;
unsafe extern "C" fn root_wide(object: Ptr, pinned: bool) -> usize {
    root(object, pinned);
    WIDE_HANDLE
}
unsafe extern "C" fn target_wide(root: usize) -> Ptr {
    assert_eq!(root, WIDE_HANDLE);
    target(7)
}
unsafe extern "C" fn free_wide(root: usize) {
    assert_eq!(root, WIDE_HANDLE);
    free(7)
}
unsafe extern "C" fn string_length(_: Ptr) -> i32 {
    11
}
unsafe extern "C" fn string_chars(_: Ptr) -> *const u16 {
    if MAJOR.load(SeqCst) == 6000 {
        VERSION_6000.as_ptr()
    } else if MAJOR.load(SeqCst) == 6001 {
        VERSION_ADJACENT.as_ptr()
    } else {
        VERSION_2021.as_ptr()
    }
}
unsafe extern "C" fn unbox(object: Ptr) -> Ptr {
    UNBOX_CALLS.fetch_add(1, SeqCst);
    object
}
unsafe extern "C" fn collect(generation: i32) {
    assert_eq!(generation, 0);
    COLLECTED.fetch_add(1, SeqCst);
}
unsafe extern "C" fn bytes() -> i64 {
    1024
}
extern "C" fn so_symbol(_module: Ptr, name: HostStr<'_>) -> usize {
    let wide = MAJOR.load(SeqCst) == 6000;
    let stripped = STRIPPED.load(SeqCst);
    match name.to_cstr().unwrap().to_bytes() {
        b"il2cpp_domain_get" | b"il2cpp_thread_current" => domain as usize,
        b"il2cpp_domain_get_assemblies" => assemblies as usize,
        b"il2cpp_assembly_get_image" => image as usize,
        b"il2cpp_class_from_name" => class as usize,
        b"il2cpp_class_get_method_from_name" => method as usize,
        b"il2cpp_method_get_flags" => flags as usize,
        b"il2cpp_method_get_param_count" => argc as usize,
        b"il2cpp_method_get_return_type" => returns as usize,
        b"il2cpp_method_get_param" => param as usize,
        b"il2cpp_type_get_type" => kind as usize,
        b"il2cpp_type_is_byref" => byref as usize,
        b"il2cpp_class_from_type" => class_from_type as usize,
        b"il2cpp_runtime_invoke" => invoke as usize,
        b"il2cpp_string_length" => string_length as usize,
        b"il2cpp_string_chars" => string_chars as usize,
        b"il2cpp_object_unbox" => unbox as usize,
        b"il2cpp_gchandle_new" if stripped => 0,
        b"il2cpp_gchandle_new" if wide => root_wide as usize,
        b"il2cpp_gchandle_new" => root as usize,
        b"il2cpp_gchandle_get_target" if wide => target_wide as usize,
        b"il2cpp_gchandle_get_target" => target as usize,
        b"il2cpp_gchandle_free" if wide => free_wide as usize,
        b"il2cpp_gchandle_free" => free as usize,
        b"il2cpp_gc_collect" | b"il2cpp_gc_get_used_size" | b"il2cpp_gc_get_heap_size"
            if stripped =>
        {
            0
        }
        b"il2cpp_gc_collect" => collect as usize,
        b"il2cpp_gc_get_used_size" | b"il2cpp_gc_get_heap_size" => bytes as usize,
        _ => 0,
    }
}
extern "C" fn byref(ty: Ptr) -> bool {
    let bad_type = match BAD_BYREF.load(SeqCst) {
        1 => 11,
        2 => 2,
        3 => 13,
        4 => 1,
        _ => 0,
    };
    bad_type != 0 && ty as usize == bad_type
}
fn reset_mock() {
    crate::STOPPED.store(false, SeqCst);
    MAJOR.store(6000, SeqCst);
    NOTIFY_ARGC.store(0, SeqCst);
    DONE.store(false, SeqCst);
    EXCEPTION.store(false, SeqCst);
    ROOTS.store(0, SeqCst);
    ROOT_CALLS.store(0, SeqCst);
    NOTIFICATIONS.store(0, SeqCst);
    COLLECTED.store(0, SeqCst);
    BAD_RETURN.store(false, SeqCst);
    STRIPPED.store(false, SeqCst);
    BAD_BYREF.store(0, SeqCst);
    REENTER_STOP.store(false, SeqCst);
    UNBOX_CALLS.store(0, SeqCst);
}
fn api() -> Il2cpp {
    unsafe { Il2cpp::bind(0x1000, so_symbol, REQUIRED_EXPORTS) }.unwrap()
}
#[test]
fn both_notification_shapes_exceptions_async_completion_and_exact_root_release() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    reset_mock();
    for (major, notify_argc) in [(6000, 0), (6000, 1)] {
        MAJOR.store(major, SeqCst);
        NOTIFY_ARGC.store(notify_argc, SeqCst);
        DONE.store(false, SeqCst);
        let mut engine = Engine::discover(api()).unwrap();
        engine.notify().unwrap();
        EXCEPTION.store(true, SeqCst);
        assert!(engine.request().is_err());
        assert_eq!(ROOTS.load(SeqCst), 0);
        engine.request().unwrap();
        assert!(engine.request().is_err(), "no overlapping requests");
        assert!(!engine.poll().unwrap(), "request is not completion");
        assert_eq!(ROOTS.load(SeqCst), 1);
        DONE.store(true, SeqCst);
        assert!(engine.poll().unwrap());
        assert_eq!(ROOTS.load(SeqCst), 0);
        engine.collect();
        DONE.store(false, SeqCst);
        engine.request().unwrap();
        drop(engine); // UnityStopping releases a still-pending operation.
        assert_eq!(ROOTS.load(SeqCst), 0);
    }
    assert_eq!(NOTIFICATIONS.load(SeqCst), 2);
    assert_eq!(COLLECTED.load(SeqCst), 2);
    BAD_RETURN.store(true, SeqCst);
    let engine = Engine::discover(api()).unwrap();
    assert!(!engine.supports_cleanup());
    assert_eq!(
        engine.cleanup_unavailable(),
        Some("UnloadUnusedAssets/isDone return type mismatch")
    );
    BAD_RETURN.store(false, SeqCst);
    // Unverified builds are rejected BEFORE any root creation.
    let root_calls = ROOT_CALLS.load(SeqCst);
    NOTIFY_ARGC.store(0, SeqCst);
    for version in [2021, 6001] {
        MAJOR.store(version, SeqCst);
        let mut engine = Engine::discover(api()).unwrap();
        assert!(engine.supports_notification());
        assert!(!engine.supports_cleanup());
        assert!(engine
            .cleanup_unavailable()
            .unwrap()
            .contains("unverified Unity build"));
        assert!(engine.request().is_err());
        assert_eq!(ROOTS.load(SeqCst), 0);
        assert_eq!(ROOT_CALLS.load(SeqCst), root_calls);
    }
    // Missing optional collector/measurement exports do not suppress
    // a validated developer notification or create an unrooted unload.
    MAJOR.store(6000, SeqCst);
    STRIPPED.store(true, SeqCst);
    let stripped = api();
    STRIPPED.store(false, SeqCst);
    let mut engine = Engine::discover(stripped).unwrap();
    assert!(engine.notify().unwrap());
    assert!(!engine.supports_cleanup());
    assert_eq!(
        engine.cleanup_unavailable(),
        Some("GC handle exports unavailable")
    );
    assert!(!engine.collect());
    assert_eq!(engine.heap_bytes(), (None, None));
    assert!(engine.request().is_err());
    assert_eq!(ROOT_CALLS.load(SeqCst), root_calls);
    // Exercise the legacy typed adapter independently; this does not
    // add a verified production entry for the simulated old build.
    MAJOR.store(2021, SeqCst);
    let handles = unsafe { api().gc_handles(GcHandleAbi::U32) }.unwrap();
    let root = unsafe { handles.root(100usize as Ptr) }.unwrap();
    assert_eq!(root.target() as usize, 100);
    drop(root);
    assert_eq!(ROOTS.load(SeqCst), 0);
}

#[test]
fn byref_metadata_must_reject_notification_and_collector_methods() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    reset_mock();
    for bad in 1..=4 {
        BAD_BYREF.store(bad, SeqCst);
        NOTIFY_ARGC.store(if bad == 3 { 1 } else { 0 }, SeqCst);
        let engine = Engine::discover(api()).unwrap();
        if bad <= 2 {
            assert!(
                !engine.supports_cleanup(),
                "byref collector ABI {bad} must be rejected"
            );
        } else {
            assert!(
                !engine.supports_notification(),
                "byref notification ABI {bad} must be rejected"
            );
        }
    }
    BAD_BYREF.store(0, SeqCst);
}

#[test]
fn diagnostic_budget_never_notifies_unloads_or_collects_even_under_pressure() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    reset_mock();
    MAJOR.store(6000, SeqCst);
    let config = crate::Config {
        reserve_kib: u64::MAX / 2,
        recovery_kib: u64::MAX,
        cooldown: std::time::Duration::from_secs(30),
        warmup: std::time::Duration::ZERO,
        collect_managed: true,
        notify: true,
        unload: true,
        notification_grace: std::time::Duration::ZERO,
        diagnostic_only: true,
        diagnostic_interval: std::time::Duration::from_secs(60),
        diagnostic_samples: 2,
    };
    let mut controller = crate::Controller::new(api(), config).unwrap();
    controller.tick();
    assert_eq!(controller.diagnostic_remaining, 1);
    controller.tick();
    assert_eq!(controller.diagnostic_remaining, 1, "interval must bound file/engine reads");
    controller.next_diagnostic = std::time::Duration::ZERO;
    controller.tick();
    assert!(controller.disabled);
    for _ in 0..8 { controller.tick(); }
    assert_eq!(controller.diagnostic_remaining, 0);
    assert_eq!(NOTIFICATIONS.load(SeqCst), 0);
    assert_eq!(ROOT_CALLS.load(SeqCst), 0);
    assert_eq!(COLLECTED.load(SeqCst), 0);
    assert!(controller.pending_since.is_none());
}

#[test]
fn stopping_reentrant_from_poll_releases_root_and_does_not_restore_controller() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    reset_mock();
    let mut host = rudroid_common::host_api::HostApiV0::stubbed();
    extern "C" fn silent(_: HostStr<'_>, _: HostStr<'_>) {}
    host.log = silent;
    assert!(crate::HOST.set(host).is_ok());
    let config = crate::Config {
        reserve_kib: 64,
        recovery_kib: 128,
        cooldown: std::time::Duration::from_secs(30),
        warmup: std::time::Duration::ZERO,
        collect_managed: false,
        notify: true,
        unload: true,
        notification_grace: std::time::Duration::from_secs(1),
        diagnostic_only: false,
        diagnostic_interval: std::time::Duration::from_secs(5),
        diagnostic_samples: 32,
    };
    assert!(crate::CONFIG.set(config).is_ok());
    let mut controller = crate::Controller::new(api(), config).unwrap();
    controller.engine.request().unwrap();
    controller.pending_since = Some(std::time::Instant::now());
    crate::SLOT.with(|cell| cell.replace(crate::Slot::Active(controller)));
    REENTER_STOP.store(true, SeqCst);
    let event = rudroid_common::registry::RudroidModuleEvent {
        kind: rudroid_common::ModuleKind::UnityMaintenance as u32,
        base: 0,
        name: HostStr::new(c"runtime/unity_maintenance"),
    };
    crate::lifecycle(Some(&event), std::ptr::null_mut());
    assert_eq!(
        ROOTS.load(SeqCst),
        0,
        "pending root must be released before nativeDone"
    );
    assert_eq!(
        UNBOX_CALLS.load(SeqCst),
        0,
        "no managed access after stop request"
    );
    crate::SLOT.with(|cell| assert!(matches!(*cell.borrow(), crate::Slot::Stopped)));
}
