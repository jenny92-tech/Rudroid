//! INPUT:  private items of `src/policy.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/policy.rs`; declared there with `#[path]`
use super::*;

fn policy(notify: bool, unload: bool, grace_ms: u64) -> ReclaimPolicy {
    ReclaimPolicy::new(
        PressureGate::new(64, 128, Duration::from_secs(30), Duration::from_secs(5)),
        128,
        Duration::from_millis(grace_ms),
        notify,
        unload,
    )
}
fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

#[test]
fn developer_recovery_avoids_unload_and_repeated_notification() {
    let mut policy = policy(true, true, 1000);
    assert_eq!(policy.next(256, at(10)), Action::Wait);
    assert_eq!(policy.next(32, at(1)), Action::Wait);
    assert_eq!(policy.next(32, at(5)), Action::Notify);
    assert_eq!(policy.next(32, at(5)), Action::Wait);
    policy.notified(at(5), true);
    assert!(policy.awaiting_developer());
    assert_eq!(policy.next(32, at(5)), Action::Wait);
    // A game's callback releases its cache during subsequent frames.
    assert_eq!(policy.next(128, at(6)), Action::Recovered);
    assert!(!policy.awaiting_developer());
    assert_eq!(policy.next(32, at(35)), Action::Wait);
    assert_eq!(policy.next(32, at(36)), Action::Notify);
}

#[test]
fn persistent_pressure_falls_back_once_then_cools_down() {
    let mut policy = policy(true, true, 1000);
    assert_eq!(policy.next(32, at(5)), Action::Notify);
    policy.notified(at(5), true);
    assert_eq!(policy.next(80, at(5)), Action::Wait);
    assert_eq!(policy.next(80, at(6)), Action::Unload);
    assert_eq!(policy.next(0, at(100)), Action::Wait);
    policy.completed(at(101));
    assert_eq!(policy.next(32, at(130)), Action::Wait);
    assert_eq!(policy.next(32, at(131)), Action::Notify);
}

#[test]
fn notification_only_and_missing_handler_do_not_force_gc() {
    let mut notify_only = policy(true, false, 1000);
    assert_eq!(notify_only.next(32, at(5)), Action::Notify);
    notify_only.notified(at(5), true);
    assert_eq!(notify_only.next(32, at(6)), Action::NotificationOnly);
    assert_eq!(notify_only.next(32, at(7)), Action::Wait);
    let mut no_notify = policy(false, true, 1000);
    assert_eq!(no_notify.next(32, at(5)), Action::Unload);
}

#[test]
fn failed_notifications_zero_grace_and_exhausted_headroom_do_not_wait() {
    for (grace, success, remaining) in [(1000, false, 32), (0, true, 32), (1000, true, 0)] {
        let mut policy = policy(true, true, grace);
        assert_eq!(policy.next(32, at(5)), Action::Notify);
        policy.notified(at(5), success);
        assert_eq!(policy.next(remaining, at(5)), Action::Unload);
    }
}
