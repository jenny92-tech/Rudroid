//! Ticket 92: the real audio-device bridge. A guest-side producer hands
//! interleaved f32 frames across the deployed capability's C API into an
//! SDL-owned device, and every observable of the chain — requested vs
//! obtained spec, actual driver (including NOSOUND/dummy/disk virtual
//! devices), queue depth, underruns, overflow drops, callback counts — is
//! exported for the sampling-chain observation record (the 93 A/B baseline).
//!
//! Contract notes:
//! - The callback hot path does NO allocation, NO I/O, NO logging and holds
//!   no lock across any external call: it drains a fixed-capacity SPSC ring
//!   through atomics only. Bounded queue: a producer outrunning the device
//!   DROPS the excess and counts it — the queue can never grow.
//! - Underrun is not fatal: the callback emits silence for the missing
//!   frames and counts the event.
//! - Device close joins the SDL audio thread before returning (`AudioDevice::
//!   close` consumes the device and hands back the callback), so no callback
//!   is in flight afterwards — a submit after close is a structured error,
//!   never a dangling callback.
//! - Open retries follow the registered policy ladder (`open_attempts`,
//!   `retry_delay_ms`) and fail with a structured diagnostic naming the
//!   driver after the ladder is exhausted.
//! - The device is opened through the raw `SDL_OpenAudioDevice` with the
//!   reference `allowed_changes` (`SDL_AUDIO_ALLOW_SAMPLES_CHANGE`, as in
//!   `fakefmod.cpp`); rust-sdl2's `open_playback` passes 0. Under these
//!   flags SDL may change only the block size: rate, channels and format of
//!   the obtained spec equal the application request BY CONSTRUCTION, and
//!   SDL converts to whatever the driver really runs. The record therefore
//!   keeps the application format (what the callback is fed) and the
//!   obtained spec separately, and neither proves the hardware format or
//!   the absence of a conversion stage (review 2026-09-19, audio section;
//!   SDL 2.0.14 offers no device-native spec query). Audibility is NOT
//!   claimed here: the container runs virtual devices (dummy/disk); a real
//!   speaker test stays device-side work (ticket 55/37 posture).
//! - Timing (ticket 92 step 4): `submit_ns` is the producer-side cost of
//!   handing frames to the ring, `callback_ns` the cost of writing them
//!   into SDL's device buffer. The bridge itself has no conversion stage
//!   (f32 in, f32 out); any SDL-internal conversion runs outside the
//!   callback and is not observable here.
//!
//! Threading (SPSC): one producer thread submits; the SDL audio thread is
//! the single consumer. Stats/spec/queue reads are safe from any thread at
//! any time. Pause/resume/close/release must not race with submit or with
//! each other.

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use sdl2::sys;
use rudroid_common::audio_backpressure::AudioBackpressure;

/// Reference `allowed_changes` (`fakefmod.cpp` opens its device with
/// `SDL_AUDIO_ALLOW_SAMPLES_CHANGE`).
pub const ALLOWED_CHANGES: u32 = sys::SDL_AUDIO_ALLOW_SAMPLES_CHANGE;
/// Sample format the callback is fed (and the producer submits).
pub const APP_FORMAT: &str = "F32LSB";
/// Largest block SDL can carry: `SDL_AudioSpec::samples` is a `Uint16`
/// rounded up to a power of two.
pub const MAX_BLOCK_FRAMES: u32 = 32_768;

#[derive(Debug)]
pub enum RealAudioError {
    /// Spec values out of range.
    InvalidSpec(String),
    /// The device open ladder failed; carries the last SDL diagnostic.
    OpenFailed { attempts: u32, last_error: String },
    /// The bridge is closed (or was never opened).
    Closed,
    /// Submit arguments invalid (null buffer, partial frame).
    InvalidSubmit(String),
    /// SDL subsystem failure.
    Sdl(String),
}

impl std::fmt::Display for RealAudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RealAudioError::InvalidSpec(reason) => write!(f, "invalid audio spec: {reason}"),
            RealAudioError::OpenFailed {
                attempts,
                last_error,
            } => write!(
                f,
                "audio device open failed after {attempts} attempt(s): {last_error}"
            ),
            RealAudioError::Closed => write!(f, "audio bridge is closed"),
            RealAudioError::InvalidSubmit(reason) => write!(f, "invalid submit: {reason}"),
            RealAudioError::Sdl(reason) => write!(f, "SDL audio failure: {reason}"),
        }
    }
}

