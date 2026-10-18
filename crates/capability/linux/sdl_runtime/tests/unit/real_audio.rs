//! INPUT:  private items of `src/real_audio.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/real_audio.rs`; declared there with `#[path]`
use super::*;

#[test]
fn spsc_ring_preserves_frames_across_wraparound() {
    let ring = Arc::new(FrameRing::new(7, 2));
    let producer = Arc::clone(&ring);
    let thread = std::thread::spawn(move || {
        for i in 1..=10_000 {
            while producer.push(&[i as f32, -(i as f32)]) == 0 {
                std::thread::yield_now();
            }
        }
    });
    for i in 1..=10_000 {
        let mut out = [0.0; 2];
        while ring.pull(&mut out) == 0 {
            std::thread::yield_now();
        }
        assert_eq!(out, [i as f32, -(i as f32)]);
    }
    thread.join().unwrap();
    assert_eq!(ring.queued_frames(), 0);
}

/// Unequal block sizes cross both the storage wrap and the counter cycle,
/// including partial accepts and underrun tails. Every stereo frame must
/// arrive exactly once, in order; empty pulls must overwrite stale output.
#[test]
fn spsc_ring_preserves_mixed_blocks_across_wraparound() {
    const TOTAL: usize = 1_000_003;
    let ring = Arc::new(FrameRing::new(4093, 2));
    let producer = Arc::clone(&ring);
    let started = Instant::now();
    let thread = std::thread::spawn(move || {
        let mut input = [0.0; 1536 * 2];
        let mut sent = 0;
        let mut block = 0;
        let mut submit_ns = 0;
        let mut calls = 0;
        while sent < TOTAL {
            assert!(started.elapsed() < Duration::from_secs(30));
            let frames = [1, 17, 256, 1024, 1536][block % 5].min(TOTAL - sent);
            for (i, frame) in input[..frames * 2].chunks_exact_mut(2).enumerate() {
                let value = (sent + i + 1) as f32;
                frame.copy_from_slice(&[value, -value]);
            }
            let before = Instant::now();
            let accepted = producer.push(&input[..frames * 2]);
            submit_ns += before.elapsed().as_nanos();
            calls += 1;
            sent += accepted;
            block += 1;
            if accepted == 0 {
                std::thread::yield_now();
            }
        }
        (submit_ns, calls)
    });
    let mut out = [0.0; 769 * 2];
    let mut received = 0;
    let mut block = 0;
    let mut pull_ns = 0;
    let mut calls = 0;
    while received < TOTAL {
        assert!(started.elapsed() < Duration::from_secs(30));
        let frames = [3, 127, 512, 769][block % 4];
        out.fill(f32::NAN);
        let before = Instant::now();
        let served = ring.pull(&mut out[..frames * 2]);
        pull_ns += before.elapsed().as_nanos();
        calls += 1;
        for (i, frame) in out[..served * 2].chunks_exact(2).enumerate() {
            let value = (received + i + 1) as f32;
            assert_eq!(frame, [value, -value]);
        }
        assert!(out[served * 2..frames * 2].iter().all(|s| *s == 0.0));
        received += served;
        block += 1;
        if served == 0 {
            std::thread::yield_now();
        }
    }
    let (submit_ns, submit_calls) = thread.join().unwrap();
    assert_eq!(received, TOTAL);
    assert_eq!(ring.queued_frames(), 0);
    eprintln!(
        "SPSC mixed-block observation frames={TOTAL} submit_ns={submit_ns} \
         submit_calls={submit_calls} pull_ns={pull_ns} pull_calls={calls} \
         elapsed_ns={}",
        started.elapsed().as_nanos()
    );
}

#[test]
fn callback_peak_measures_current_pcm_and_keeps_maximum() {
    let shared = Arc::new(Shared {
        ring: FrameRing::new(4, 1),
        stats: AudioStats::default(),
    });
    let callback = Pull {
        shared: OnceLock::from(Arc::clone(&shared)),
    };
    let mut out = [99.0]; // Not PCM: must be overwritten before observing.
    for value in [0.25, -0.5, 0.125] {
        assert_eq!(shared.ring.push(&[value]), 1);
        callback.observe(&mut out);
        assert_eq!(out, [value]);
    }
    assert_eq!(
        f32::from_bits(shared.stats.peak_abs_bits.load(Ordering::Acquire)),
        0.5
    );
    callback.observe(&mut out);
    assert_eq!(out, [0.0]);
    assert_eq!(
        f32::from_bits(shared.stats.peak_abs_bits.load(Ordering::Acquire)),
        0.5
    );
}

