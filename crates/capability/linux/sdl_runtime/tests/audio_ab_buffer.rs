//! Ticket 93: resampling-attribution and buffer A/B on the ticket-92 bridge.
//!
//! Container constraints shape this experiment honestly:
//! - Conversion attribution is recorded, not concluded: the bridge itself
//!   has no conversion stage (f32 in, f32 out), but the device is opened
//!   with the reference `SDL_AUDIO_ALLOW_SAMPLES_CHANGE`, so SDL reports the
//!   application rate/channels/format back BY CONSTRUCTION and converts to
//!   the driver's real format out of sight. Requested == obtained is
//!   therefore NOT evidence of zero conversion (the earlier ticket-93
//!   reading was tautological), and no conversion implementation is
//!   changed. Tone integrity through the whole chain is the disk closed
//!   loop of ticket 92.
//! - Buffer A/B: the card's candidates 32/64/128 ms are measured as BLOCK
//!   sizes with a paced producer, recording queue watermark, underruns,
//!   drops, callback cadence and the latency lower bound (capacity + one
//!   block). Buffer SIZE trades delivery latency against burst tolerance;
//!   this experiment measures exactly that trade and changes NO default.
//! - Producer/mixer cost is measured separately from the sink: submit-side
//!   nanoseconds per frame are recorded per configuration.
//! - Frame-time percentiles (p50/p95/p99) are a guest-render metric; no
//!   guest render loop runs in the container, so they are recorded N/A
//!   rather than extrapolated.

#![cfg(feature = "sdl")]

use platform_sdl_runtime::real_audio::AudioBridge;
use std::sync::atomic::Ordering;

const RATE: i32 = 48_000;
const CHANNELS: u8 = 2;
const SECONDS: f64 = 0.5;

fn sine_block(from_frame: usize, frames: usize, freq: f64) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let v =
                (2.0 * std::f64::consts::PI * freq * (from_frame + n) as f64 / RATE as f64) as f32;
            [v, v]
        })
        .collect()
}

struct RunResult {
    block_frames: u32,
    capacity_frames: u64,
    max_queue_frames: u64,
    underruns: u64,
    dropped: u64,
    callbacks: u64,
    pulled_frames: u64,
    submitted_frames: u64,
    submit_ns_per_frame: f64,
    latency_lower_bound_ms: f64,
    callback_interval_p50_us: f64,
    callback_interval_p95_us: f64,
    callback_interval_p99_us: f64,
}

