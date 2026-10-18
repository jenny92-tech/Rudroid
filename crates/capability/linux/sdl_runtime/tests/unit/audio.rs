//! INPUT:  private items of `src/audio.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/audio.rs`; declared there with `#[path]`
use super::*;

#[test]
fn output_policy_retries_are_idempotent_but_cannot_replace_live_values() {
    let slot = std::sync::OnceLock::new();
    let first = OutputPolicy::default();
    assert!(publish_output_policy(&slot, first).is_ok());
    assert!(publish_output_policy(&slot, first).is_ok());
    for changed in [
        OutputPolicy { default_attempts: 1, ..first },
        OutputPolicy { named_attempts: 1, ..first },
        OutputPolicy { retry_ms: 0, ..first },
        OutputPolicy { enumerate: false, ..first },
        OutputPolicy { prefer_non_hdmi: false, ..first },
        OutputPolicy { reject_silent: false, ..first },
    ] {
        assert!(publish_output_policy(&slot, changed).is_err());
        assert_eq!(slot.get(), Some(&first));
    }
}

#[test]
fn concurrent_output_policy_publish_keeps_one_immutable_winner() {
    let slot = std::sync::OnceLock::new();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8).map(|i| {
            let (slot, barrier) = (&slot, &barrier);
            scope.spawn(move || {
                let policy = OutputPolicy { retry_ms: if i % 2 == 0 { 100 } else { 200 }, ..OutputPolicy::default() };
                barrier.wait();
                let accepted = publish_output_policy(slot, policy).is_ok();
                assert_eq!(accepted, slot.get() == Some(&policy));
            })
        }).collect();
        for worker in workers { worker.join().unwrap(); }
    });
    let winner = *slot.get().unwrap();
    assert!(publish_output_policy(&slot, winner).is_ok());
    assert_eq!(slot.get(), Some(&winner));
}

fn policy(reject_silent: u32) -> AudioPolicyParams {
    // Reference `platform_audio_sdl` values ("sdl-adaptive": 4/5/250).
    AudioPolicyParams {
        sample_rate: 48_000,
        channels: 2,
        open_attempts: 4,
        retry_delay_ms: 250,
        reject_silent_drivers: reject_silent,
    }
}

#[test]
fn frames_are_samples_divided_by_channels() {
    let mut pump = AudioPump::open(policy(0), None).unwrap();
    assert_eq!(
        pump.write(&[0.25f32; 8]).unwrap(),
        4,
        "8 samples / 2 channels = 4 frames"
    );
    assert_eq!(pump.frames_written(), 4);
    assert_eq!(pump.write_calls(), 1);
}

#[test]
fn empty_buffer_writes_no_frames_and_no_sink_call() {
    let mut pump = AudioPump::open(policy(0), None).unwrap();
    assert_eq!(pump.write(&[]).unwrap(), 0);
    assert_eq!(pump.frames_written(), 0);
    assert_eq!(
        pump.write_calls(),
        0,
        "the sink must not be invoked for nothing"
    );
}

#[test]
fn partial_frame_is_rejected_without_counting() {
    let mut pump = AudioPump::open(policy(0), None).unwrap();
    let err = pump.write(&[0.5f32; 3]).unwrap_err();
    assert!(
        err.to_string().contains("not a whole number of frames"),
        "{err}"
    );
    assert_eq!(pump.frames_written(), 0);
    assert_eq!(pump.write_calls(), 0);
}

#[test]
fn reject_silent_drivers_forbids_the_dummy_sink() {
    let err = AudioPump::open(policy(1), None).unwrap_err();
    assert!(err.to_string().contains("rejects silent drivers"), "{err}");
    // With a file sink the same policy opens: the rejection targets
    // the silent stand-in, not file output.
    let dir = std::env::temp_dir().join(format!("rudroid-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(AudioPump::open(policy(1), Some(&dir.join("raw.pcm"))).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn policy_bounds_match_the_reference_validation() {
    let mut bad = policy(0);
    bad.open_attempts = 0;
    assert!(AudioPump::open(bad, None).is_err());
    bad.open_attempts = 101;
    assert!(AudioPump::open(bad, None).is_err());
    bad.open_attempts = 4;
    bad.retry_delay_ms = 10_001;
    assert!(AudioPump::open(bad, None).is_err());
    bad.retry_delay_ms = 250;
    bad.sample_rate = 0;
    assert!(AudioPump::open(bad, None).is_err());
}

#[test]
fn file_sink_persists_interleaved_samples() {
    let dir = std::env::temp_dir().join(format!("rudroid-audio-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("raw.pcm");
    let mut pump = AudioPump::open(policy(0), Some(&path)).unwrap();
    // A channel sweep so the file proves interleaved data flowed.
    let samples: Vec<f32> = (0..8).map(|i| (i as f32) * 0.25 - 0.5).collect();
    assert_eq!(pump.write(&samples).unwrap(), 4);
    drop(pump);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 8 * 4, "one LE f32 per sample");
    assert!(bytes.iter().any(|b| *b != 0), "samples must be non-zero");
    let written: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(written, samples);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn real_output_policy_orders_hdmi_after_speakers_and_identifies_silent_drivers() {
    let mut names =
        ["HDMI 0", "Speaker", "headphones", "HDMI 1"].map(|n| std::ffi::CString::new(n).unwrap());
    order_output_names(&mut names, true);
    assert_eq!(
        names
            .iter()
            .map(|n| n.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["Speaker", "headphones", "HDMI 0", "HDMI 1"]
    );
    for name in ["dummy", "DISK", "NoSound"] {
        assert!(silent_driver(name));
    }
    assert!(!silent_driver("pulseaudio"));
}