/// A third thread reading the queue depth while the producer and the
/// consumer race must never underflow (the old write-then-read snapshot
/// could see a consumer that had overtaken the stale write position)
/// and never report more than the capacity.
#[test]
fn queued_frames_snapshot_is_safe_from_a_third_thread() {
    let ring = Arc::new(FrameRing::new(8, 1));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let producer = {
        let ring = Arc::clone(&ring);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                ring.push(&[1.0; 3]);
            }
        })
    };
    let consumer = {
        let ring = Arc::clone(&ring);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut out = [0.0; 3];
            while !stop.load(Ordering::Relaxed) {
                ring.pull(&mut out);
            }
        })
    };
    for _ in 0..2_000_000 {
        assert!(ring.queued_frames() <= ring.capacity_frames());
    }
    stop.store(true, Ordering::Relaxed);
    producer.join().unwrap();
    consumer.join().unwrap();
}

#[test]
fn block_frames_above_the_sdl_u16_power_of_two_are_rejected() {
    // RED pre-fix: 32769..=65536 rounded to 65536 and truncated to a
    // 0-sample request.
    for block in [32_769u32, 65_536, 0] {
        match AudioBridge::open(48_000, 2, block, 1, 0) {
            Err(RealAudioError::InvalidSpec(reason)) => {
                assert!(reason.contains("1..=32768"), "{reason}");
            }
            Err(other) => panic!("block {block}: expected InvalidSpec, got {other}"),
            Ok(_) => panic!("block {block}: must be rejected before SDL"),
        }
    }
    assert_eq!(MAX_BLOCK_FRAMES.next_power_of_two() as u16, 32_768);
}

#[test]
fn format_names_follow_the_sdl_spelling() {
    assert_eq!(format_name(sys::AUDIO_F32LSB as u16), APP_FORMAT);
    assert_eq!(format_name(sys::AUDIO_S16LSB as u16), "S16LSB");
    assert_eq!(format_name(0x1234), "0x1234");
    assert_eq!(
        ALLOWED_CHANGES, 8,
        "reference SDL_AUDIO_ALLOW_SAMPLES_CHANGE"
    );
}

#[test]
fn ring_capacity_follows_the_obtained_block() {
    // RED pre-fix: request 256, obtain 2048 → a 1024-frame ring.
    assert_eq!(ring_capacity_frames(256, 2048).unwrap(), 8192);
    assert_eq!(ring_capacity_frames(1536, 1024).unwrap(), 6144);
    assert!(matches!(
        ring_capacity_frames(256, 0),
        Err(RealAudioError::Sdl(_))
    ));
}

/// A driver period larger than the request: with the ring sized from the
/// obtained block, a full callback is served without an underrun once
/// the producer filled one period (the request-sized ring could never
/// hold it).
#[test]
fn obtained_block_larger_than_request_is_served_without_underrun() {
    let (requested, obtained) = (256u32, 2048u16);
    let capacity = ring_capacity_frames(requested, obtained).unwrap();
    let shared = Arc::new(Shared {
        ring: FrameRing::new(capacity, 2),
        stats: AudioStats::default(),
    });
    let callback = Pull {
        shared: OnceLock::from(Arc::clone(&shared)),
    };
    let period = vec![0.25f32; obtained as usize * 2];
    assert_eq!(
        shared.ring.push(&period),
        obtained as usize,
        "one obtained period fits"
    );
    let mut out = vec![0.0f32; obtained as usize * 2];
    callback.observe(&mut out);
    assert_eq!(shared.stats.underruns.load(Ordering::Acquire), 0);
    assert_eq!(
        shared.stats.pulled_frames.load(Ordering::Acquire),
        u64::from(obtained)
    );
    assert!(out.iter().all(|s| *s == 0.25));
    // The old request-sized ring accepted only part of one period.
    assert!((requested as usize * 4) < obtained as usize);
}

#[test]
fn unbound_callback_writes_silence() {
    let callback = Pull {
        shared: OnceLock::new(),
    };
    let mut out = [1.0f32; 8];
    callback.observe(&mut out);
    assert_eq!(out, [0.0; 8]);
}