impl std::error::Error for RealAudioError {}

/// The application request (rate/channels/block plus `app_format`) and the
/// spec `SDL_OpenAudioDevice` reported under `allowed_changes`. Both are
/// part of the observation record; neither proves the hardware format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecReport {
    pub requested_rate: i32,
    pub requested_channels: u8,
    pub requested_block_frames: u32,
    pub app_format: String,
    pub allowed_changes: u32,
    pub obtained_rate: i32,
    pub obtained_channels: u8,
    pub obtained_block_frames: u32,
    pub obtained_format: String,
    pub driver: String,
}

/// SDL sample-format name (the `AudioFormat` spelling).
pub fn format_name(format: u16) -> String {
    match u32::from(format) {
        sys::AUDIO_U8 => "U8".to_owned(),
        sys::AUDIO_S8 => "S8".to_owned(),
        sys::AUDIO_U16LSB => "U16LSB".to_owned(),
        sys::AUDIO_S16LSB => "S16LSB".to_owned(),
        sys::AUDIO_U16MSB => "U16MSB".to_owned(),
        sys::AUDIO_S16MSB => "S16MSB".to_owned(),
        sys::AUDIO_S32LSB => "S32LSB".to_owned(),
        sys::AUDIO_S32MSB => "S32MSB".to_owned(),
        sys::AUDIO_F32LSB => "F32LSB".to_owned(),
        sys::AUDIO_F32MSB => "F32MSB".to_owned(),
        other => format!("{other:#06x}"),
    }
}

/// Fixed-capacity SPSC frame ring. `write`/`read` cycle through twice the
/// capacity (acquire/release publish the payload), so the producer never
/// blocks and the callback never allocates. Capacity is frames × channels.
/// Exactly one thread may `push` and exactly one may `pull`;
/// `queued_frames` may be read from any thread.
pub struct FrameRing {
    buffer: Box<[std::cell::UnsafeCell<f32>]>,
    channels: usize,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
}

// SAFETY: strict SPSC — the producer writes only slots in
// [write_pos, write_pos+accepted) and publishes via the write_pos release;
// the consumer reads only [read_pos, write_pos) published by acquire. The
// slots are therefore never concurrently written, and both sides touch
// disjoint index ranges at any point in time.
unsafe impl Sync for FrameRing {}

impl FrameRing {
    fn new(capacity_frames: usize, channels: usize) -> FrameRing {
        FrameRing {
            buffer: (0..capacity_frames.max(1) * channels)
                .map(|_| std::cell::UnsafeCell::new(0.0))
                .collect(),
            channels,
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
        }
    }

    fn capacity_frames(&self) -> usize {
        self.buffer.len() / self.channels
    }

    fn distance(&self, write: usize, read: usize) -> usize {
        let cycle = self.capacity_frames() * 2;
        (write + cycle - read) % cycle
    }

    fn advance(&self, position: usize) -> usize {
        (position + 1) % (self.capacity_frames() * 2)
    }

    /// A third-thread snapshot can straddle consume/refill operations;
    /// clamp that diagnostic snapshot to capacity. On the producer, its
    /// own write position is stable, so the acquired read gives exact free slots.
    fn queued_frames(&self) -> usize {
        let read = self.read_pos.load(Ordering::Acquire);
        let write = self.write_pos.load(Ordering::Acquire);
        self.distance(write, read).min(self.capacity_frames())
    }

    /// Producer side: push whole frames; returns accepted FRAMES. The excess
    /// is dropped and counted — the ring never grows.
    fn push(&self, samples: &[f32]) -> usize {
        let queued = self.queued_frames();
        let free = self.capacity_frames() - queued;
        let channels = self.channels;
        let incoming = samples.len() / channels;
        let accepted = incoming.min(free);
        let mut write = self.write_pos.load(Ordering::Acquire);
        for frame in samples[..accepted * channels].chunks_exact(channels) {
            let slot = (write % self.capacity_frames()) * channels;
            for (cell, sample) in self.buffer[slot..slot + channels].iter().zip(frame) {
                // SAFETY: only the producer owns unpublished/free slots.
                unsafe { cell.get().write(*sample) };
            }
            write = self.advance(write);
            self.write_pos.store(write, Ordering::Release);
        }
        accepted
    }

