//! Ticket 92 acceptance: a synthetic producer drives the real audio-device
//! bridge through the deployed capability's C API, and the output is
//! verified for CONTENT — continuous non-zero PCM, correct duration, and
//! correct tone frequency — with a frequency/duration check that a pure
//! counting sink cannot provide.
//!
//! Device level: the SDL `disk` virtual device (SDL_AUDIODRIVER=disk)
//! writes the device stream to a WAV file. This is a REAL SDL device open
//! with a full callback loop through SDL's conversion layer, explicitly
//! labeled a VIRTUAL device: audibility is NOT claimed here (no speaker is
//! attached in the container) — level B of the acceptance ladder, distinct
//! from the level-A real-speaker test that stays device-side (tickets
//! 55/37) and from the level-C counting/file sinks of ticket 55.

#![cfg(feature = "sdl")]

use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn wav_path(name: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::AcqRel);
    std::env::temp_dir().join(format!("rudroid92-{}-{name}-{id}.wav", std::process::id()))
}

/// One second of 440 Hz stereo sine at 48000 Hz.
fn sine_frames(rate: i32, seconds: f64, freq: f64) -> Vec<f32> {
    let total = (rate as f64 * seconds) as usize;
    let mut out = Vec::with_capacity(total * 2);
    for n in 0..total {
        let value = (2.0 * std::f64::consts::PI * freq * n as f64 / rate as f64).sin() as f32;
        out.push(value);
        out.push(value);
    }
    out
}

/// Closed loop: producer -> bridge -> SDL disk DEVICE -> WAV file. The file
/// holds continuous non-zero samples whose duration matches the submitted
/// frames and whose tone is the submitted frequency.
#[test]
fn disk_device_closed_loop_duration_and_frequency_are_correct() {
    let out = wav_path("sine440");
    std::env::set_var("SDL_AUDIODRIVER", "disk");
    std::env::set_var("SDL_DISKAUDIOFILE", &out);

    const RATE: i32 = 48_000;
    const SECONDS: f64 = 1.0;
    const FREQ: f64 = 440.0;
    let mut bridge = platform_sdl_runtime::real_audio::AudioBridge::open(
        RATE, 2, 1024, 4, 250,
    )
    .expect("disk virtual device opens");
    let spec = bridge.spec_report().clone();
    assert!(
        spec.driver == "disk" || spec.driver == "dummy",
        "explicitly-labeled virtual device, got: {}",
        spec.driver
    );

    let frames = sine_frames(RATE, SECONDS, FREQ);
    let total_frames = frames.len() / 2;
    // Producer cadence: 10 ms blocks, like a real mixer tick. A partially
    // accepted block is resubmitted from the ACCEPTED offset only (never
    // re-counted), so `dropped` stays a true queue statistic.
    let block_samples = RATE as usize / 100 * 2;
    let mut offset = 0usize;
    while offset < frames.len() {
        let accepted = bridge
            .submit(&frames[offset..(offset + block_samples).min(frames.len())])
            .expect("bridge open while submitting");
        offset += accepted * 2;
        if accepted == 0 {
            // Ring full: the device is draining — a bounded producer wait.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    // Wait for the device to consume everything (bounded wait).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while bridge.stats().pulled_frames.load(Ordering::Acquire) < total_frames as u64 {
        assert!(
            std::time::Instant::now() < deadline,
            "device never consumed the submission"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    bridge.close().expect("clean close (audio thread joined)");

    // SDL 2.0.14's disk driver streams the raw device data and leaves the
    // WAV RIFF header as placeholder zeros (it never rewrites it on this
    // version), so the file is verified as a raw f32 LE stereo stream. The
    // leading zeros are REAL leading silence: the device started consuming
    // before the producer's first block (bounded producer cadence), and the
    // stream length is the submission rounded up to whole device blocks.
    let data = std::fs::read(&out).expect("disk driver wrote the stream");
    assert_eq!(data.len() % 8, 0, "whole stereo f32 frames");

    let samples: Vec<f32> = data
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let frames_out = samples.len() / 2;
    let duration = frames_out as f64 / RATE as f64;
    assert!(
        duration >= SECONDS && duration - SECONDS < 0.05,
        "duration must cover the submitted frames (device-block rounding up): \
         {duration}s vs {SECONDS}s"
    );
    let nonzero = samples.iter().filter(|s| s.abs() > 0.01).count();
    assert!(
        nonzero > samples.len() * 3 / 10,
        "the output must be continuous non-zero PCM (nonzero {nonzero}/{})",
        samples.len()
    );
    // Frequency via zero crossings on the left channel, measured inside the
    // NON-ZERO span only (the padded stream starts with the real leading
    // silence and ends with device-block zero padding): crossings ≈ f·span.
    let first = samples.iter().position(|s| s.abs() > 0.01).expect("nonzero span");
    let last = samples.iter().rposition(|s| s.abs() > 0.01).expect("nonzero span");
    // Frame-align the analysis window (first may land mid-frame).
    let first = first - (first % 2);
    let frames: Vec<&[f32]> = samples[first..=last].chunks_exact(2).collect();
    let span_frames = frames.len();
    let crossings = frames
        .windows(2)
        .filter(|pair| pair[0][0] < 0.0 && pair[1][0] >= 0.0)
        .count();
    let measured = crossings as f64 / (span_frames as f64 / RATE as f64);
    assert!(
        (measured - FREQ).abs() < FREQ * 0.05,
        "tone frequency must survive the chain: measured {measured} Hz vs {FREQ} Hz"
    );
    // The chain observability record (ticket 92 → 93 baseline input).
    eprintln!(
        "T92 OBSERVATION {}",
        serde_json_to_string(&spec, bridge.stats(), total_frames)
    );
    let _ = std::fs::remove_file(&out);
}

// Tiny local JSON writer (the crate has no serde dependency on purpose).
fn serde_json_to_string(
    spec: &platform_sdl_runtime::real_audio::SpecReport,
    stats: &platform_sdl_runtime::real_audio::AudioStats,
    submitted: usize,
) -> String {
    use std::sync::atomic::Ordering as O;
    let load = |a: &std::sync::atomic::AtomicU64| a.load(O::Acquire);
    format!(
        concat!(
            r#"{{"driver":"{}","requested":{{"rate":{},"channels":{},"block":{},"format":"{}"}},"#,
            r#""allowed_changes":{},"#,
            r#""obtained":{{"rate":{},"channels":{},"block":{},"format":"{}"}},"#,
            r#""producer_frames":{},"pulled_frames":{},"underruns":{},"dropped":{},"#,
            r#""callbacks":{},"max_queue_frames":{},"open_failures":{},"#,
            r#""submit_ns":{},"submit_calls":{},"callback_ns":{},"#,
            r#""level":"virtual-disk-device","audible":false}}"#
        ),
        spec.driver,
        spec.requested_rate,
        spec.requested_channels,
        spec.requested_block_frames,
        spec.app_format,
        spec.allowed_changes,
        spec.obtained_rate,
        spec.obtained_channels,
        spec.obtained_block_frames,
        spec.obtained_format,
        submitted,
        load(&stats.pulled_frames),
        load(&stats.underruns),
        load(&stats.dropped_frames),
        load(&stats.callbacks),
        load(&stats.max_queue_frames),
        load(&stats.open_failures),
        load(&stats.submit_ns),
        load(&stats.submit_calls),
        load(&stats.callback_ns),
    )
}
