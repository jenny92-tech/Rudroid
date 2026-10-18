//! Ticket 70 acceptance: the catalog provider threading contract.
//!
//! Decision A (documented on `rudroid_common::registry::CatalogProviderV0`):
//! the ABI requires providers to tolerate CONCURRENT queries on the same
//! `user` — the host leases the provider under the registry lock but invokes
//! the callback outside every lock — and the host guarantees `destroy` runs
//! only after every in-flight query callback has returned (happens-before via
//! the locked lease handoff, tickets 61/66).
//!
//! - `concurrent_queries_overlap_inside_the_provider_callback` is the
//!   deterministic semantic probe: two threads must be inside the same
//!   provider callback AT THE SAME TIME. A host that serialized per-provider
//!   calls (the rejected decision B) fails its rendezvous by timeout instead
//!   of deadlocking.
//! - Concurrent queries racing a `shutdown` destroy exactly once, never
//!   overlap the destroy with an active callback, and miss cleanly after.
//! - Query re-entry (nested query, shutdown from inside callbacks) stays
//!   deadlock-free when callbacks from different threads overlap.
//! - The built-in reply_catalog participates as the REAL cdylib: its
//!   provider registers through `rudroid_plugin_early_init`, serves
//!   concurrent queries through the host registry, and its real destructor
//!   (which frees the `Box<Catalog>`) runs exactly once under the same
//!   shutdown race — not only the never-freeing probes.
//!
//! The registry is process-global state, so every test in this binary
//! serializes on one mutex. Test providers only observe timing (no real
//! free), so an assertion failure cannot turn into a use-after-free.

use std::ffi::{c_int, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rudroid_common::registry::{CatalogProviderV0, HostStr};
use rudroid_unity_loader::{host_api, runtime};

/// Serializes every test in this binary: the registry is process-global.
static SERIAL: Mutex<()> = Mutex::new(());

/// Upper bound for every wait, so a future deadlock regresses as a test
/// failure instead of a hung run.
const DEADLINE: Duration = Duration::from_secs(30);

/// Two-party rendezvous used INSIDE provider callbacks: returns `true` when
/// both query threads were inside the callback at the same time. A plain
/// `Barrier` would hang forever under the rejected serialization semantics
/// (and leak parked leases into later tests); the timeout instead turns that
/// regression into a `false` return the test can assert on, and timed-out
/// threads leave cleanly so no lease stays parked after a failed run. The
/// first successful meet wins (`met` latches): later single callers can never
/// fake an overlap against a spent rendezvous.
struct Meetup {
    /// (arrived, met)
    state: Mutex<(usize, bool)>,
    all_here: Condvar,
}

impl Meetup {
    fn new() -> Self {
        Self {
            state: Mutex::new((0, false)),
            all_here: Condvar::new(),
        }
    }

    fn meet(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.1 {
            return true;
        }
        state.0 += 1;
        if state.0 >= 2 {
            state.1 = true;
            self.all_here.notify_all();
            return true;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let now = Instant::now();
            if now >= deadline {
                state.0 -= 1; // leave cleanly; the test asserts below
                return false;
            }
            let (next, timed_out) = self
                .all_here
                .wait_timeout(state, deadline - now)
                .unwrap();
            state = next;
            if state.1 {
                return true;
            }
            if timed_out.timed_out() {
                state.0 -= 1;
                return false;
            }
        }
    }
}

/// Query callback signature of `CatalogProviderV0::query_int`.
type QueryFn = extern "C" fn(
    user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int;

/// Runs `body` on a worker thread and applies a result deadline, so a
/// deadlock in the code under test surfaces as a test failure instead of a
/// hung run.
fn with_deadline<R: Send + 'static>(body: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(body()).expect("receiver still waiting");
    });
    rx.recv_timeout(DEADLINE)
        .expect("catalog query must not deadlock")
}

fn open_slot_query_hit() -> c_int {
    let mut out = 0;
    runtime::catalog_query_int(
        HostStr::new(c"class"),
        HostStr::new(c"method"),
        HostStr::new(c"()I"),
        Some(&mut out),
    )
}