    /// Consumer side (audio thread): drain up to `out.len()/channels` frames
    /// into `out`, zero-filling any shortfall (underrun silence).
    fn pull(&self, out: &mut [f32]) -> usize {
        let channels = self.channels;
        let wanted = out.len() / channels;
        let mut read = self.read_pos.load(Ordering::Acquire);
        let available = self.distance(self.write_pos.load(Ordering::Acquire), read);
        let served = wanted.min(available);
        for frame in 0..served {
            let slot = (read % self.capacity_frames()) * channels;
            let dst = frame * channels;
            for (sample, cell) in out[dst..dst + channels]
                .iter_mut()
                .zip(&self.buffer[slot..slot + channels])
            {
                // SAFETY: acquire observed the producer's publication; this
                // slot is not returned to the producer until read_pos.store.
                *sample = unsafe { cell.get().read() };
            }
            read = self.advance(read);
            self.read_pos.store(read, Ordering::Release);
        }
        for sample in out[served * channels..].iter_mut() {
            *sample = 0.0;
        }
        served
    }
}

/// Observable counters (atomics: the callback thread only ever increments).
#[derive(Default)]
pub struct AudioStats {
    /// Bit pattern of the largest |sample| the callback has seen — a
    /// diagnostics probe distinguishing "producer delivered silence" from
    /// "the device wrote silence" (ticket 92 evidence).
    pub peak_abs_bits: AtomicU32,
    pub submitted_frames: AtomicU64,
    pub dropped_frames: AtomicU64,
    pub pulled_frames: AtomicU64,
    pub underruns: AtomicU64,
    pub callbacks: AtomicU64,
    pub max_queue_frames: AtomicU64,
    pub open_failures: AtomicU64,
    pub paused_frames: AtomicU64,
    /// Producer cost: nanoseconds spent handing frames to the ring.
    pub submit_ns: AtomicU64,
    pub submit_calls: AtomicU64,
    /// Write cost: nanoseconds the callback spent filling SDL's buffer.
    pub callback_ns: AtomicU64,
}

/// Callback state handed to SDL; runs on the SDL audio thread. The ring is
/// sized from the OBTAINED block, so it is bound after the (paused) open
/// and before the first unpause; an unbound callback writes silence.
struct Pull {
    shared: OnceLock<Arc<Shared>>,
}

struct Shared {
    ring: FrameRing,
    stats: AudioStats,
}

impl Pull {
    fn observe(&self, out: &mut [f32]) {
        let Some(shared) = self.shared.get() else {
            out.fill(0.0);
            return;
        };
        let started = Instant::now();
        shared.stats.callbacks.fetch_add(1, Ordering::AcqRel);
        let channels = shared.ring.channels;
        let wanted = out.len() / channels;
        let queued = shared.ring.queued_frames();
        let served = shared.ring.pull(out);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        // Non-negative IEEE f32 bit patterns have the same ordering as
        // their values. Measure the PCM just delivered, not the old output.
        shared
            .stats
            .peak_abs_bits
            .fetch_max(peak.to_bits(), Ordering::AcqRel);
        shared
            .stats
            .pulled_frames
            .fetch_add(served as u64, Ordering::AcqRel);
        if served < wanted {
            shared.stats.underruns.fetch_add(1, Ordering::AcqRel);
        }
        let max = shared.stats.max_queue_frames.load(Ordering::Acquire);
        if queued as u64 > max {
            shared
                .stats
                .max_queue_frames
                .compare_exchange(max, queued as u64, Ordering::AcqRel, Ordering::Acquire)
                .ok();
        }
        shared
            .stats
            .callback_ns
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::AcqRel);
    }
}

/// `SDL_AudioCallback` trampoline: `userdata` is the device's `Pull`, the
/// stream is `len` bytes of `APP_FORMAT` samples (format and channels are
/// not in `allowed_changes`, so SDL feeds exactly the requested layout).
unsafe extern "C" fn audio_callback(userdata: *mut c_void, stream: *mut u8, len: c_int) {
    // SAFETY: SDL passes back the `Pull` the device was opened with; it
    // outlives the device (freed only after SDL_CloseAudioDevice).
    let pull = unsafe { &*(userdata as *const Pull) };
    let samples = len.max(0) as usize / std::mem::size_of::<f32>();
    // SAFETY: SDL hands a writable, suitably aligned buffer of `len` bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(stream as *mut f32, samples) };
    pull.observe(out);
}

