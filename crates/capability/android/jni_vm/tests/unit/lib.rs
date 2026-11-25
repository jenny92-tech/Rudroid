//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

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
