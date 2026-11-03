//! INPUT:  pure title-unload gate from src/after_title.rs
//! OUTPUT:  title-delay, once-only and cancellation regressions
//! POS:    checks scene transitions without relying on managed pointer layout
use super::*;

#[test]
fn bootstrap_and_gameplay_frames_do_not_count_towards_title_delay() {
    let mut gate = Gate::default();
    for _ in 0..1000 {
        assert_eq!(gate.frame(false, 60), Step::Waiting);
    }
    for _ in 0..59 {
        assert_eq!(gate.frame(true, 60), Step::Counting);
    }
    assert_eq!(gate.frame(true, 60), Step::Unload);
}

#[test]
fn completed_attempt_never_rearms_on_later_title_visits() {
    let mut gate = Gate::default();
    assert_eq!(gate.frame(true, 1), Step::Unload);
    for title in [true, false, false, true, true] {
        assert_eq!(gate.frame(title, 1), Step::Waiting);
    }
}

#[test]
fn leaving_title_cancels_pending_attempt_without_rearming() {
    let mut gate = Gate::default();
    assert_eq!(gate.frame(true, 60), Step::Counting);
    assert_eq!(gate.frame(false, 60), Step::Cancelled);
    for _ in 0..100 {
        assert_eq!(gate.frame(true, 60), Step::Waiting);
    }
}