/// An open SDL audio device plus the callback state it points at. Dropping
/// it closes the device (SDL joins the audio thread) BEFORE the callback
/// state is freed.
// SDL audio may be opened by the guest's producer thread. Own the native
// subsystem reference directly instead of rust-sdl2's main-thread Sdl token.
// Init/quit are serialized; each successful init is paired after device close.
static SUBSYSTEM_LIFETIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct AudioSubsystemLease;
impl AudioSubsystemLease {
    fn open() -> Result<Self, String> {
        let _guard = SUBSYSTEM_LIFETIME.lock().unwrap();
        // SAFETY: plain SDL call; init/quit serialized by the guard.
        if unsafe { sys::SDL_InitSubSystem(sys::SDL_INIT_AUDIO) } != 0 {
            Err(sdl2::get_error())
        } else {
            Ok(Self)
        }
    }
}
impl Drop for AudioSubsystemLease {
    fn drop(&mut self) {
        let _guard = SUBSYSTEM_LIFETIME.lock().unwrap();
        // SAFETY: pairs the successful init of this lease, under the guard.
        unsafe {
            sys::SDL_QuitSubSystem(sys::SDL_INIT_AUDIO);
        }
    }
}
struct RawDevice {
    id: sys::SDL_AudioDeviceID,
    pull: *mut Pull,
    _audio: AudioSubsystemLease,
}

// SAFETY: `pull` is owned by this device alone (freed only in Drop, after
// SDL_CloseAudioDevice joined the audio thread) and `Pull` is Send + Sync;
// SDL locks the device for pause/close from any thread, and subsystem
// init/quit are serialized by SUBSYSTEM_LIFETIME. Not Sync: `AudioBridge`
// relies on that for its single-producer `submit(&self)`.
unsafe impl Send for RawDevice {}

impl RawDevice {
    /// Hand the callback its ring; must happen before the first unpause.
    fn bind(&self, shared: Arc<Shared>) {
        // SAFETY: `pull` lives until Drop; OnceLock is safe to set while
        // the (paused) audio thread may read it.
        let _ = unsafe { &*self.pull }.shared.set(shared);
    }

    fn pause(&self, pause: bool) {
        // SAFETY: a device id from a successful SDL_OpenAudioDevice.
        unsafe { sys::SDL_PauseAudioDevice(self.id, c_int::from(pause)) };
    }
}

impl Drop for RawDevice {
    fn drop(&mut self) {
        // SAFETY: open device id; SDL_CloseAudioDevice joins the audio
        // thread, so no callback can still reference `pull` afterwards.
        unsafe {
            sys::SDL_CloseAudioDevice(self.id);
            drop(Box::from_raw(self.pull));
        }
    }
}

/// Ring capacity in frames: four blocks of the LARGER of the requested
/// block and the block SDL obtained. Under `SDL_AUDIO_ALLOW_SAMPLES_CHANGE`
/// the driver may grant a bigger period (ALSA hardware periods on
/// handhelds); a ring sized from the request alone would underrun every
/// callback. A zero obtained block is refused.
pub fn ring_capacity_frames(
    requested_block: u32,
    obtained_samples: u16,
) -> Result<usize, RealAudioError> {
    if obtained_samples == 0 {
        return Err(RealAudioError::Sdl(
            "SDL obtained a 0-frame block; the callback size is unusable".to_owned(),
        ));
    }
    Ok(requested_block.max(u32::from(obtained_samples)) as usize * 4)
}

/// The producer-side bridge handle. All lifecycle transitions live here;
/// the SDL device is owned exclusively and closed exactly once.
pub struct AudioBridge {
    shared: Arc<Shared>,
    spec_report: SpecReport,
    device: Option<RawDevice>,
    block_frames: u32,
    closed: bool,
}

