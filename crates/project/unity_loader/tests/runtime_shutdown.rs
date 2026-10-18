//! Ticket 61/66 acceptance: registry shutdown must respect in-flight catalog
//! queries and generation boundaries.
//!
//! - `destroy` never runs while one of the provider's query callbacks is
//!   active; the last in-flight user tears the provider down.
//! - Queries after `shutdown` miss cleanly; repeated and provider-less
//!   shutdowns are no-ops; each provider is destroyed exactly once and a
//!   next-generation provider registered after the drain is not touched by
//!   the old drain.
//! - Callbacks stay outside the registry lock: re-entrant queries and a
//!   `shutdown` called from inside a query callback must not deadlock.
//! - Ticket 66: while the old generation drains or its `destroy` callback
//!   has not returned yet, a new provider registration is deterministically
//!   rejected; the barrier opens only when the destroy returned. The same
//!   policy holds for registry calls re-entered from inside a destroy
//!   callback, and no delayed old-generation teardown can clear or destroy
//!   a newer generation. (The `in_flight` underflow assert is covered by
//!   `catalog_state_machine_tests` in `src/runtime.rs` — the integration
//!   surface cannot legally construct that state.)
//!
//! The registry is process-global state, so every test serializes on one
//! mutex. Providers here are test-owned probes whose `destroy` only observes
//! timing (no real free), so the old premature-destroy behavior is
//! reproduced without an actual use-after-free.

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{mpsc, Barrier, Condvar, Mutex};
use std::time::Duration;

use rudroid_common::registry::{CatalogProviderV0, HostStr};
use rudroid_unity_loader::runtime;

/// Serializes every test in this binary: the registry is process-global.
static SERIAL: Mutex<()> = Mutex::new(());

/// Probe provider state. Blocking probes park their query callback on
/// barriers so the test controls exactly when it returns; non-blocking
/// probes answer immediately. The destroy handler only records timing —
/// it never frees or invalidates `user` (the owning test drops the box
/// after shutdown), which reproduces the old premature-destroy behavior
/// without an actual use-after-free.
struct Probe {
    blocking: bool,
    entered: Barrier,
    resume: Barrier,
    active: AtomicBool,
    destroyed_while_active: AtomicBool,
    destroys: AtomicUsize,
    /// Signalled from `destroy` so a test can observe the destruction
    /// deterministically instead of racing a flag read against the shutdown
    /// thread (receivers are dropped in tests that don't need it).
    destruction: mpsc::Sender<()>,
}

impl Probe {
    fn blocking() -> Box<Self> {
        let (tx, _rx) = mpsc::channel();
        Self::blocking_with_notify(tx)
    }

    /// Blocking probe whose destroy events are observable via the receiver.
    fn blocking_with_notify(tx: mpsc::Sender<()>) -> Box<Self> {
        Box::new(Self {
            blocking: true,
            entered: Barrier::new(2),
            resume: Barrier::new(2),
            active: AtomicBool::new(false),
            destroyed_while_active: AtomicBool::new(false),
            destroys: AtomicUsize::new(0),
            destruction: tx,
        })
    }

    fn immediate() -> Box<Self> {
        let (tx, _rx) = mpsc::channel();
        Box::new(Self {
            blocking: false,
            entered: Barrier::new(2),
            resume: Barrier::new(2),
            active: AtomicBool::new(false),
            destroyed_while_active: AtomicBool::new(false),
            destroys: AtomicUsize::new(0),
            destruction: tx,
        })
    }

    fn provider(this: &mut Box<Self>) -> CatalogProviderV0 {
        CatalogProviderV0 {
            user: (&mut **this as *mut Probe).cast(),
            query_int: Some(query),
            query_bool: None,
            destroy: Some(destroy),
        }
    }
}

extern "C" fn query(
    user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    // SAFETY: the owning test keeps the Probe box alive until shutdown has
    // run; `destroy` never frees or invalidates it.
    let probe = unsafe { &*(user as *const Probe) };
    probe.active.store(true, Ordering::SeqCst);
    if probe.blocking {
        probe.entered.wait();
        probe.resume.wait();
    }
    probe.active.store(false, Ordering::SeqCst);
    1
}

