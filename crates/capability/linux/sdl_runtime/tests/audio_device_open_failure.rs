//! Ticket 92: the device-open failure ladder. Its own test binary because
//! SDL chooses its driver from the environment at first subsystem init —
//! this process exists to attempt a nonexistent driver and assert the
//! structured failure after exactly `open_attempts` tries.

#![cfg(feature = "sdl")]

use platform_sdl_runtime::real_audio::{AudioBridge, RealAudioError};

#[test]
fn open_failure_ladder_is_structured() {
    std::env::set_var("SDL_AUDIODRIVER", "rudroid92-nonexistent-driver");
    let error = match AudioBridge::open(48_000, 2, 512, 3, 10) {
        Ok(_) => panic!("a nonexistent driver cannot open"),
        Err(error) => error,
    };
    let RealAudioError::OpenFailed {
        attempts,
        last_error,
    } = error
    else {
        panic!("expected OpenFailed");
    };
    assert_eq!(attempts, 3, "the ladder ran exactly open_attempts times");
    assert!(!last_error.is_empty(), "the SDL diagnostic is carried");
}