/// Two `catalog_query_int` calls on SIMULTANEOUS worker threads (each with
/// the result deadline applied). Sequential helper calls would never overlap
/// and defeat the rendezvous probes.
fn two_concurrent_query_hits() -> (c_int, c_int) {
    let (tx, rx) = mpsc::channel();
    for _ in 0..2 {
        let tx = tx.clone();
        std::thread::spawn(move || {
            tx.send(open_slot_query_hit()).expect("receiver still waiting");
        });
    }
    drop(tx);
    let a = rx
        .recv_timeout(DEADLINE)
        .expect("catalog query must not deadlock");
    let b = rx
        .recv_timeout(DEADLINE)
        .expect("catalog query must not deadlock");
    (a, b)
}

// ------------------------------------------------------------------
// Probe 1: the overlap rendezvous (the decision-A semantic proof).
// ------------------------------------------------------------------

struct MeetProbe {
    meetup: Meetup,
    overlapped: AtomicBool,
}

extern "C" fn meet_query(
    user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    // SAFETY: the owning test keeps the probe box alive until both query
    // threads have joined; the provider has no destroy and shutdown only
    // drops the registry's copy afterwards.
    let probe = unsafe { &*(user as *const MeetProbe) };
    if probe.meetup.meet() {
        probe.overlapped.store(true, Ordering::SeqCst);
    }
    1
}

#[test]
fn concurrent_queries_overlap_inside_the_provider_callback() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let probe = Box::new(MeetProbe {
        meetup: Meetup::new(),
        overlapped: AtomicBool::new(false),
    });
    let provider = CatalogProviderV0 {
        user: (&*probe as *const MeetProbe).cast_mut().cast(),
        query_int: Some(meet_query),
        query_bool: None,
        destroy: None,
    };
    assert_eq!(runtime::register_catalog_provider(Some(&provider)), 0);

    // Both threads must get past the rendezvous INSIDE the callback: that is
    // only possible if the host runs provider queries concurrently outside
    // the registry lock (decision A). The deadline turns a serializing host
    // into an assertion failure instead of a hang.
    let (a, b) = two_concurrent_query_hits();
    assert_eq!(a, 1);
    assert_eq!(b, 1);
    assert!(
        probe.overlapped.load(Ordering::SeqCst),
        "two queries must be inside the provider callback at the same time; \
         a host that serializes per-provider calls times the rendezvous out"
    );

    runtime::shutdown();
}

// ------------------------------------------------------------------
// Probes 2-4: shared counting probe (destroy never overlaps an active
// callback; exactly-once destroy; re-entrancy under concurrency).
// ------------------------------------------------------------------

struct CountingProbe {
    /// Number of query callbacks currently inside the provider.
    active: AtomicUsize,
    /// Completed nested re-entrant queries.
    nested_queries: AtomicUsize,
    /// Claim flag so nested re-entry cannot recurse unboundedly.
    nested_claimed: AtomicBool,
    /// Hit/miss the nested query observed (-1 until it ran).
    nested_hit: AtomicI32,
    meetup: Meetup,
    rendezvoused: AtomicBool,
    destroys: AtomicUsize,
    /// What `destroy` observed: the documented happens-before requires 0.
    destroy_saw_active: AtomicUsize,
}

impl CountingProbe {
    fn boxed() -> Box<Self> {
        Box::new(Self {
            active: AtomicUsize::new(0),
            nested_queries: AtomicUsize::new(0),
            nested_claimed: AtomicBool::new(false),
            nested_hit: AtomicI32::new(-1),
            meetup: Meetup::new(),
            rendezvoused: AtomicBool::new(false),
            destroys: AtomicUsize::new(0),
            destroy_saw_active: AtomicUsize::new(0),
        })
    }

    fn provider(this: &mut Box<Self>, query: QueryFn) -> CatalogProviderV0 {
        CatalogProviderV0 {
            user: (&mut **this as *mut CountingProbe).cast(),
            query_int: Some(query),
            query_bool: None,
            destroy: Some(counting_destroy),
        }
    }
}

extern "C" fn counting_destroy(user: *mut c_void) {
    // SAFETY: the owning test keeps the probe box alive past the destroy.
    let probe = unsafe { &*(user as *const CountingProbe) };
    probe
        .destroy_saw_active
        .store(probe.active.load(Ordering::SeqCst), Ordering::SeqCst);
    probe.destroys.fetch_add(1, Ordering::SeqCst);
}