extern "C" fn destroy(user: *mut c_void) {
    // SAFETY: see `query`.
    let probe = unsafe { &*(user as *const Probe) };
    probe.destroys.fetch_add(1, Ordering::SeqCst);
    if probe.active.load(Ordering::SeqCst) {
        probe.destroyed_while_active.store(true, Ordering::SeqCst);
    }
    let _ = probe.destruction.send(());
}

/// Query on a worker thread with a result deadline, so a future deadlock
/// regresses as a test failure instead of a hung CI run.
fn query_int_with_deadline() -> c_int {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = 0;
        let hit = runtime::catalog_query_int(
            HostStr::new(c"class"),
            HostStr::new(c"method"),
            HostStr::new(c"()I"),
            Some(&mut out),
        );
        tx.send(hit).expect("receiver still waiting");
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("catalog query must not deadlock")
}

#[test]
fn shutdown_must_not_destroy_provider_while_query_is_in_flight() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let (tx, rx) = mpsc::channel();
    let mut probe = Probe::blocking_with_notify(tx);
    assert_eq!(runtime::register_catalog_provider(Some(&Probe::provider(&mut probe))), 0);

    let querying = std::thread::spawn(|| {
        let mut out = 0;
        runtime::catalog_query_int(HostStr::new(c"class"), HostStr::new(c"method"), HostStr::new(c"()I"), Some(&mut out))
    });
    probe.entered.wait();
    let stopping = std::thread::spawn(runtime::shutdown);
    // Deterministic observation (the review probe's pattern): the old
    // implementation destroys the provider right away, so the channel fires
    // while the callback is parked; an implementation that keeps the provider
    // alive until the query returns times out here.
    let destroyed_during = match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(()) => probe.destroyed_while_active.load(Ordering::SeqCst),
        Err(_) => false,
    };
    probe.resume.wait();
    assert_eq!(querying.join().unwrap(), 1);
    stopping.join().unwrap();
    runtime::shutdown();
    assert!(
        !destroyed_during,
        "provider destroy ran while its query callback was still active; \
         the real catalog destroy frees its Box here"
    );
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1, "destroyed exactly once");
}

#[test]
fn shutdown_repeats_and_post_shutdown_queries_miss_cleanly() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    // Provider-less shutdown is a no-op.
    runtime::shutdown();
    let mut probe = Probe::immediate();
    assert_eq!(runtime::register_catalog_provider(Some(&Probe::provider(&mut probe))), 0);
    assert_eq!(query_int_with_deadline(), 1);
    runtime::shutdown();
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1);
    // Queries after shutdown miss instead of calling the destroyed provider.
    assert_eq!(query_int_with_deadline(), 0, "post-shutdown queries must miss");
    // Repeated shutdown: no second destroy, no panic.
    runtime::shutdown();
    runtime::shutdown();
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1, "destroyed exactly once");
}

#[test]
fn drain_happens_before_the_next_generation_registers() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    // Generation A: a query is in flight while shutdown marks the slot
    // closing; the destroy must happen only after the query returns.
    let mut probe_a = Probe::blocking();
    assert_eq!(runtime::register_catalog_provider(Some(&Probe::provider(&mut probe_a))), 0);
    let querying = std::thread::spawn(|| {
        let mut out = 0;
        runtime::catalog_query_int(HostStr::new(c"class"), HostStr::new(c"method"), HostStr::new(c"()I"), Some(&mut out))
    });
    probe_a.entered.wait();
    runtime::shutdown();
    assert_eq!(
        probe_a.destroys.load(Ordering::SeqCst),
        0,
        "shutdown with an in-flight query must defer the destroy"
    );
    probe_a.resume.wait();
    assert_eq!(querying.join().unwrap(), 1);
    assert_eq!(
        probe_a.destroys.load(Ordering::SeqCst),
        1,
        "the last in-flight user tears the provider down"
    );
    assert!(!probe_a.destroyed_while_active.load(Ordering::SeqCst));

    // Generation B: registered only after A drained; its lifecycle is fresh.
    let mut probe_b = Probe::immediate();
    assert_eq!(
        runtime::register_catalog_provider(Some(&Probe::provider(&mut probe_b))),
        0,
        "a next-generation provider registers after the old one drained"
    );
    assert_eq!(query_int_with_deadline(), 1, "the new provider answers queries");
    runtime::shutdown();
    assert_eq!(probe_b.destroys.load(Ordering::SeqCst), 1);
    assert!(
        !probe_b.destroyed_while_active.load(Ordering::SeqCst),
        "the old drain must never touch the next-generation provider"
    );
}

