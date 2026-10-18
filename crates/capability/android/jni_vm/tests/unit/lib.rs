//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use std::sync::{Arc, Mutex};

/// Test resolver over the shared in-repo source table. Test-only: the
/// capability itself has no compile-time catalog (ticket 43).
fn source_resolver() -> Resolver {
    Resolver::with_catalog(Catalog::from_source())
}

#[test]
fn catalog_hits_return_the_fixed_replies() {
    let resolver = source_resolver();
    assert!(!resolver.resolve_bool(
        &MethodKey::new("android/media/AudioManager", "isBluetoothA2dpOn", "()Z"),
        true
    ));
    assert_eq!(
        resolver.resolve_int(
            &MethodKey::new("android/media/AudioManager", "getStreamVolume", "(I)I"),
            -1
        ),
        100
    );
    assert_eq!(
        resolver.resolve_int(
            &MethodKey::new("android/media/AudioDeviceInfo", "getType", "()I"),
            -1
        ),
        4
    );
}

#[test]
fn misses_return_type_default_and_log_once() {
    let misses: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let handle = misses.clone();
    let mut resolver = source_resolver();
    resolver.on_miss = Some(Box::new(move |key: &MethodKey| {
        handle.lock().unwrap().push(format!(
            "STUB-MISS {}/{}{}",
            key.class, key.method, key.sig
        ));
    }));

    let key = MethodKey::new("some/Cls", "absent", "()I");
    assert_eq!(resolver.resolve_int(&key, 42), 42);
    assert!(resolver.resolve_bool(&key, true));
    assert_eq!(resolver.resolve_str(&key, "dflt"), "dflt");
    let logged = misses.lock().unwrap().clone();
    assert_eq!(logged.len(), 3, "one STUB-MISS per resolver call: {logged:?}");
    assert!(logged[0].starts_with("STUB-MISS some/Cls/absent()I"));
}

#[test]
fn hit_does_not_log_miss() {
    let mut resolver = source_resolver();
    resolver.on_miss = Some(Box::new(|_: &MethodKey| {
        panic!("hit must not be logged as a miss")
    }));
    let key = MethodKey::new("android/media/AudioManager", "getStreamVolume", "(I)I");
    assert_eq!(resolver.resolve_int(&key, 0), 100);
}

#[test]
fn type_mismatch_counts_as_miss() {
    // The entry is a bool: an int resolver applies the default (documented
    // missing-key behavior — typed accessors are strict about type).
    let resolver = source_resolver();
    let key = MethodKey::new("android/media/AudioManager", "isBluetoothA2dpOn", "()Z");
    assert_eq!(resolver.resolve_int(&key, 7), 7);
}

#[test]
fn wrong_signature_is_a_miss() {
    let resolver = source_resolver();
    let key = MethodKey::new("android/media/AudioManager", "getStreamVolume", "(I)V");
    assert!(resolver.hit(&key).is_none());
}

/// Ticket 52: the early-init phase registers the session's JavaVM* in
/// the (stub) shared-VM slot and is idempotent across repeated
/// bring-ups in one process. Takes the crate-shared SERIAL lock: the
/// stub catalog table and the shared-VM slot mirror are process-global
/// and the session tests use the same lock.
#[test]
fn early_init_registers_the_shared_vm_slot_idempotently() {
    use crate::session::test_support;

    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    let api: &'static _ = Box::leak(Box::new(test_support::stub_host()));
    assert_eq!(rudroid_plugin_early_init(Some(api)), 0, "first registration");
    let vm = (api.shared_vm)();
    assert_eq!(vm.unwrap().as_raw(), abi::java_vm_ptr());
    assert_eq!(rudroid_plugin_early_init(Some(api)), 0, "repeat bring-up is idempotent");
    // Wrong ABI version and null table fail explicitly.
    let mut wrong = *api;
    wrong.abi_version = rudroid_common::host_api::HOST_API_VERSION + 1;
    assert_eq!(rudroid_plugin_early_init(Some(Box::leak(Box::new(wrong)))), -2);
    assert_eq!(rudroid_plugin_early_init(None), -1);
    test_support::reset_shared_vm_slot();
    test_support::stub_clear();
}