// SAFETY (shared by plain_query/nesting_query/shutdown_query): the owning
// test keeps the probe box alive until every query thread joined and the
// final shutdown destroyed the provider; the callbacks only touch the
// probe's atomics and registry entry points.
extern "C" fn plain_query(
    user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    let probe = unsafe { &*(user as *const CountingProbe) };
    probe.active.fetch_add(1, Ordering::SeqCst);
    // Give concurrent callbacks a real time window to overlap in.
    std::thread::sleep(Duration::from_millis(1));
    probe.active.fetch_sub(1, Ordering::SeqCst);
    1
}

#[test]
fn shutdown_racing_concurrent_queries_destroys_once_after_the_last_callback() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let mut probe = CountingProbe::boxed();
    assert_eq!(
        runtime::register_catalog_provider(Some(&CountingProbe::provider(&mut probe, plain_query))),
        0
    );

    const THREADS: usize = 4;
    const QUERIES: usize = 64;
    let (tx, rx) = mpsc::channel();
    for _ in 0..THREADS {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for _ in 0..QUERIES {
                let mut out = 0;
                let hit = runtime::catalog_query_int(
                    HostStr::new(c"class"),
                    HostStr::new(c"method"),
                    HostStr::new(c"()I"),
                    Some(&mut out),
                );
                tx.send(hit).expect("receiver still waiting");
            }
        });
    }
    drop(tx);

    // Deterministic overlap: shutdown only runs once at least two callbacks
    // are active inside the provider.
    let started = Instant::now();
    while probe.active.load(Ordering::SeqCst) < 2 {
        assert!(
            started.elapsed() < DEADLINE,
            "query threads never entered the provider callback"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime::shutdown();

    let mut hits = 0;
    let mut misses = 0;
    loop {
        match rx.recv_timeout(DEADLINE) {
            Ok(hit) => {
                assert!(
                    hit == 0 || hit == 1,
                    "every racing query must answer 1 (hit) or 0 (closed slot), got {hit}"
                );
                if hit == 1 {
                    hits += 1;
                } else {
                    misses += 1;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("concurrent queries must not deadlock against shutdown")
            }
        }
    }
    assert_eq!(hits + misses, THREADS * QUERIES, "every query returned");
    assert!(hits >= 1, "the in-flight queries answered before the drain");
    assert!(misses >= 1, "queries after shutdown must miss cleanly");
    assert_eq!(
        probe.destroys.load(Ordering::SeqCst),
        1,
        "destroyed exactly once under the race"
    );
    assert_eq!(
        probe.destroy_saw_active.load(Ordering::SeqCst),
        0,
        "destroy must run only after every query callback returned \
         (the documented happens-before)"
    );

    // Post-shutdown misses stay clean and repeated shutdowns are no-ops (a
    // second destroy of a real provider would be a double free).
    assert_eq!(with_deadline(open_slot_query_hit), 0);
    runtime::shutdown();
    runtime::shutdown();
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1);
}

/// Rendezvous the two outer callbacks inside the provider, then perform one
/// re-entrant query from inside the callback while the other thread's
/// callback is active at the same time.
extern "C" fn nesting_query(
    user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    let probe = unsafe { &*(user as *const CountingProbe) };
    probe.active.fetch_add(1, Ordering::SeqCst);
    if probe.meetup.meet() {
        probe.rendezvoused.store(true, Ordering::SeqCst);
        if !probe.nested_claimed.swap(true, Ordering::SeqCst) {
            // Re-entrant query from inside the callback: must not deadlock
            // (the registry lock is never held across callbacks and the
            // in-flight count is per-entry, not per-thread).
            let mut nested_out = 0;
            let hit = runtime::catalog_query_int(class, method, sig, Some(&mut nested_out));
            probe.nested_queries.fetch_add(1, Ordering::SeqCst);
            probe.nested_hit.store(hit, Ordering::SeqCst);
        }
    }
    probe.active.fetch_sub(1, Ordering::SeqCst);
    1
}

#[test]
fn reentrant_query_from_overlapping_callbacks_does_not_deadlock() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let mut probe = CountingProbe::boxed();
    let provider = CatalogProviderV0 {
        query_int: Some(nesting_query),
        ..CountingProbe::provider(&mut probe, plain_query)
    };
    assert_eq!(runtime::register_catalog_provider(Some(&provider)), 0);

    let (a, b) = two_concurrent_query_hits();
    assert_eq!(a, 1);
    assert_eq!(b, 1);
    assert!(
        probe.rendezvoused.load(Ordering::SeqCst),
        "the two outer callbacks must have been active at the same time"
    );
    assert_eq!(
        probe.nested_queries.load(Ordering::SeqCst),
        1,
        "exactly one re-entrant query per registration (claim guard)"
    );
    assert_eq!(
        probe.nested_hit.load(Ordering::SeqCst),
        1,
        "the nested query must hit the still-open slot"
    );

    runtime::shutdown();
    assert_eq!(
        probe.destroys.load(Ordering::SeqCst),
        1,
        "destroyed exactly once"
    );
    assert_eq!(
        probe.destroy_saw_active.load(Ordering::SeqCst),
        0,
        "the destroy must not overlap an active callback"
    );
}

