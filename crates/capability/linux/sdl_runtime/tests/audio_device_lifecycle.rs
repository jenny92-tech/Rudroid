//! Ticket 92 regressions: underrun/late-producer accounting, the bounded
//! queue, pause/resume, submit validation, and the close/reopen lifecycle —
//! all on the SDL `dummy` virtual device (a real device open with a real
//! callback loop; output discarded, which is exactly the controlled
//! producer-late environment these tests need). Audibility is NOT claimed.
//!
//! SDL initializes ONCE per process and picks its driver from the
//! environment at that moment, so every scenario in this file runs
//! sequentially on ONE thread inside a single #[test] (the disk-driver
//! closed loop and the open-failure ladder are separate processes/files).

#![cfg(feature = "sdl")]

use platform_sdl_runtime::real_audio::{AudioBridge, RealAudioError};
use std::sync::atomic::Ordering;

fn open_dummy(rate: i32, channels: u8, block: u32) -> AudioBridge {
    AudioBridge::open(rate, channels, block, 4, 50).expect("dummy virtual device opens")
}

/// A late producer is counted, never fatal: before the first submission the
/// callback runs on silence (underruns), and it recovers as soon as frames
/// arrive.
fn scenario_late_producer() {
    let mut bridge = open_dummy(48_000, 2, 1024);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while bridge.stats().callbacks.load(Ordering::Acquire) == 0 {
        assert!(std::time::Instant::now() < deadline, "callbacks never ran");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    let underruns = bridge.stats().underruns.load(Ordering::Acquire);
    assert!(underruns >= 1, "the starving callback must count underruns");
    let mut submitted = 0u64;
    let start = std::time::Instant::now();
    while submitted < 4800 && start.elapsed() < std::time::Duration::from_secs(5) {
        submitted += bridge.submit(&vec![0.25f32; 960]).expect("open bridge") as u64;
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while bridge.stats().pulled_frames.load(Ordering::Acquire) < 4800 {
        assert!(std::time::Instant::now() < deadline, "device never drained: pulled={} submitted={} underruns={} callbacks={} queued={}",
            bridge.stats().pulled_frames.load(Ordering::Acquire),
            bridge.stats().submitted_frames.load(Ordering::Acquire),
            bridge.stats().underruns.load(Ordering::Acquire),
            bridge.stats().callbacks.load(Ordering::Acquire),
            bridge.queued_frames());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    bridge.close().unwrap();
}

/// The queue is bounded: submitting more than capacity while the device is
/// paused drops the excess and counts it — the ring never grows past
/// capacity.
fn scenario_bounded_queue() {
    let mut bridge = open_dummy(48_000, 2, 512);
    bridge.pause().unwrap();
    let capacity = bridge.capacity_frames() as u64;
    let mut accepted_total = 0u64;
    let mut submitted_total = 0u64;
    for _ in 0..8 {
        let chunk = vec![0.5f32; 1024]; // 512 frames per submit
        let accepted = bridge.submit(&chunk).unwrap() as u64;
        accepted_total += accepted;
        submitted_total += 512;
    }
    assert_eq!(accepted_total, capacity, "exactly the capacity fits while paused");
    assert_eq!(
        submitted_total - accepted_total,
        bridge.stats().dropped_frames.load(Ordering::Acquire),
        "every dropped frame is counted"
    );
    assert!(
        bridge.stats().max_queue_frames.load(Ordering::Acquire) <= capacity,
        "the queue must never exceed its bound"
    );
    bridge.resume().unwrap();
    bridge.close().unwrap();
}

/// Pause stops the drain, resume restarts it; close ends the lifecycle
/// (submit/close after close are structured errors) and stats stay readable.
fn scenario_pause_resume_close() {
    let mut bridge = open_dummy(48_000, 2, 512);
    bridge.submit(&vec![0.25f32; 960]).unwrap(); // 960 samples = 480 frames @2ch
    bridge.pause().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));
    let pulled_paused = bridge.stats().pulled_frames.load(Ordering::Acquire);
    std::thread::sleep(std::time::Duration::from_millis(80));
    let pulled_still = bridge.stats().pulled_frames.load(Ordering::Acquire);
    assert_eq!(pulled_paused, pulled_still, "a paused device must not consume");
    bridge.resume().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while bridge.stats().pulled_frames.load(Ordering::Acquire) < 480 {
        assert!(std::time::Instant::now() < deadline, "resume never drained");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    bridge.close().unwrap();
    assert!(bridge.is_closed());
    let error = bridge.submit(&[0.0f32; 16]).expect_err("closed bridge");
    assert!(matches!(error, RealAudioError::Closed));
    assert!(matches!(
        bridge.close().expect_err("double close"),
        RealAudioError::Closed
    ));
    let _ = bridge.stats().callbacks.load(Ordering::Acquire);
}

/// Format change = close + reopen with the new spec; the report reflects the
/// new request and the obtained side SDL actually granted.
fn scenario_reopen_format() {
    let bridge = open_dummy(48_000, 2, 512);
    let first = bridge.spec_report().clone();
    assert_eq!(first.requested_rate, 48_000);
    assert_eq!(first.driver, "dummy");
    assert!(first.obtained_rate > 0, "obtained spec recorded");
    let mut reopened = bridge.reopen(22_050, 1, 256, 4, 50).expect("reopen");
    let second = reopened.spec_report().clone();
    assert_eq!(second.requested_rate, 22_050);
    assert_eq!(second.requested_channels, 1);
    assert_eq!(second.requested_block_frames, 256);
    assert!(second.obtained_rate > 0);
    reopened.close().unwrap();
}

/// Submit validation: an empty buffer is a no-op; a partial frame is
/// rejected explicitly; nothing is accounted for a rejected submit.
fn scenario_submit_validation() {
    let mut bridge = open_dummy(48_000, 2, 512);
    assert_eq!(bridge.submit(&[]).unwrap(), 0, "empty is a no-op");
    let error = bridge.submit(&[0.0f32; 3]).expect_err("3 samples / 2 channels");
    assert!(matches!(error, RealAudioError::InvalidSubmit(_)));
    assert_eq!(bridge.stats().submitted_frames.load(Ordering::Acquire), 0);
    bridge.close().unwrap();
}

#[test]
fn all_dummy_device_lifecycle_scenarios() {
    // SDL picks its driver at first subsystem init from the environment.
    std::env::set_var("SDL_AUDIODRIVER", "dummy");
    scenario_late_producer();
    scenario_bounded_queue();
    scenario_pause_resume_close();
    scenario_reopen_format();
    scenario_submit_validation();
}