/// The callback queries the registry once more from inside itself; the flag
/// stops the recursion. Set false by the test before registering.
static NESTED_TAKEN: AtomicBool = AtomicBool::new(false);

extern "C" fn nested_query(
    user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int {
    // SAFETY: the owning test keeps the box alive until shutdown ran.
    let probe = unsafe { &*(user as *const Probe) };
    probe.active.store(true, Ordering::SeqCst);
    if !NESTED_TAKEN.swap(true, Ordering::SeqCst) {
        // A nested query from inside the callback must not deadlock: the
        // registry lock is never held across callbacks and the in-flight
        // count is per-entry, not per-thread.
        let _ = runtime::catalog_query_int(class, method, sig, out);
    }
    probe.active.store(false, Ordering::SeqCst);
    1
}

#[test]
fn reentrant_query_from_callback_does_not_deadlock_and_drains_cleanly() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    NESTED_TAKEN.store(false, Ordering::SeqCst);

    let mut probe = Probe::immediate();
    let provider = CatalogProviderV0 {
        query_int: Some(nested_query),
        ..Probe::provider(&mut probe)
    };
    assert_eq!(runtime::register_catalog_provider(Some(&provider)), 0);
    assert_eq!(
        query_int_with_deadline(),
        1,
        "outer query with a nested re-entrant query must not deadlock"
    );
    runtime::shutdown();
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1, "destroyed exactly once");
    assert!(!probe.destroyed_while_active.load(Ordering::SeqCst));
}

/// The query callback triggers `shutdown` while it is in flight. Shutdown
/// must not wait for the callback (that would deadlock against itself) and
/// the destroy must run only after the callback returned.
extern "C" fn sic_query(
    user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    // SAFETY: the owning test keeps the box alive until shutdown ran.
    let probe = unsafe { &*(user as *const Probe) };
    probe.active.store(true, Ordering::SeqCst);
    runtime::shutdown();
    probe.active.store(false, Ordering::SeqCst);
    1
}

#[test]
fn shutdown_from_inside_a_query_callback_does_not_deadlock() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let mut probe = Probe::immediate();
    let provider = CatalogProviderV0 {
        query_int: Some(sic_query),
        ..Probe::provider(&mut probe)
    };
    assert_eq!(runtime::register_catalog_provider(Some(&provider)), 0);
    assert_eq!(
        query_int_with_deadline(),
        1,
        "callback-triggered shutdown must not deadlock"
    );
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1, "destroyed exactly once");
    assert!(
        !probe.destroyed_while_active.load(Ordering::SeqCst),
        "the destroy must wait for the callback to return"
    );
}

// ------------------------------------------------------------------
// Ticket 66 probe (merged verbatim from the R2 review, see
// `.scratch/parallel-rust-crates/review-2026-09-15-r2/runtime-destroy-overlap.rs`):
// while the OLD generation's destroy callback is still running (blocked on a
// condvar here), a NEW generation must not be able to register — the probe
// asserts the registration is rejected with `REGISTRY_REJECTED` (1). The
// helper names are prefixed `overlap_` to avoid collisions with the ticket-61
// probe above; semantics and assertions are unchanged.
// ------------------------------------------------------------------

/// Probe state for the overlap test: `destroy` signals `entered` and then
/// parks on a condvar until the test releases it.
struct OverlapProbe {
    entered: mpsc::Sender<()>,
    release: (Mutex<bool>, Condvar),
}

extern "C" fn overlap_destroy(user: *mut c_void) {
    // SAFETY: the owning test keeps the probe box alive past the destroy.
    let probe = unsafe { &*(user as *const OverlapProbe) };
    let _ = probe.entered.send(());
    let (lock, wake) = &probe.release;
    let mut released = lock.lock().unwrap();
    while !*released {
        released = wake.wait(released).unwrap();
    }
}

extern "C" fn overlap_query(
    _user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    1
}

