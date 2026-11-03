//! INPUT:  private items of `src/frame_timing.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/frame_timing.rs`; declared there with `#[path]`
use super::*;

#[test]
fn nearest_rank_boundaries_overflow_and_empty_are_explicit() {
    let mut h = Histogram::new();
    assert_eq!(h.percentile_upper(99), None);
    assert!(h.report("empty").contains("p99_upper_us=unavailable"));
    for n in 0..100 {
        h.record(n * 250);
    }
    assert_eq!(h.percentile_upper(50), Some(12_499));
    assert_eq!(h.percentile_upper(95), Some(23_749));
    assert_eq!(h.percentile_upper(99), Some(24_749));
    let mut overflow = Histogram::new();
    overflow.record(127_999);
    overflow.record(128_000);
    overflow.record(2_000_000);
    assert_eq!(overflow.percentile_upper(50), None);
    assert!(overflow
        .report("overflow")
        .contains("p50_upper_us=>=128000"));
    assert_eq!(overflow.max_us, 2_000_000);
    assert_eq!(overflow.over_50ms, 3);
}

#[test]
fn gameplay_window_excludes_startup_and_has_half_open_bounds() {
    let mut timing = FrameWorkTiming::default();
    for (start, work) in [
        (0, 2_000_000),
        (44_999, 100_000),
        (45_000, 10_000),
        (89_999, 20_000),
        (90_000, 100_000),
    ] {
        timing.record(start, work);
    }
    assert_eq!(timing.all.samples, 5);
    assert_eq!(timing.gameplay.samples, 2);
    assert_eq!(timing.gameplay.sum_us, 30_000);
    assert_eq!(timing.gameplay.percentile_upper(50), Some(10_249));
    assert_eq!(timing.gameplay.max_us, 20_000);
    // This exact fixed bound includes both histograms and all counters.
    assert!(std::mem::size_of::<FrameWorkTiming>() < 9 * 1024);
}