impl AudioBridge {
    /// Open the device through the policy retry ladder. The SDL audio
    /// subsystem is initialized lazily here so the window path and the audio
    /// path stay independent.
    pub fn open(
        rate: i32,
        channels: u8,
        block_frames: u32,
        open_attempts: u32,
        retry_delay_ms: u32,
    ) -> Result<AudioBridge, RealAudioError> {
        Self::open_with_policy(
            rate,
            channels,
            block_frames,
            super::audio::OutputPolicy {
                default_attempts: open_attempts.clamp(1, 100),
                named_attempts: 1,
                retry_ms: retry_delay_ms.min(10000),
                enumerate: false,
                prefer_non_hdmi: false,
                reject_silent: false,
            },
        )
    }
    pub fn open_with_policy(
        rate: i32,
        channels: u8,
        block_frames: u32,
        policy: super::audio::OutputPolicy,
    ) -> Result<AudioBridge, RealAudioError> {
        if rate <= 0 || rate > 384_000 {
            return Err(RealAudioError::InvalidSpec(format!(
                "sample_rate {rate} out of range 1..=384000"
            )));
        }
        if channels == 0 || channels > 8 {
            return Err(RealAudioError::InvalidSpec(format!(
                "channels {channels} out of range 1..=8"
            )));
        }
        if block_frames == 0 || block_frames > MAX_BLOCK_FRAMES {
            return Err(RealAudioError::InvalidSpec(format!(
                "block_frames {block_frames} out of range 1..={MAX_BLOCK_FRAMES}"
            )));
        }
        let mut attempts = 0;
        let mut candidates = vec![None];
        let mut candidate = 0;
        // <= MAX_BLOCK_FRAMES, so the power of two still fits the u16 field.
        let sdl_samples = block_frames.next_power_of_two() as u16;
        let mut open_failures = 0u64;
        let mut last_error = String::new();
        let mut opened: Option<(RawDevice, String, sys::SDL_AudioSpec)> = None;
        // The whole subsystem-init + device-open sequence is the attempt
        // unit: a driver that cannot even initialize counts against the
        // policy ladder exactly like a failed device open.
        while candidate < candidates.len() {
            let limit = if candidate == 0 {
                policy.default_attempts
            } else {
                policy.named_attempts
            }
            .clamp(1, 100);
            for attempt in 1..=limit {
                attempts += 1;
                let try_open = || -> Result<(RawDevice, String, sys::SDL_AudioSpec), String> {
                    let audio = AudioSubsystemLease::open()?;
                    // SAFETY: the audio subsystem is up; SDL returns null or a
                    // NUL-terminated driver name, copied before any SDL call.
                    let driver = unsafe {
                        let driver = sys::SDL_GetCurrentAudioDriver();
                        if driver.is_null() {
                            "unknown".into()
                        } else {
                            std::ffi::CStr::from_ptr(driver)
                                .to_string_lossy()
                                .into_owned()
                        }
                    };
                    if policy.reject_silent && super::audio::silent_driver(&driver) {
                        return Err(format!("audio policy rejects silent driver {driver}"));
                    }
                    let pull = Box::into_raw(Box::new(Pull {
                        shared: OnceLock::new(),
                    }));
                    let desired = sys::SDL_AudioSpec {
                        freq: rate,
                        format: sys::AUDIO_F32LSB as sys::SDL_AudioFormat,
                        channels,
                        silence: 0,
                        samples: sdl_samples,
                        padding: 0,
                        size: 0,
                        callback: Some(audio_callback),
                        userdata: pull.cast(),
                    };
                    let mut obtained = std::mem::MaybeUninit::<sys::SDL_AudioSpec>::zeroed();
                    // SAFETY: valid desired/obtained specs; the device opens
                    // paused, so the callback cannot run before `resume`.
                    let id = unsafe {
                        sys::SDL_OpenAudioDevice(
                            candidates[candidate]
                                .as_ref()
                                .map_or(std::ptr::null(), |name: &std::ffi::CString| name.as_ptr()),
                            0,
                            &desired,
                            obtained.as_mut_ptr(),
                            ALLOWED_CHANGES as c_int,
                        )
                    };
                    if id == 0 {
                        // SAFETY: SDL never saw a live device for this `Pull`.
                        drop(unsafe { Box::from_raw(pull) });
                        return Err(sdl2::get_error());
                    }
                    let device = RawDevice {
                        id,
                        pull,
                        _audio: audio,
                    };
                    // SAFETY: SDL filled `obtained` on success.
                    let obtained = unsafe { obtained.assume_init() };
                    // Format/channels are not in `allowed_changes`; the callback
                    // relies on them, so a mismatch is refused, never trusted.
                    if u32::from(obtained.format) != sys::AUDIO_F32LSB
                        || obtained.channels != channels
                    {
                        return Err(format!(
                            "SDL changed a fixed field: format {} channels {}",
                            format_name(obtained.format),
                            obtained.channels
                        ));
                    }
                    Ok((device, driver, obtained))
                };
                match try_open() {
                    Ok(result) => {
                        opened = Some(result);
                        break;
                    }
                    Err(err) => {
                        open_failures += 1;
                        last_error = err;
                        if attempt < limit && policy.retry_ms > 0 {
                            std::thread::sleep(Duration::from_millis(
                                policy.retry_ms.min(10000) as u64
                            ));
                        }
                    }
                }
            }
            if opened.is_some() {
                break;
            }
            if candidate == 0 && policy.enumerate {
                if let Ok(_audio) = AudioSubsystemLease::open() {
                    // SAFETY: initialized SDL audio subsystem, bounded indexes; names are copied
                    // before dropping the lease and before any subsequent device-open call.
                    let mut names = unsafe {
                        (0..sys::SDL_GetNumAudioDevices(0).clamp(0, 128))
                            .filter_map(|i| {
                                let name = sys::SDL_GetAudioDeviceName(i, 0);
                                if name.is_null() {
                                    None
                                } else {
                                    Some(std::ffi::CStr::from_ptr(name).to_owned())
                                }
                            })
                            .filter(|name| !name.as_bytes().is_empty())
                            .collect::<Vec<_>>()
                    };
                    super::audio::order_output_names(&mut names, policy.prefer_non_hdmi);
                    candidates.extend(names.into_iter().map(Some));
                }
            }
            candidate += 1;
        }
        let (device, driver, obtained) = opened.ok_or_else(|| RealAudioError::OpenFailed {
            attempts,
            last_error,
        })?;
        // The device is still paused: size the ring from what SDL will
        // actually ask for per callback, bind it, only then unpause.
        let capacity = ring_capacity_frames(block_frames, obtained.samples)?;
        let shared = Arc::new(Shared {
            ring: FrameRing::new(capacity, channels as usize),
            stats: AudioStats::default(),
        });
        shared
            .stats
            .open_failures
            .store(open_failures, Ordering::Release);
        device.bind(Arc::clone(&shared));
        device.pause(false);
        let spec_report = SpecReport {
            requested_rate: rate,
            requested_channels: channels,
            requested_block_frames: block_frames,
            app_format: APP_FORMAT.to_owned(),
            allowed_changes: ALLOWED_CHANGES,
            obtained_rate: obtained.freq,
            obtained_channels: obtained.channels,
            obtained_block_frames: u32::from(obtained.samples),
            obtained_format: format_name(obtained.format),
            driver,
        };
        Ok(AudioBridge {
            shared,
            spec_report,
            device: Some(device),
            block_frames,
            closed: false,
        })
    }

