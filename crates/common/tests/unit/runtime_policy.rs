//! INPUT:  private items of `src/runtime_policy.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/runtime_policy.rs`; declared there with `#[path]`
use super::*;

#[test]
fn ninety_fps_preserves_unused_time_and_skips_delay_for_slow_frames() {
    assert_eq!(
        remaining_frame_time(DEFAULT_TARGET_FPS, Duration::from_millis(5)),
        Duration::from_nanos(6_111_112)
    );
    assert_eq!(
        remaining_frame_time(DEFAULT_TARGET_FPS, Duration::from_millis(12)),
        Duration::ZERO
    );
    assert_eq!(resolve_target_fps(90), 90);
    assert_eq!(resolve_target_fps(60), 60);
    assert_eq!(resolve_target_fps(0), 0);
    assert_eq!(resolve_target_fps(240), 240);
    assert_eq!(resolve_target_fps(-1), DEFAULT_TARGET_FPS);
    assert_eq!(resolve_target_fps(241), DEFAULT_TARGET_FPS);
}

#[test]
fn sixty_fps_waits_only_for_unused_time() {
    assert_eq!(
        remaining_frame_time(60, Duration::from_millis(5)),
        Duration::from_nanos(11_666_667)
    );
    assert_eq!(
        remaining_frame_time(60, Duration::from_millis(20)),
        Duration::ZERO
    );
    assert_eq!(
        remaining_frame_time(60, Duration::from_millis(33)),
        Duration::ZERO
    );
    assert_eq!(remaining_frame_time(0, Duration::ZERO), Duration::ZERO);
}