fn overlap_provider(
    user: *mut c_void,
    destroy: Option<extern "C" fn(*mut c_void)>,
) -> CatalogProviderV0 {
    CatalogProviderV0 {
        user,
        query_int: Some(overlap_query),
        query_bool: None,
        destroy,
    }
}

#[test]
fn next_generation_waits_until_previous_destroy_returns() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let (entered_tx, entered_rx) = mpsc::channel();
    let mut old = Box::new(OverlapProbe {
        entered: entered_tx,
        release: (Mutex::new(false), Condvar::new()),
    });
    let old_provider =
        overlap_provider((&mut *old as *mut OverlapProbe).cast(), Some(overlap_destroy));
    assert_eq!(runtime::register_catalog_provider(Some(&old_provider)), 0);

    let stopping = std::thread::spawn(runtime::shutdown);
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("old provider destroy entered");

    let mut new_user = 0u8;
    let new_provider = overlap_provider((&mut new_user as *mut u8).cast(), None);
    let rc_while_old_destroy_runs = runtime::register_catalog_provider(Some(&new_provider));

    let (lock, wake) = &old.release;
    *lock.lock().unwrap() = true;
    wake.notify_all();
    stopping.join().unwrap();
    runtime::shutdown();

    assert_eq!(
        rc_while_old_destroy_runs, 1,
        "a new generation must not register until the old destroy callback returns"
    );
}

// ------------------------------------------------------------------
// Ticket 66 acceptance coverage: the generation barrier on both destroy
// paths (shutdown thread and last in-flight query epilogue), re-entrant
// registry calls from inside a destroy callback, and a generation chain
// where no delayed old-generation teardown touches a newer generation.
// ------------------------------------------------------------------

/// Probe whose `destroy` signals `destroy_entered` and then parks on a
/// condvar until the test releases it; the query callback answers
/// immediately. Opens the ticket-66 barrier window deterministically.
struct GateProbe {
    destroy_entered: mpsc::Sender<()>,
    /// Receiver side of `destroy_entered`, kept in the probe so the test can
    /// wait for the signal deterministically.
    entered_rx: Mutex<mpsc::Receiver<()>>,
    release: (Mutex<bool>, Condvar),
    destroys: AtomicUsize,
}

impl GateProbe {
    fn boxed() -> Box<Self> {
        let (tx, rx) = mpsc::channel();
        Box::new(Self {
            destroy_entered: tx,
            entered_rx: Mutex::new(rx),
            release: (Mutex::new(false), Condvar::new()),
            destroys: AtomicUsize::new(0),
        })
    }

    /// Waits (bounded) until the destroy callback signalled its entry.
    fn wait_destroy_entered(&self) {
        self.entered_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("destroy entered");
    }

    fn release(&self) {
        let (lock, wake) = &self.release;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }

    fn provider(this: &mut Box<Self>) -> CatalogProviderV0 {
        CatalogProviderV0 {
            user: (&mut **this as *mut GateProbe).cast(),
            query_int: Some(gate_query),
            query_bool: None,
            destroy: Some(gate_destroy),
        }
    }
}

extern "C" fn gate_query(
    _user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    1
}

extern "C" fn gate_destroy(user: *mut c_void) {
    // SAFETY: the owning test keeps the probe box alive past the destroy.
    let probe = unsafe { &*(user as *const GateProbe) };
    probe.destroys.fetch_add(1, Ordering::SeqCst);
    let _ = probe.destroy_entered.send(());
    let (lock, wake) = &probe.release;
    let mut released = lock.lock().unwrap();
    while !*released {
        released = wake.wait(released).unwrap();
    }
}

/// Probe whose query parks on barriers AND whose destroy parks on a condvar:
/// reproduces the in-flight-query drain whose epilogue destroy is then
/// observed through the barrier window.
struct DrainGateProbe {
    query_entered: Barrier,
    query_resume: Barrier,
    destroy_entered: mpsc::Sender<()>,
    /// Receiver side of `destroy_entered`, kept in the probe so the test can
    /// wait for the signal deterministically.
    entered_rx: Mutex<mpsc::Receiver<()>>,
    release: (Mutex<bool>, Condvar),
    destroys: AtomicUsize,
}

