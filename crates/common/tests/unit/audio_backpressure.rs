//! INPUT:  sample/callback geometry and remaining producer deadline
//! OUTPUT:  pacing bounds, extreme input and deadline regressions
//! POS:    validates backend-neutral wait arithmetic without an audio device
use super::*;

#[test]
fn delay_follows_the_callback_and_missing_frames() {
    let plan = AudioBackpressure::new(48_000, 1024).unwrap();
    assert_eq!(plan.delay(1024, Duration::from_secs(1)), Duration::from_millis(10));
    assert_eq!(plan.delay(96, Duration::from_secs(1)), Duration::from_millis(2));
    let small = AudioBackpressure::new(48_000, 256).unwrap();
    assert_eq!(small.delay(256, Duration::from_secs(1)), Duration::from_nanos(2_666_666));
}

#[test]
fn no_space_wait_exceeds_deadline_or_increases_the_old_poll_rate() {
    let plan = AudioBackpressure::new(384_000, 32).unwrap();
    assert_eq!(plan.delay(1, Duration::from_secs(1)), Duration::from_millis(1));
    assert_eq!(plan.delay(1, Duration::from_micros(75)), Duration::from_micros(75));
    assert_eq!(plan.delay(1, Duration::ZERO), Duration::ZERO);
    assert_eq!(plan.delay(0, Duration::from_secs(1)), Duration::ZERO);
}

#[test]
fn extreme_inputs_are_bounded_and_invalid_specs_are_rejected() {
    assert!(AudioBackpressure::new(0, 1).is_none());
    assert!(AudioBackpressure::new(1, 0).is_none());
    for (rate, block) in [(1, u32::MAX), (u32::MAX, u32::MAX)] {
        let plan = AudioBackpressure::new(rate, block).unwrap();
        let wait = plan.delay(usize::MAX, Duration::MAX);
        assert!(wait >= Duration::from_millis(1) && wait <= Duration::from_millis(10));
    }
}