/// Both callbacks trigger `shutdown` while both are active inside the
/// provider: shutdown must not wait for its own callers, the destroy must run
/// exactly once and only after the last callback returned.
extern "C" fn shutdown_query(
    user: *mut c_void,
    _class: HostStr<'_>,
    _method: HostStr<'_>,
    _sig: HostStr<'_>,
    _out: Option<&mut i64>,
) -> c_int {
    let probe = unsafe { &*(user as *const CountingProbe) };
    probe.active.fetch_add(1, Ordering::SeqCst);
    if probe.meetup.meet() {
        probe.rendezvoused.store(true, Ordering::SeqCst);
        runtime::shutdown();
    }
    probe.active.fetch_sub(1, Ordering::SeqCst);
    1
}

#[test]
fn shutdown_from_overlapping_callbacks_does_not_deadlock_or_double_destroy() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();

    let mut probe = CountingProbe::boxed();
    let provider = CatalogProviderV0 {
        query_int: Some(shutdown_query),
        ..CountingProbe::provider(&mut probe, plain_query)
    };
    assert_eq!(runtime::register_catalog_provider(Some(&provider)), 0);

    let (a, b) = two_concurrent_query_hits();
    assert_eq!(a, 1);
    assert_eq!(b, 1);
    assert!(
        probe.rendezvoused.load(Ordering::SeqCst),
        "both callbacks must have been active at the same time"
    );
    assert_eq!(
        probe.destroys.load(Ordering::SeqCst),
        1,
        "destroyed exactly once despite two concurrent shutdown calls"
    );
    assert_eq!(
        probe.destroy_saw_active.load(Ordering::SeqCst),
        0,
        "the destroy must not overlap an active callback"
    );

    runtime::shutdown();
    runtime::shutdown();
    assert_eq!(probe.destroys.load(Ordering::SeqCst), 1);
}

// ------------------------------------------------------------------
// The REAL reply_catalog cdylib under the documented contract.
// ------------------------------------------------------------------

/// Rebuild the cdylib from current sources (never a stale artifact).
fn ensure_cdylib(package: &str, artifact: &str) -> PathBuf {
    rudroid_test_support::cargo::ensure_workspace_cdylib(package, artifact)
}

/// Load the real cdylib and register its provider through `early_init`.
/// The caller keeps the returned handle alive for the whole registry
/// lifetime of the test (the provider's function pointers live in it).
fn dlopen_real_catalog_early_init() -> libloading::Library {
    let catalog = ensure_cdylib(
        "rudroid-android-reply-catalog",
        "rudroid_android_reply_catalog",
    );
    // SAFETY: dlopen of a workspace-built cdylib.
    let lib = unsafe { libloading::Library::new(&catalog) }.expect("dlopen catalog cdylib");
    let early_init: libloading::Symbol<
        unsafe extern "C" fn(*const rudroid_common::host_api::HostApiV0) -> c_int,
    > = unsafe { lib.get(b"rudroid_plugin_early_init") }.expect("early_init export");
    assert_eq!(
        unsafe { early_init(host_api::table()) },
        0,
        "the real catalog cdylib must register its provider through early_init"
    );
    lib
}