impl DrainGateProbe {
    fn boxed() -> Box<Self> {
        let (tx, rx) = mpsc::channel();
        Box::new(Self {
            query_entered: Barrier::new(2),
            query_resume: Barrier::new(2),
            destroy_entered: tx,
            entered_rx: Mutex::new(rx),
            release: (Mutex::new(false), Condvar::new()),
            destroys: AtomicUsize::new(0),
        })
    }

    /// Waits (bounded) until the destroy callback signalled its entry.
    fn wait_destroy_entered(&self) {
        self.entered_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("destroy entered");
    }

    fn release(&self) {
        let (lock, wake) = &self.release;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }

    fn provider(this: &mut Box<Self>) -> CatalogProviderV0 {
        CatalogProviderV0 {
            user: (&mut **this as *mut DrainGateProbe).cast(),
            query_int: Some(drain_gate_query),
            query_bool: None,
            destroy: Some(drain_gate_destroy),
        }
    }
}

extern "C" fn drain_gate_query(
    user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    // SAFETY: the owning test keeps the probe box alive until shutdown ran.
    let probe = unsafe { &*(user as *const DrainGateProbe) };
    probe.query_entered.wait();
    probe.query_resume.wait();
    1
}

extern "C" fn drain_gate_destroy(user: *mut c_void) {
    // SAFETY: the owning test keeps the probe box alive past the destroy.
    let probe = unsafe { &*(user as *const DrainGateProbe) };
    probe.destroys.fetch_add(1, Ordering::SeqCst);
    let _ = probe.destroy_entered.send(());
    let (lock, wake) = &probe.release;
    let mut released = lock.lock().unwrap();
    while !*released {
        released = wake.wait(released).unwrap();
    }
}

/// Checklist: no in-flight query — the shutdown thread performs the destroy
/// itself, and a concurrent register is rejected before the destroy callback
/// returns and accepted afterwards.
#[test]
fn shutdown_destroy_barrier_rejects_register_until_the_callback_returns() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let mut gen1 = GateProbe::boxed();
    assert_eq!(runtime::register_catalog_provider(Some(&GateProbe::provider(&mut gen1))), 0);

    let stopping = std::thread::spawn(runtime::shutdown);
    gen1.wait_destroy_entered();

    // Before the destroy returns: deterministically rejected, the caller
    // keeps ownership of its provider.
    let mut gen2 = Probe::immediate();
    let gen2_provider = Probe::provider(&mut gen2);
    assert_eq!(
        runtime::register_catalog_provider(Some(&gen2_provider)),
        1,
        "registration while the old destroy runs must be rejected"
    );

    gen1.release();
    stopping.join().unwrap();

    // After the destroy returned: the barrier is open.
    assert_eq!(
        runtime::register_catalog_provider(Some(&gen2_provider)),
        0,
        "registration after the old destroy returned must be accepted"
    );
    assert_eq!(query_int_with_deadline(), 1, "the new generation answers");
    runtime::shutdown();
    assert_eq!(gen2.destroys.load(Ordering::SeqCst), 1);
    assert_eq!(
        gen1.destroys.load(Ordering::SeqCst),
        1,
        "the old generation was destroyed exactly once"
    );
}

/// Checklist: the same interleaving on the LAST IN-FLIGHT QUERY path — the
/// query epilogue performs the destroy, and registration is rejected while
/// that destroy runs, accepted only after it returned.
#[test]
fn query_epilogue_destroy_barrier_rejects_register_until_the_callback_returns() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let mut gen1 = DrainGateProbe::boxed();
    assert_eq!(runtime::register_catalog_provider(Some(&DrainGateProbe::provider(&mut gen1))), 0);

    // An in-flight query parks; shutdown only drains.
    let querying = std::thread::spawn(|| {
        let mut out = 0;
        runtime::catalog_query_int(HostStr::new(c"class"), HostStr::new(c"method"), HostStr::new(c"()I"), Some(&mut out))
    });
    gen1.query_entered.wait();
    runtime::shutdown();
    assert_eq!(
        gen1.destroys.load(Ordering::SeqCst),
        0,
        "shutdown with an in-flight query must defer the destroy"
    );

    // The query returns: its epilogue performs the destroy, which parks.
    gen1.query_resume.wait();
    gen1.wait_destroy_entered();

    let mut gen2 = Probe::immediate();
    let gen2_provider = Probe::provider(&mut gen2);
    assert_eq!(
        runtime::register_catalog_provider(Some(&gen2_provider)),
        1,
        "registration while the epilogue destroy runs must be rejected"
    );

    gen1.release();
    // The epilogue reopens the slot before the query thread returns, so the
    // join below makes "barrier open" deterministic.
    assert_eq!(querying.join().unwrap(), 1);

    assert_eq!(
        runtime::register_catalog_provider(Some(&gen2_provider)),
        0,
        "registration after the epilogue destroy returned must be accepted"
    );
    assert_eq!(query_int_with_deadline(), 1);
    runtime::shutdown();
    assert_eq!(gen2.destroys.load(Ordering::SeqCst), 1);
    assert_eq!(
        gen1.destroys.load(Ordering::SeqCst),
        1,
        "destroyed exactly once"
    );
}