fn run_once(block_frames: u32, paced: bool) -> RunResult {
    std::env::set_var("SDL_AUDIODRIVER", "dummy");
    let mut bridge =
        AudioBridge::open(RATE, CHANNELS, block_frames, 4, 50).expect("dummy device opens");
    let capacity = bridge.capacity_frames() as u64;

    let total_frames = (RATE as f64 * SECONDS) as usize;
    let mut offset = 0usize;
    let mut submit_ns: u128 = 0;
    let mut intervals_us: Vec<f64> = Vec::new();
    let mut last_callbacks = 0u64;
    let mut last_seen_ns = std::time::Instant::now();

    let start = std::time::Instant::now();
    while offset < total_frames {
        let frames_now = block_frames as usize / 4; // quarter-block producer ticks
        let chunk = sine_block(offset, frames_now.min(total_frames - offset), 440.0);
        let t0 = std::time::Instant::now();
        let accepted = bridge.submit(&chunk).expect("open bridge");
        submit_ns += t0.elapsed().as_nanos();
        offset += accepted; // submit() returns FRAMES
        if accepted == 0 {
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
        if paced {
            // Real-time pacing: consume at production rate, no bursts.
            std::thread::sleep(std::time::Duration::from_millis(
                1000 * frames_now as u64 / RATE as u64,
            ));
        }
        let callbacks = bridge.stats().callbacks.load(Ordering::Acquire);
        if callbacks != last_callbacks {
            let now = std::time::Instant::now();
            intervals_us.push(now.duration_since(last_seen_ns).as_secs_f64() * 1e6);
            last_seen_ns = now;
            last_callbacks = callbacks;
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while bridge.stats().pulled_frames.load(Ordering::Acquire) < total_frames as u64 {
        assert!(std::time::Instant::now() < deadline, "device never drained");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let _ = start;
    let stats = bridge.stats();
    let load = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Acquire);
    intervals_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |p: f64| {
        intervals_us
            .get(((intervals_us.len() as f64) * p) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let result = RunResult {
        block_frames,
        capacity_frames: capacity,
        max_queue_frames: load(&stats.max_queue_frames),
        underruns: load(&stats.underruns),
        dropped: load(&stats.dropped_frames),
        callbacks: load(&stats.callbacks),
        pulled_frames: load(&stats.pulled_frames),
        submitted_frames: load(&stats.submitted_frames),
        submit_ns_per_frame: submit_ns as f64 / offset as f64,
        latency_lower_bound_ms: (capacity + block_frames as u64) as f64 * 1000.0 / RATE as f64,
        callback_interval_p50_us: pick(0.50),
        callback_interval_p95_us: pick(0.95),
        callback_interval_p99_us: pick(0.99),
    };
    bridge.close().unwrap();
    result
}

fn json(r: &RunResult, paced: bool) -> String {
    format!(
        concat!(
            r#"{{"block_ms":{},"paced":{},"capacity_frames":{},"max_queue_frames":{},"#,
            r#""underruns":{},"dropped":{},"callbacks":{},"pulled_frames":{},"submitted_frames":{},"#,
            r#""submit_ns_per_frame":{:.1},"latency_lower_bound_ms":{:.2},"#,
            r#""callback_interval_us":{{"p50":{:.0},"p95":{:.0},"p99":{:.0}}},"#,
            r#""frame_time_percentiles":"N/A (no guest render loop in container)"}}"#
        ),
        r.block_frames * 1000 / RATE as u32,
        paced,
        r.capacity_frames,
        r.max_queue_frames,
        r.underruns,
        r.dropped,
        r.callbacks,
        r.pulled_frames,
        r.submitted_frames,
        r.submit_ns_per_frame,
        r.latency_lower_bound_ms,
        r.callback_interval_p50_us,
        r.callback_interval_p95_us,
        r.callback_interval_p99_us,
    )
}

#[test]
fn buffer_ab_matrix_32_64_128_ms() {
    std::env::set_var("SDL_AUDIODRIVER", "dummy");
    // SDL initializes once per process on ONE thread: all SDL work runs in
    // this single test (the attribution scenario is part of this test, not
    // a parallel one).
    for block_ms in [32u64, 64, 128] {
        let block_frames = (RATE as u64 * block_ms / 1000) as u32;
        for paced in [true, false] {
            let result = run_once(block_frames, paced);
            eprintln!("T93 A/B {}", json(&result, paced));
            // Invariants every configuration must hold.
            assert_eq!(result.pulled_frames, result.submitted_frames.min(total_frames() as u64), "delivery is frame-exact");
            assert!(result.max_queue_frames <= result.capacity_frames, "queue bounded");
            if paced {
                assert_eq!(result.dropped, 0, "a paced producer never hits backpressure");
            }
        }
    }
    conversion_attribution_record();
}

fn total_frames() -> usize {
    (RATE as f64 * SECONDS) as usize
}

/// The attribution record: application format and the SDL-reported
/// obtained spec, side by side with the `allowed_changes` that fixes the
/// obtained rate/channels/format to the request. Only the block size can
/// legitimately differ; a device-side conversion is not observable here.
fn conversion_attribution_record() {
    let mut bridge = open_dummy_report();
    let spec = bridge.spec_report().clone();
    assert_eq!(spec.allowed_changes, platform_sdl_runtime::real_audio::ALLOWED_CHANGES);
    assert_eq!(spec.app_format, "F32LSB");
    bridge.close().unwrap();
    eprintln!(
        "T93 ATTRIBUTION app={} Hz/{} ch/{} block={} obtained={} Hz/{} ch/{} block={} \
         allowed_changes={:#x} driver={}: obtained rate/channels/format are fixed to the \
         request by allowed_changes; device-side conversion NOT observable (no zero-conversion claim)",
        spec.requested_rate,
        spec.requested_channels,
        spec.app_format,
        spec.requested_block_frames,
        spec.obtained_rate,
        spec.obtained_channels,
        spec.obtained_format,
        spec.obtained_block_frames,
        spec.allowed_changes,
        spec.driver
    );
}

#[test]
#[ignore] // folded into the matrix test: SDL inits once per process/thread
fn conversion_attribution_record_standalone() {
    conversion_attribution_record();
}

fn open_dummy_report() -> AudioBridge {
    std::env::set_var("SDL_AUDIODRIVER", "dummy");
    AudioBridge::open(RATE, CHANNELS, 1024, 4, 50).expect("dummy device opens")
}
