//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static CALLS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn probe(_event: Option<&RudroidEvent>, _user: *mut c_void) {
    CALLS.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn reentrant_register(event: Option<&RudroidEvent>, user: *mut c_void) {
    CALLS.fetch_add(1, Ordering::SeqCst);
    let fake = unsafe { &*(user as *const SubscriptionFake) };
    // Reentrant subscription lands in the table but must NOT run in the
    // fire that is already in progress (registry: next fire on).
    fake.register(7u32, Some(probe), std::ptr::null_mut());
    assert!(event.is_some());
}

extern "C" fn reentrant_fire(event: Option<&RudroidEvent>, user: *mut c_void) {
    CALLS.fetch_add(1, Ordering::SeqCst);
    let fake = unsafe { &*(user as *const SubscriptionFake) };
    // Reentrant fire must not deadlock (callback runs outside the lock);
    // it targets a DIFFERENT kind so the snapshot cannot recurse into
    // itself.
    fake.fire(12u32, 0, "nested");
    assert!(event.is_some());
}

extern "C" fn nested_probe(_event: Option<&RudroidEvent>, _user: *mut c_void) {
    CALLS.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn same_kind_accepts_multiple_subscriptions_and_fires_all() {
    let _serial = serial_lock();
    let fake = SubscriptionFake::default();
    assert_eq!(fake.register(3, Some(probe), std::ptr::null_mut()), 0);
    // The old fake rejected this with 1; the host never does.
    assert_eq!(fake.register(3, Some(probe), std::ptr::null_mut()), 0);
    assert_eq!(fake.register(3, None, std::ptr::null_mut()), 2);
    assert_eq!(fake.slots_of(3).len(), 2);
    CALLS.store(0, Ordering::SeqCst);
    fake.fire(3, 0x1000, "m");
    assert_eq!(CALLS.load(Ordering::SeqCst), 2, "every subscription fires");
    assert_eq!(fake.slot(3).map(|(c, _)| c), Some(probe as usize));
}

#[test]
fn mid_fire_subscription_applies_only_to_the_next_fire() {
    let _serial = serial_lock();
    let fake: &'static SubscriptionFake = Box::leak(Box::new(SubscriptionFake::default()));
    assert_eq!(fake.register(7, Some(reentrant_register), fake as *const _ as *mut _), 0);
    CALLS.store(0, Ordering::SeqCst);
    fake.fire(7, 0, "first");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1, "mid-fire addition is not in this fire");
    fake.fire(7, 0, "second");
    assert_eq!(CALLS.load(Ordering::SeqCst), 3, "next fire runs it");
}

#[test]
fn reentrant_fire_does_not_deadlock() {
    let _serial = serial_lock();
    let fake: &'static SubscriptionFake = Box::leak(Box::new(SubscriptionFake::default()));
    assert_eq!(fake.register(11, Some(reentrant_fire), fake as *const _ as *mut _), 0);
    assert_eq!(fake.register(12, Some(nested_probe), std::ptr::null_mut()), 0);
    CALLS.store(0, Ordering::SeqCst);
    // With the lock held across the callback this deadlocked (review
    // 2026-09-19 Standards §2); now the callback runs outside the lock.
    fake.fire(11, 0, "outer");
    assert_eq!(
        CALLS.load(Ordering::SeqCst),
        2,
        "outer probe then the nested fire's probe"
    );
}
