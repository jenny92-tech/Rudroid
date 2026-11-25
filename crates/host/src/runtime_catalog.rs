//! INPUT: provider registrations, queries and shutdown
//! OUTPUT: generation-scoped leases and exactly-once destruction
//! POS: loader catalog registry lifecycle
#![forbid(unsafe_code)]

use super::*;

/// Lifecycle of the catalog provider slot (tickets 61/66). The states make
/// illegal interleavings unrepresentable: an in-flight counter only exists
/// while its generation can legally hold leases, and a destroy that has not
/// returned yet is visible as [`CatalogSlot::Destroying`], so no new
/// generation can slip in behind it.
#[derive(Default)]
pub(super) enum CatalogSlot {
    /// Open for queries: `provider` answers, `in_flight` query callbacks are
    /// currently running outside the lock.
    Open {
        provider: CatalogProviderV0,
        in_flight: u32,
    },
    /// `shutdown` marked this generation: new queries miss and new providers
    /// are rejected; the last in-flight query's epilogue performs the destroy.
    Draining {
        provider: CatalogProviderV0,
        in_flight: u32,
    },
    /// The provider copy has been taken for destruction, which runs OUTSIDE
    /// the registry lock (the callback may re-enter the registry). This is
    /// the ticket-66 generation barrier: registrations are rejected until the
    /// destroy callback returns and the slot becomes [`CatalogSlot::Vacant`].
    Destroying,
    /// No provider; a fresh registration starts a new generation.
    #[default]
    Vacant,
}

pub fn register_catalog_provider(provider: Option<&CatalogProviderV0>) -> c_int {
    let Some(provider) = provider else {
        return NULL_ARG;
    };
    let mut state = registry().lock().unwrap();
    // Ticket 66 generation barrier: a new provider registers only into a
    // `Vacant` slot. While the old generation still drains (in-flight
    // queries) or its destroy callback has not returned yet, registration is
    // deterministically rejected and the caller keeps ownership.
    if !matches!(state.catalog, CatalogSlot::Vacant) {
        return REGISTRY_REJECTED;
    }
    state.catalog = CatalogSlot::Open {
        provider: *provider,
        in_flight: 0,
    };
    0
}

/// Release one catalog lease (the query-epilogue half of the slot state
/// machine, ticket 66). Returns the provider to destroy when this was the
/// last in-flight user of a draining slot. Illegal states panic: an
/// in-flight underflow is a state-machine bug and must never be smoothed
/// over with `saturating_sub`.
pub(super) fn release_lease(slot: &mut CatalogSlot) -> Option<CatalogProviderV0> {
    match slot {
        CatalogSlot::Open { in_flight, .. } => {
            assert!(*in_flight > 0, "catalog lease underflow on an open slot");
            *in_flight -= 1;
            None
        }
        CatalogSlot::Draining {
            provider,
            in_flight,
        } => {
            assert!(*in_flight > 0, "catalog lease underflow on a draining slot");
            *in_flight -= 1;
            if *in_flight == 0 {
                // Exclusive with `shutdown`: the first side to reach zero
                // in-flight moves the slot to `Destroying` and gets the
                // provider copy (exactly-once destroy).
                let provider = *provider;
                *slot = CatalogSlot::Destroying;
                Some(provider)
            } else {
                None
            }
        }
        CatalogSlot::Destroying | CatalogSlot::Vacant => {
            panic!("catalog query epilogue without an active lease");
        }
    }
}

fn release_catalog_lease() -> Option<CatalogProviderV0> {
    let mut state = registry().lock().unwrap();
    release_lease(&mut state.catalog)
}

/// Run the destroy callback OUTSIDE the registry lock (it may re-enter the
/// registry) and only reopen the slot for the next generation after the
/// callback returned — the ticket-66 barrier.
pub(super) fn destroy_provider_and_reopen(provider: CatalogProviderV0) {
    if let Some(destroy) = provider.destroy {
        // This generation's provider copy was handed to exactly one drainer;
        // the registry lock is not held here and every registry entry point
        // the callback might call observes `Destroying`.
        destroy(provider.user);
    }
    let mut state = registry().lock().unwrap();
    assert!(
        matches!(state.catalog, CatalogSlot::Destroying),
        "destroy epilogue on a slot that is not being destroyed"
    );
    state.catalog = CatalogSlot::Vacant;
}

fn query_with<R>(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut R>,
    call: impl Fn(
        &CatalogProviderV0,
        *mut c_void,
        HostStr<'_>,
        HostStr<'_>,
        HostStr<'_>,
        Option<&mut R>,
    ) -> c_int,
) -> c_int {
    // Lease the provider for this query (ticket 61): the copy plus an
    // in-flight bump happen under the lock, so shutdown cannot destroy the
    // provider while the callback is running. Only an `Open` slot leases;
    // draining, destroying and vacant slots miss cleanly.
    let provider = {
        let mut state = registry().lock().unwrap();
        match &mut state.catalog {
            CatalogSlot::Open {
                provider,
                in_flight,
            } => {
                // Exhaustion must miss rather than wrap and let shutdown
                // destroy a provider with outstanding callbacks.
                let Some(next) = in_flight.checked_add(1) else {
                    return 0;
                };
                *in_flight = next;
                *provider
            }
            CatalogSlot::Draining { .. } | CatalogSlot::Destroying | CatalogSlot::Vacant => {
                return 0
            }
        }
    };
    // The callback runs outside the registry lock and may re-enter.
    let _lease = CatalogLease;
    (call)(&provider, provider.user, class, method, sig, out)
}

struct CatalogLease;
impl Drop for CatalogLease {
    fn drop(&mut self) {
        // Release the lease; the last in-flight user of a draining slot takes
        // the provider for destruction (exactly once — the transition to
        // `Destroying` is exclusive with shutdown) and the destroy stays outside
        // the lock, opening the generation barrier only when it returns.
        if let Some(provider) = release_catalog_lease() {
            destroy_provider_and_reopen(provider);
        }
    }
}

pub fn catalog_query_int(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int {
    query_with(class, method, sig, out, |provider, user, c, m, s, out| {
        provider
            .query_int
            .map_or(0, |query| query(user, c, m, s, out))
    })
}

pub fn catalog_query_bool(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut c_int>,
) -> c_int {
    query_with(class, method, sig, out, |provider, user, c, m, s, out| {
        provider
            .query_bool
            .map_or(0, |query| query(user, c, m, s, out))
    })
}

/// `shutdown` half of the slot state machine (tickets 61/66): stop the slot
/// from leasing and decide who performs the destroy. Returns the provider
/// when the caller must destroy it right away (no query in flight); `None`
/// when an in-flight query's epilogue will, when a destroy is already in
/// progress (never wait for it, never destroy twice), or when nothing is
/// registered.
pub(super) fn begin_shutdown(slot: &mut CatalogSlot) -> Option<CatalogProviderV0> {
    match slot {
        CatalogSlot::Open {
            provider,
            in_flight: 0,
        } => {
            let provider = *provider;
            *slot = CatalogSlot::Destroying;
            Some(provider)
        }
        CatalogSlot::Open {
            provider,
            in_flight,
        } => {
            *slot = CatalogSlot::Draining {
                provider: *provider,
                in_flight: *in_flight,
            };
            None
        }
        CatalogSlot::Draining { .. } | CatalogSlot::Destroying | CatalogSlot::Vacant => None,
    }
}