    /// Producer side: submit whole frames. Bounded — the excess is dropped
    /// and counted, never queued unboundedly. Only ONE thread may submit at
    /// a time (single ring producer).
    pub fn submit(&self, samples: &[f32]) -> Result<usize, RealAudioError> {
        if self.closed {
            return Err(RealAudioError::Closed);
        }
        if samples.is_empty() {
            return Ok(0);
        }
        let channels = self.spec_report.requested_channels as usize;
        if samples.len() % channels != 0 {
            return Err(RealAudioError::InvalidSubmit(format!(
                "{} samples is not a whole number of frames at channels={channels}",
                samples.len()
            )));
        }
        let started = Instant::now();
        let accepted = self.shared.ring.push(samples);
        let stats = &self.shared.stats;
        stats
            .submit_ns
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::AcqRel);
        stats.submit_calls.fetch_add(1, Ordering::AcqRel);
        stats
            .submitted_frames
            .fetch_add(accepted as u64, Ordering::AcqRel);
        // Bounded-queue guarantee: the never-queued excess is counted here.
        let dropped = samples.len() / channels - accepted;
        if dropped > 0 {
            self.shared
                .stats
                .dropped_frames
                .fetch_add(dropped as u64, Ordering::AcqRel);
        }
        Ok(accepted)
    }

    /// Wait on the producer thread for enough whole-frame queue capacity.
    /// Timeout leaves the queue untouched; lifecycle operations remain serialized
    /// with the producer, just like `submit`.
    pub fn wait_for_space(&self, frames: usize, timeout: Duration) -> Result<bool, RealAudioError> {
        if self.closed {
            return Err(RealAudioError::Closed);
        }
        if frames > self.capacity_frames() {
            return Err(RealAudioError::InvalidSubmit("wait exceeds ring capacity".into()));
        }
        let deadline = Instant::now().checked_add(timeout)
            .ok_or_else(|| RealAudioError::InvalidSubmit("wait deadline out of range".into()))?;
        let plan = u32::try_from(self.spec_report.obtained_rate).ok()
            .and_then(|rate| AudioBackpressure::new(rate, self.spec_report.obtained_block_frames))
            .ok_or_else(|| RealAudioError::InvalidSubmit("invalid obtained audio timing".into()))?;
        loop {
            let free = self.capacity_frames().saturating_sub(self.queued_frames());
            if free >= frames { return Ok(true); }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            std::thread::sleep(plan.delay(frames - free, remaining));
        }
    }

    pub fn pause(&mut self) -> Result<(), RealAudioError> {
        let device = self.device.as_ref().ok_or(RealAudioError::Closed)?;
        device.pause(true);
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), RealAudioError> {
        let device = self.device.as_ref().ok_or(RealAudioError::Closed)?;
        device.pause(false);
        Ok(())
    }

    /// Close the device. SDL joins the audio thread before this returns, so
    /// afterwards NO callback can be in flight (the pending-callback
    /// ownership contract); stats stay readable through `stats`.
    pub fn close(&mut self) -> Result<(), RealAudioError> {
        let device = self.device.take().ok_or(RealAudioError::Closed)?;
        // Pause first, then drop: RawDevice's Drop runs SDL_CloseAudioDevice,
        // which JOINS the audio thread — after this returns, no callback can
        // be in flight and the callback object is released with the device
        // (the pending-callback ownership contract; stats live on in the
        // Arc).
        device.pause(true);
        drop(device);
        self.closed = true;
        Ok(())
    }

    /// Format change = close + reopen with the new spec (documented
    /// lifecycle; no in-flight format switch is attempted).
    pub fn reopen(
        mut self,
        rate: i32,
        channels: u8,
        block_frames: u32,
        open_attempts: u32,
        retry_delay_ms: u32,
    ) -> Result<AudioBridge, RealAudioError> {
        if let Err(error) = self.close() {
            return Err(error);
        }
        Self::open(rate, channels, block_frames, open_attempts, retry_delay_ms)
    }

    pub fn stats(&self) -> &AudioStats {
        &self.shared.stats
    }

    pub fn spec_report(&self) -> &SpecReport {
        &self.spec_report
    }

    pub fn queued_frames(&self) -> usize {
        self.shared.ring.queued_frames()
    }

    pub fn capacity_frames(&self) -> usize {
        self.shared.ring.capacity_frames()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn block_frames(&self) -> u32 {
        self.block_frames
    }

    /// Read side of this bridge for other threads (see [`AudioObserver`]).
    pub fn observer(&self) -> AudioObserver {
        AudioObserver {
            shared: Arc::clone(&self.shared),
            spec_report: self.spec_report.clone(),
        }
    }
}

/// Lock-free view of a bridge's counters, queue depth and spec report:
/// readable from any thread concurrently with `submit` and the callback, and
/// still after `close`. It never pushes or pulls frames.
pub struct AudioObserver {
    shared: Arc<Shared>,
    spec_report: SpecReport,
}

impl AudioObserver {
    pub fn stats(&self) -> &AudioStats {
        &self.shared.stats
    }

    pub fn spec_report(&self) -> &SpecReport {
        &self.spec_report
    }

    pub fn queued_frames(&self) -> usize {
        self.shared.ring.queued_frames()
    }

    pub fn capacity_frames(&self) -> usize {
        self.shared.ring.capacity_frames()
    }
}

#[cfg(test)]
#[path = "../tests/unit/real_audio.rs"]
mod tests;