/// Records what registry calls observe from INSIDE a destroy callback.
struct ReentryProbe {
    observed_query_hit: AtomicI32,
    observed_register_rc: AtomicI32,
    observed_shutdown_returned: AtomicBool,
    /// A candidate provider the callback tries to register while Destroying.
    candidate: Mutex<Option<CatalogProviderV0>>,
    destroys: AtomicUsize,
}

extern "C" fn reentry_destroy(user: *mut c_void) {
    // SAFETY: the owning test keeps the probe box alive past the destroy.
    let probe = unsafe { &*(user as *const ReentryProbe) };
    probe.destroys.fetch_add(1, Ordering::SeqCst);

    // (1) A query from inside the destroy observes the Destroying slot and
    // misses without deadlocking (the registry lock is not held here).
    let mut out = 0;
    let hit = runtime::catalog_query_int(
        HostStr::new(c"class"),
        HostStr::new(c"method"),
        HostStr::new(c"()I"),
        Some(&mut out),
    );
    probe.observed_query_hit.store(hit, Ordering::SeqCst);

    // (2) A registration from inside the destroy follows the same policy:
    // rejected, the caller keeps ownership.
    let candidate = probe.candidate.lock().unwrap().expect("candidate provider");
    probe
        .observed_register_rc
        .store(runtime::register_catalog_provider(Some(&candidate)), Ordering::SeqCst);

    // (3) shutdown from inside the destroy must not wait for itself and must
    // not destroy a second time.
    runtime::shutdown();
    probe.observed_shutdown_returned.store(true, Ordering::SeqCst);
}

/// Checklist: destroy-callback re-entry — query misses, register is
/// rejected, shutdown returns without waiting for itself or destroying
/// twice; afterwards a fresh generation registers and runs cleanly.
#[test]
fn destroy_callback_reentrant_registry_calls_follow_the_same_policy() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let probe = Box::new(ReentryProbe {
        observed_query_hit: AtomicI32::new(-1),
        observed_register_rc: AtomicI32::new(-1),
        observed_shutdown_returned: AtomicBool::new(false),
        candidate: Mutex::new(None),
        destroys: AtomicUsize::new(0),
    });
    let mut candidate_user = 0u8;
    *probe.candidate.lock().unwrap() = Some(CatalogProviderV0 {
        user: (&mut candidate_user as *mut u8).cast(),
        query_int: Some(overlap_query),
        query_bool: None,
        destroy: None,
    });
    let provider = CatalogProviderV0 {
        user: (&*probe as *const ReentryProbe).cast_mut().cast(),
        query_int: Some(overlap_query),
        query_bool: None,
        destroy: Some(reentry_destroy),
    };
    assert_eq!(runtime::register_catalog_provider(Some(&provider)), 0);

    // The destroy (with all its re-entrant registry calls) runs on this
    // shutdown thread; the deadline channel turns a deadlock into a test
    // failure instead of a hung run.
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        runtime::shutdown();
        done_tx.send(()).expect("receiver still waiting");
    });
    done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("shutdown must not wait for its own destroy callback");

    assert_eq!(
        probe.observed_query_hit.load(Ordering::SeqCst),
        0,
        "a query from inside the destroy must miss while the slot is Destroying"
    );
    assert_eq!(
        probe.observed_register_rc.load(Ordering::SeqCst),
        1,
        "a registration from inside the destroy must be rejected"
    );
    assert!(probe.observed_shutdown_returned.load(Ordering::SeqCst));
    assert_eq!(
        probe.destroys.load(Ordering::SeqCst),
        1,
        "the re-entrant shutdown must not destroy a second time"
    );

    // The candidate was rejected, so nothing was installed behind the
    // barrier; after it opened, a fresh generation registers and runs.
    let mut gen2 = Probe::immediate();
    assert_eq!(runtime::register_catalog_provider(Some(&Probe::provider(&mut gen2))), 0);
    assert_eq!(query_int_with_deadline(), 1);
    runtime::shutdown();
    assert_eq!(gen2.destroys.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.destroys.load(Ordering::SeqCst),
        1,
        "destroyed exactly once"
    );
}

