//! INPUT:  private items of `src/runtime.rs` via `super`
//! OUTPUT:  `catalog_state_machine_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/runtime.rs`; declared there with `#[path]`
//! Ticket 66: the lease bookkeeping must expose an in-flight underflow
//! as an assertion instead of a `saturating_sub` that silently masks a
//! state-machine bug. The public surface cannot legally reach the
//! underflow (leases and slot transitions are exclusive under the
//! registry lock, and registration during a drain is rejected), so these
//! tests drive the transition function directly on a hand-built slot —
//! no global registry state involved.

use super::*;

fn probe_provider() -> CatalogProviderV0 {
    CatalogProviderV0 {
        user: std::ptr::null_mut(),
        query_int: None,
        query_bool: None,
        destroy: None,
    }
}

#[test]
#[should_panic(expected = "catalog lease underflow on an open slot")]
fn open_slot_lease_underflow_panics_instead_of_saturating() {
    let mut slot = CatalogSlot::Open {
        provider: probe_provider(),
        in_flight: 0,
    };
    let _ = release_lease(&mut slot);
}

#[test]
#[should_panic(expected = "catalog lease underflow on a draining slot")]
fn draining_slot_lease_underflow_panics_instead_of_saturating() {
    let mut slot = CatalogSlot::Draining {
        provider: probe_provider(),
        in_flight: 0,
    };
    let _ = release_lease(&mut slot);
}

#[test]
fn last_draining_lease_hands_the_provider_to_the_caller() {
    let provider = probe_provider();
    let mut slot = CatalogSlot::Draining {
        provider,
        in_flight: 2,
    };
    assert!(release_lease(&mut slot).is_none());
    assert!(
        matches!(slot, CatalogSlot::Draining { in_flight: 1, .. }),
        "an earlier lease keeps the slot draining"
    );
    let torn_down = release_lease(&mut slot).expect("the last lease tears down");
    assert!(std::ptr::eq(torn_down.user, provider.user));
    assert!(
        matches!(slot, CatalogSlot::Destroying),
        "the drained slot enters the generation barrier"
    );
}