#[test]
fn non_power_of_two_ring_reuses_slots_across_the_counter_cycle() {
    let ring = FrameRing::new(7, 1);
    ring.write_pos.store(13, Ordering::Relaxed);
    ring.read_pos.store(13, Ordering::Relaxed);
    assert_eq!(ring.push(&[1., 2., 3., 4., 5., 6., 7., 8.]), 7);
    assert_eq!(ring.queued_frames(), 7);
    assert_eq!(ring.push(&[99.]), 0);
    let mut out = [0.; 9];
    assert_eq!(ring.pull(&mut out), 7);
    assert_eq!(out, [1., 2., 3., 4., 5., 6., 7., 0., 0.]);
    assert_eq!(ring.queued_frames(), 0);
    assert_eq!(ring.push(&[8., 9.]), 2);
    let mut next = [0.; 2];
    assert_eq!(ring.pull(&mut next), 2);
    assert_eq!(next, [8., 9.]);
}

#[test]
#[ignore = "run on handheld with real SDL audio driver; verifies callbacks and policy, not speaker audibility"]
fn handheld_audio_policy_consumes_pcm_in_real_callback() {
    let mut bridge =
        AudioBridge::open_with_policy(48000, 2, 1024, super::super::audio::OutputPolicy::default())
            .unwrap();
    assert!(!super::super::audio::silent_driver(
        &bridge.spec_report().driver
    ));
    bridge.submit(&vec![0.025; 2048]).unwrap();
    for _ in 0..500 {
        if bridge.stats().pulled_frames.load(Ordering::Acquire) >= 1024 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(bridge.stats().pulled_frames.load(Ordering::Acquire) >= 1024);
    assert_eq!(
        f32::from_bits(bridge.stats().peak_abs_bits.load(Ordering::Acquire)),
        0.025
    );
    bridge.close().unwrap();
    assert!(matches!(
        bridge.submit(&[0.0; 2]),
        Err(RealAudioError::Closed)
    ));
}

#[test]
#[ignore = "run on handheld with real audio; bounded producer wait, PCM consumption and lifecycle"]
fn handheld_audio_wait_preserves_pcm_without_overflow() {
    let mut bridge = AudioBridge::open(48_000, 2, 1024, 1, 0).unwrap();
    assert!(!super::super::audio::silent_driver(&bridge.spec_report().driver));
    bridge.pause().unwrap();
    let pcm = vec![0.025; 2048];
    let capacity = bridge.capacity_frames();
    assert!(bridge.wait_for_space(0, Duration::ZERO).unwrap());
    assert!(bridge.wait_for_space(capacity + 1, Duration::ZERO).is_err());
    while bridge.queued_frames() + 1024 <= capacity {
        assert_eq!(bridge.submit(&pcm).unwrap(), 1024);
    }
    let queued = bridge.queued_frames();
    assert!(!bridge.wait_for_space(capacity, Duration::from_millis(20)).unwrap());
    assert_eq!(bridge.queued_frames(), queued, "timed out wait must not alter PCM");
    bridge.resume().unwrap();
    let start = Instant::now();
    let mut warmup = None;
    let mut writes = 0;
    while start.elapsed() < Duration::from_secs(5) {
        assert!(bridge.wait_for_space(1024, Duration::from_secs(1)).unwrap());
        assert_eq!(bridge.submit(&pcm).unwrap(), 1024);
        writes += 1;
        if warmup.is_none() && start.elapsed() >= Duration::from_secs(1) {
            let s = bridge.stats();
            warmup = Some((s.underruns.load(Ordering::Acquire),
                s.dropped_frames.load(Ordering::Acquire), s.callbacks.load(Ordering::Acquire),
                s.pulled_frames.load(Ordering::Acquire)));
        }
    }
    bridge.pause().unwrap();
    let (underruns, drops, callbacks, pulled) = warmup.unwrap();
    let s = bridge.stats();
    let steady_underruns = s.underruns.load(Ordering::Acquire) - underruns;
    let steady_drops = s.dropped_frames.load(Ordering::Acquire) - drops;
    let steady_callbacks = s.callbacks.load(Ordering::Acquire) - callbacks;
    let steady_pulled = s.pulled_frames.load(Ordering::Acquire) - pulled;
    assert!(steady_callbacks > 100 && steady_pulled > 100_000);
    assert_eq!(steady_underruns, 0);
    assert_eq!(steady_drops, 0);
    assert_eq!(f32::from_bits(s.peak_abs_bits.load(Ordering::Acquire)), 0.025);
    eprintln!("audio-wait observation driver={} elapsed_ns={} writes={writes} steady_callbacks={steady_callbacks} steady_pulled={steady_pulled} steady_underruns={steady_underruns} steady_drops={steady_drops} warmup_underruns={underruns}", bridge.spec_report().driver, start.elapsed().as_nanos());
    bridge.close().unwrap();
    assert!(matches!(bridge.wait_for_space(0, Duration::ZERO), Err(RealAudioError::Closed)));
}