/// Checklist: generation chain A/B/C — while an old generation's destroy has
/// not returned, the next generation is rejected; once it returned, each new
/// generation answers queries and drains under its own lifecycle, and no
/// delayed old-generation teardown ever clears or destroys a newer one.
#[test]
fn generation_chain_old_teardown_never_touches_newer_generations() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    // Generation A: query in flight, shutdown drains, the epilogue destroy
    // parks on the gate — B is rejected meanwhile.
    let mut gen_a = DrainGateProbe::boxed();
    assert_eq!(runtime::register_catalog_provider(Some(&DrainGateProbe::provider(&mut gen_a))), 0);
    let querying_a = std::thread::spawn(|| {
        let mut out = 0;
        runtime::catalog_query_int(HostStr::new(c"class"), HostStr::new(c"method"), HostStr::new(c"()I"), Some(&mut out))
    });
    gen_a.query_entered.wait();
    runtime::shutdown();
    gen_a.query_resume.wait();
    gen_a.wait_destroy_entered();

    let mut gen_b = Probe::blocking();
    let gen_b_provider = Probe::provider(&mut gen_b);
    assert_eq!(
        runtime::register_catalog_provider(Some(&gen_b_provider)),
        1,
        "generation B is rejected while A's destroy has not returned"
    );

    gen_a.release();
    // A's epilogue reopens the slot before its query thread returns.
    assert_eq!(querying_a.join().unwrap(), 1);

    // Generation B: registers only after A fully drained; its lifecycle is
    // fresh and A's teardown never interferes.
    assert_eq!(runtime::register_catalog_provider(Some(&gen_b_provider)), 0);
    // B answers one full blocking query round: main takes both barrier slots
    // so the query completes instead of leaking a lease.
    let answering_b = std::thread::spawn(|| {
        let mut out = 0;
        runtime::catalog_query_int(HostStr::new(c"class"), HostStr::new(c"method"), HostStr::new(c"()I"), Some(&mut out))
    });
    gen_b.entered.wait();
    gen_b.resume.wait();
    assert_eq!(answering_b.join().unwrap(), 1, "generation B answers");

    // B's own drain: an in-flight query defers the destroy, the epilogue
    // performs it, and only then does C get in.
    let mut gen_c = Probe::immediate();
    let gen_c_provider = Probe::provider(&mut gen_c);
    let querying_b = std::thread::spawn(|| {
        let mut out = 0;
        runtime::catalog_query_int(HostStr::new(c"class"), HostStr::new(c"method"), HostStr::new(c"()I"), Some(&mut out))
    });
    gen_b.entered.wait();
    runtime::shutdown();
    assert_eq!(
        gen_b.destroys.load(Ordering::SeqCst),
        0,
        "B's destroy is deferred while its query is in flight"
    );
    gen_b.resume.wait();
    assert_eq!(querying_b.join().unwrap(), 1);

    assert_eq!(
        runtime::register_catalog_provider(Some(&gen_c_provider)),
        0,
        "generation C registers after B drained"
    );
    assert_eq!(query_int_with_deadline(), 1, "generation C answers");
    runtime::shutdown();
    runtime::shutdown(); // repeated shutdowns are no-ops

    assert_eq!(gen_a.destroys.load(Ordering::SeqCst), 1, "A destroyed exactly once");
    assert_eq!(gen_b.destroys.load(Ordering::SeqCst), 1, "B destroyed exactly once");
    assert_eq!(gen_c.destroys.load(Ordering::SeqCst), 1, "C destroyed exactly once");
    assert_eq!(query_int_with_deadline(), 0, "post-shutdown queries miss");
}