#[test]
fn builtin_reply_catalog_cdylib_answers_concurrent_queries() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let _lib = dlopen_real_catalog_early_init();

    const THREADS: usize = 4;
    const QUERIES: usize = 250;
    let (tx, rx) = mpsc::channel();
    for _ in 0..THREADS {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for _ in 0..QUERIES {
                let mut out = 0;
                let hit = runtime::catalog_query_int(
                    HostStr::new(c"android/media/AudioManager"),
                    HostStr::new(c"getStreamVolume"),
                    HostStr::new(c"(I)I"),
                    Some(&mut out),
                );
                tx.send((hit, out)).expect("receiver still waiting");
            }
        });
    }
    drop(tx);

    let mut received = 0;
    loop {
        match rx.recv_timeout(DEADLINE) {
            Ok((hit, value)) => {
                assert!(
                    hit == 0 || hit == 1,
                    "real provider answered {hit}, expected 0 or 1"
                );
                if hit == 1 {
                    assert_eq!(
                        value, 100,
                        "the real provider must return the configured fixed reply \
                         under concurrent queries"
                    );
                }
                received += 1;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("concurrent queries against the real cdylib must not deadlock")
            }
        }
    }
    assert_eq!(received, THREADS * QUERIES, "every query returned");

    // The REAL destructor (frees the Box<Catalog>) runs through shutdown;
    // afterwards queries miss and repeated shutdowns must not double-free.
    runtime::shutdown();
    assert_eq!(
        with_deadline(|| {
            let mut out = 0;
            runtime::catalog_query_int(
                HostStr::new(c"android/media/AudioManager"),
                HostStr::new(c"getStreamVolume"),
                HostStr::new(c"(I)I"),
                Some(&mut out),
            )
        }),
        0,
        "post-shutdown queries must miss instead of calling the destroyed provider"
    );
    runtime::shutdown();
    runtime::shutdown();
}

#[test]
fn builtin_reply_catalog_real_destroy_survives_shutdown_racing_queries() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    runtime::shutdown();
    let _lib = dlopen_real_catalog_early_init();

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let tx = tx.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let mut out = 0;
                    let hit = runtime::catalog_query_int(
                        HostStr::new(c"android/media/AudioManager"),
                        HostStr::new(c"getStreamVolume"),
                        HostStr::new(c"(I)I"),
                        Some(&mut out),
                    );
                    tx.send((hit, out)).expect("receiver still waiting");
                    if hit == 0 {
                        // Closed slot: stop hammering while the test winds down.
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            })
        })
        .collect();
    drop(tx);

    // Deterministic ordering: at least one full query completed against the
    // open slot before the shutdown races the still-running ones.
    let (first_hit, first_value) = rx
        .recv_timeout(DEADLINE)
        .expect("the real provider must answer");
    assert_eq!(first_hit, 1, "the first query must hit the open slot");
    assert_eq!(first_value, 100);

    // The REAL destructor (Box<Catalog> free) runs while other queries may
    // still be in flight or arriving.
    runtime::shutdown();
    stop.store(true, Ordering::SeqCst);
    for handle in handles {
        handle.join().unwrap();
    }
    let mut hits = 1;
    loop {
        match rx.recv_timeout(DEADLINE) {
            Ok((hit, value)) => {
                assert!(
                    hit == 0 || hit == 1,
                    "racing query answered {hit}, expected 0 or 1"
                );
                if hit == 1 {
                    assert_eq!(value, 100, "a hit must carry the configured reply");
                    hits += 1;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("racing queries must not deadlock against the real destroy")
            }
        }
    }
    assert!(hits >= 1, "queries answered before the drain");

    // The real destructor already ran exactly once; anything else here would
    // be a double free of the Box<Catalog>.
    assert_eq!(
        with_deadline(|| {
            let mut out = 0;
            runtime::catalog_query_int(
                HostStr::new(c"android/media/AudioManager"),
                HostStr::new(c"getStreamVolume"),
                HostStr::new(c"(I)I"),
                Some(&mut out),
            )
        }),
        0,
        "post-shutdown queries must miss"
    );
    runtime::shutdown();
    runtime::shutdown();
}
