//! INPUT: C callers, opaque SDL handles and registry services
//! OUTPUT: checked window, input and audio entry points for the loader
//! POS: SDL capability ABI adapters
// ------------------------------------------------------------- ticket 92
// Real audio-device bridge C API. Handles are opaque serial numbers
// resolved through a `HandleTable`; an unknown, released or null handle
// takes the same error path. Without the `sdl` feature every entry
// exists and reports the explicit diagnostic (the missing-symbol dlsym
// failure is never the signal).
//
// Threading contract (the bridge is an SPSC ring):
// - `submit`: ONE producer thread at a time per handle.
// - `stats` / `spec`: any thread, any time while the handle is live,
//   concurrently with `submit` and with the SDL audio thread.
// - `pause_resume` / `close` / `release`: never concurrently with
//   `submit` or with each other; `release` ends the handle for every
//   thread.
// `submit`, `pause_resume` and `close` serialize on a per-handle lock
// that the contract keeps uncontended; `stats` / `spec` read a lock-free
// observer. The SDL audio callback never takes either.

/// POD stats snapshot (all u64; read via rudroid_sdl_audio_device_stats).
#[repr(C)]
pub struct CRudroidAudioStats {
    pub submitted_frames: u64,
    pub dropped_frames: u64,
    pub pulled_frames: u64,
    pub underruns: u64,
    pub callbacks: u64,
    pub max_queue_frames: u64,
    pub open_failures: u64,
    pub queued_frames: u64,
    pub capacity_frames: u64,
    /// Producer cost (ns) and calls of submit; callback write cost (ns).
    pub submit_ns: u64,
    pub submit_calls: u64,
    pub callback_ns: u64,
}

/// POD spec report: the application request and the spec
/// `SDL_OpenAudioDevice` reported under `allowed_changes`, plus the
/// driver name pointer.
#[repr(C)]
pub struct CRudroidAudioSpec {
    pub requested_rate: i32,
    pub requested_channels: u8,
    pub requested_block_frames: u32,
    pub obtained_rate: i32,
    pub obtained_channels: u8,
    pub obtained_block_frames: u32,
    /// NUL-terminated, process-lifetime driver name (e.g. "dummy",
    /// "disk", "alsa") — the actual backend, NOSOUND-style stand-ins
    /// included.
    pub driver: *const c_char,
    /// NUL-terminated, process-lifetime obtained SDL format name.
    pub obtained_format: *const c_char,
    /// NUL-terminated, process-lifetime format the callback is fed.
    pub app_format: *const c_char,
    /// The `allowed_changes` mask the device was opened with.
    pub allowed_changes: u32,
}

/// One open bridge: its lifecycle behind the per-handle lock, and the
/// lock-free observer (stats stay readable after close).
#[cfg(feature = "sdl")]
struct AudioDevice {
    bridge: Mutex<super::real_audio::AudioBridge>,
    observer: super::real_audio::AudioObserver,
}

#[cfg(feature = "sdl")]
static AUDIO_DEVICES: HandleTable<AudioDevice> = HandleTable::new();

#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_open(
    rate: i32,
    channels: u8,
    block_frames: u32,
    open_attempts: u32,
    retry_delay_ms: u32,
) -> *mut c_void {
    match super::real_audio::AudioBridge::open(
        rate,
        channels,
        block_frames,
        open_attempts,
        retry_delay_ms,
    ) {
        Ok(bridge) => AUDIO_DEVICES.insert(AudioDevice {
            observer: bridge.observer(),
            bridge: Mutex::new(bridge),
        }),
        Err(err) => {
            set_last_error(&err.to_string());
            std::ptr::null_mut()
        }
    }
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_open(
    _rate: i32,
    _channels: u8,
    _block_frames: u32,
    _open_attempts: u32,
    _retry_delay_ms: u32,
) -> *mut c_void {
    set_last_error(
        "platform_sdl_runtime was built without the `sdl` feature; no real audio device can be opened (rebuild with --features rudroid-linux-sdl-runtime/sdl)",
    );
    std::ptr::null_mut()
}

/// # Safety
/// `samples` valid for `frames * channels` f32 values (or null → error);
/// `bridge` from audio_open (anything else → error).
#[cfg(feature = "sdl")]
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_audio_device_submit(
    bridge: *mut c_void,
    samples: *const f32,
    frames: usize,
) -> i64 {
    let device = if samples.is_null() {
        None
    } else {
        AUDIO_DEVICES.get(bridge)
    };
    let Some(device) = device else {
        set_last_error("audio_submit requires a bridge and a samples buffer");
        return -1;
    };
    let channels = device.observer.spec_report().requested_channels as usize;
    let Some(count) = frames
        .checked_mul(channels)
        .filter(|n| *n <= isize::MAX as usize / size_of::<f32>())
    else {
        set_last_error("audio_submit sample count exceeds addressable memory");
        return -1;
    };
    let slice: &[f32] = if count == 0 {
        &[]
    } else {
        // SAFETY: caller contract — frames * channels readable f32s.
        unsafe { std::slice::from_raw_parts(samples, count) }
    };
    let result = lock(&device.bridge).submit(slice);
    match result {
        Ok(accepted) => accepted as i64,
        Err(err) => {
            set_last_error(&err.to_string());
            -1
        }
    }
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_audio_device_submit(
    _bridge: *mut c_void,
    _samples: *const f32,
    _frames: usize,
) -> i64 {
    set_last_error("platform_sdl_runtime has no `sdl` feature: audio bridge unavailable");
    -1
}

#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_pause_resume(
    bridge: *mut c_void,
    pause: c_int,
) -> c_int {
    let Some(device) = AUDIO_DEVICES.get(bridge) else {
        set_last_error("audio_pause_resume requires a bridge");
        return -1;
    };
    let outcome = {
        let mut bridge = lock(&device.bridge);
        if pause != 0 {
            bridge.pause()
        } else {
            bridge.resume()
        }
    };
    match outcome {
        Ok(()) => 0,
        Err(err) => {
            set_last_error(&err.to_string());
            -1
        }
    }
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_pause_resume(
    _bridge: *mut c_void,
    _pause: c_int,
) -> c_int {
    set_last_error("platform_sdl_runtime has no `sdl` feature: audio bridge unavailable");
    -1
}

/// Close the device. After this returns, no callback is in flight
/// (SDL joined the audio thread); the handle stays valid for stats
/// reads and must be released with audio_close_final.
#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_close(bridge: *mut c_void) -> c_int {
    let Some(device) = AUDIO_DEVICES.get(bridge) else {
        set_last_error("audio_close_device requires a bridge");
        return -1;
    };
    let outcome = lock(&device.bridge).close();
    match outcome {
        Ok(()) => 0,
        Err(err) => {
            set_last_error(&err.to_string());
            -1
        }
    }
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_close(_bridge: *mut c_void) -> c_int {
    set_last_error("platform_sdl_runtime has no `sdl` feature: audio bridge unavailable");
    -1
}

/// Release the bridge handle (close first for the no-callback
/// guarantee; closing twice is an error, dropping an open bridge closes
/// the device via Drop). The handle is dead for every thread afterwards.
#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_release(bridge: *mut c_void) -> c_int {
    match AUDIO_DEVICES.remove(bridge) {
        Some(device) => {
            drop(device);
            0
        }
        None => {
            set_last_error("audio_close_final requires a bridge");
            -1
        }
    }
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_release(_bridge: *mut c_void) -> c_int {
    set_last_error("platform_sdl_runtime has no `sdl` feature: audio bridge unavailable");
    -1
}

/// `bridge` live (closed ok — stats stay readable).
#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_stats(
    bridge: *const c_void,
    out: Option<&mut CRudroidAudioStats>,
) -> c_int {
    let (Some(device), Some(out)) = (AUDIO_DEVICES.get(bridge.cast_mut()), out) else {
        set_last_error("audio_stats requires a bridge and an out pointer");
        return -1;
    };
    let observer = &device.observer;
    let stats = observer.stats();
    let load = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Acquire);
    *out = CRudroidAudioStats {
        submitted_frames: load(&stats.submitted_frames),
        dropped_frames: load(&stats.dropped_frames),
        pulled_frames: load(&stats.pulled_frames),
        underruns: load(&stats.underruns),
        callbacks: load(&stats.callbacks),
        max_queue_frames: load(&stats.max_queue_frames),
        open_failures: load(&stats.open_failures),
        queued_frames: observer.queued_frames() as u64,
        capacity_frames: observer.capacity_frames() as u64,
        submit_ns: load(&stats.submit_ns),
        submit_calls: load(&stats.submit_calls),
        callback_ns: load(&stats.callback_ns),
    };
    0
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_stats(
    _bridge: *const c_void,
    _out: Option<&mut CRudroidAudioStats>,
) -> c_int {
    set_last_error("platform_sdl_runtime has no `sdl` feature: audio bridge unavailable");
    -1
}

// The descriptor ABI promises process-lifetime pointers, including after close.
#[cfg(feature = "sdl")]
fn audio_spec_string(value: &str) -> &'static std::ffi::CStr {
    use std::{collections::HashMap, ffi::CStr, sync::OnceLock};
    static STRINGS: OnceLock<Mutex<HashMap<String, &'static CStr>>> = OnceLock::new();
    let mut strings = lock(STRINGS.get_or_init(Mutex::default));
    if let Some(&existing) = strings.get(value) {
        return existing;
    }
    let permanent = rudroid_common::descriptor::leak_cstr(value).as_c_str();
    strings.insert(value.to_owned(), permanent);
    permanent
}

#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_spec(
    bridge: *const c_void,
    out: Option<&mut CRudroidAudioSpec>,
) -> c_int {
    let (Some(device), Some(out)) = (AUDIO_DEVICES.get(bridge.cast_mut()), out) else {
        set_last_error("audio_spec requires a bridge and an out pointer");
        return -1;
    };
    let report = device.observer.spec_report();
    let driver = audio_spec_string(&report.driver);
    let format = audio_spec_string(&report.obtained_format);
    let app_format = audio_spec_string(&report.app_format);
    *out = CRudroidAudioSpec {
        requested_rate: report.requested_rate,
        requested_channels: report.requested_channels,
        requested_block_frames: report.requested_block_frames,
        obtained_rate: report.obtained_rate,
        obtained_channels: report.obtained_channels,
        obtained_block_frames: report.obtained_block_frames,
        driver: driver.as_ptr(),
        obtained_format: format.as_ptr(),
        app_format: app_format.as_ptr(),
        allowed_changes: report.allowed_changes,
    };
    0
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_device_spec(
    _bridge: *const c_void,
    _out: Option<&mut CRudroidAudioSpec>,
) -> c_int {
    set_last_error("platform_sdl_runtime has no `sdl` feature: audio bridge unavailable");
    -1
}

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Mutex, MutexGuard};

use rudroid_common::handles::HandleTable;
use rudroid_common::registry::HostStr;

use super::audio::{AudioError, AudioPolicyParams, AudioPump};

pub use rudroid_common::sdl_abi::{symbols, CRudroidSdlReport};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(message: &str) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(message.to_owned()).ok();
    });
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// # Safety
/// `buffer`/`capacity` follow the two-phase elf_load last-error contract.
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_last_error(buffer: *mut c_char, capacity: usize) -> usize {
    let message = LAST_ERROR.with(|slot| slot.borrow().clone());
    let Some(message) = message else {
        return 0;
    };
    let bytes = message.as_bytes_with_nul();
    if buffer.is_null() || capacity == 0 {
        return bytes.len();
    }
    // SAFETY: the caller guarantees `capacity` writable bytes.
    unsafe {
        let usable = capacity.min(bytes.len()) - 1;
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast(), buffer, usable);
        *buffer.add(usable) = 0;
    }
    bytes.len()
}

// ---------------------------------------------------------------- session

/// Copies validated registry policy before any guest producer opens audio.
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_policy_configure(
    policy: Option<&rudroid_common::registry::RudroidAudioPolicy>,
) -> c_int {
    match super::audio::configure_output_policy(policy) {
        Ok(()) => 0,
        Err(error) => {
            set_last_error(&error);
            -1
        }
    }
}

/// Session reports by handle, readable from any thread. Their strings
/// are the live session's own shared allocations.
#[cfg(feature = "sdl")]
static SESSIONS: HandleTable<super::window::SdlSessionReport> = HandleTable::new();

#[cfg(feature = "sdl")]
thread_local! {
    /// The `!Send` sessions this thread created, keyed by handle.
    /// `ManuallyDrop`: a session never destroyed stays pinned, even past
    /// thread exit.
    static LIVE_SESSIONS: RefCell<std::collections::BTreeMap<usize, std::mem::ManuallyDrop<super::window::SdlSession>>> =
        const { RefCell::new(std::collections::BTreeMap::new()) };
}

/// Configure guest render size on its creating thread before activation.
/// The token is looked up, never dereferenced; output dimensions remain SDL-owned.
#[no_mangle]
pub extern "C" fn rudroid_sdl_render_configure(
    session: *mut c_void,
    width: i32,
    height: i32,
    sharp: i32,
) -> c_int {
    #[cfg(all(feature = "sdl", target_os = "linux"))]
    {
        let result = LIVE_SESSIONS.with(|sessions| {
            sessions.try_borrow_mut().ok().and_then(|mut sessions| {
                sessions
                    .get_mut(&(session as usize))
                    .map(|live| live.configure_render(width, height, sharp))
            })
        });
        match result {
            Some(Ok(())) => 0,
            Some(Err(error)) => {
                set_last_error(&error);
                -1
            }
            None => {
                set_last_error(
                    "render configuration requires a live session on its creating thread",
                );
                -1
            }
        }
    }
    #[cfg(not(all(feature = "sdl", target_os = "linux")))]
    {
        let _ = (session, width, height, sharp);
        set_last_error("render configuration requires Linux and sdl");
        -1
    }
}

/// Activate the existing session as the process's guest graphics service.
/// # Safety
/// Call on the session's creating thread before guest execution, with
/// exclusive access to the session (no concurrent activation/report/
/// destroy). The callback must remain valid and have no unmet
/// preconditions after a successful present. It and the capability stay
/// process-live; destroy only after all guest threads have stopped using
/// the service and released their contexts.
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_guest_graphics_activate(
    session: *mut c_void,
    presented: Option<extern "C" fn()>,
) -> c_int {
    #[cfg(all(feature = "sdl", target_os = "linux"))]
    {
        let activated = LIVE_SESSIONS.with(|sessions| {
            sessions
                .borrow_mut()
                .get_mut(&(session as usize))
                .map(|live| {
                    // SAFETY: this thread's live session, exclusively
                    // borrowed; callback/guest lifetime follow the C
                    // contract above.
                    unsafe { live.activate_guest_graphics(presented) }
                })
        });
        let result = match activated {
            Some(result) => result,
            None if SESSIONS.get(session).is_some() => {
                Err("guest graphics activation must run on the SDL creating thread".to_owned())
            }
            None => Err("guest graphics requires a live SDL session".to_owned()),
        };
        match result {
            Ok(()) => 0,
            Err(error) => {
                set_last_error(&error);
                -1
            }
        }
    }
    #[cfg(not(all(feature = "sdl", target_os = "linux")))]
    {
        let _ = (session, presented);
        set_last_error("guest graphics requires Linux and the sdl feature");
        -1
    }
}

#[cfg(not(all(feature = "sdl", target_os = "linux")))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_guest_symbol(_name: *const c_char, _user: *mut c_void) -> usize {
    0
}

/// Sizes/versions must come from the Host Config; the loader clamps the
/// versions to 1..=3 major / 0..=9 minor before calling.
#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_create(
    width: i64,
    height: i64,
    major: i32,
    minor: i32,
) -> *mut c_void {
    create_session_handle(width, height, major, minor, false)
}

/// Optional Wayland compositor presentation. Width/height are exact native buffer dimensions.
#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_create_native_buffer(
    width: i64,
    height: i64,
    major: i32,
    minor: i32,
) -> *mut c_void {
    create_session_handle(width, height, major, minor, true)
}

#[cfg(feature = "sdl")]
fn create_session_handle(
    width: i64,
    height: i64,
    major: i32,
    minor: i32,
    native_buffer: bool,
) -> *mut c_void {
    if !(1..=3).contains(&major) || !(0..=9).contains(&minor) {
        set_last_error(&format!(
            "invalid requested GLES version {major}.{minor} (expected 1..=3 \
             major, 0..=9 minor)"
        ));
        return std::ptr::null_mut();
    }
    let create = if native_buffer {
        super::window::create_session_native_buffer
    } else {
        super::window::create_session_sized
    };
    match create(width, height, major as u8, minor as u8) {
        Ok((session, report)) => {
            let handle = SESSIONS.insert(report);
            LIVE_SESSIONS.with(|sessions| {
                sessions
                    .borrow_mut()
                    .insert(handle as usize, std::mem::ManuallyDrop::new(session))
            });
            handle
        }
        Err(err) => {
            set_last_error(&err.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Compiled runtime support only. No SDL initialization, device access,
/// phases or guest loading; actual driver/window checks happen at startup.
#[no_mangle]
pub extern "C" fn rudroid_sdl_graphics_features() -> u64 {
    use rudroid_common::graphics_features::*;
    if !cfg!(feature = "sdl") {
        return 0;
    }
    let linux_gles = if cfg!(all(feature = "sdl", target_os = "linux")) {
        GLES_SCALE | GLES_SHARP | GLES_SHADER | GLES_TEXTURE
    } else {
        0
    };
    let linux_vulkan = if cfg!(all(feature = "vulkan", target_os = "linux")) {
        VULKAN | VULKAN_SCALE | VULKAN_LINEAR
    } else {
        0
    };
    SDL2 | GLES | linux_gles | linux_vulkan
}

/// Explicit optional Vulkan backend. The version arguments are reserved;
/// the report contains the real physical device's API version.
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_create_vulkan(
    width: i64,
    height: i64,
    _major: i32,
    _minor: i32,
) -> *mut c_void {
    #[cfg(all(feature = "vulkan", target_os = "linux"))]
    {
        match super::window::create_session_vulkan(width, height) {
            Ok((session, report)) => {
                let handle = SESSIONS.insert(report);
                LIVE_SESSIONS.with(|sessions| {
                    sessions
                        .borrow_mut()
                        .insert(handle as usize, std::mem::ManuallyDrop::new(session));
                });
                handle
            }
            Err(error) => {
                set_last_error(&error.to_string());
                std::ptr::null_mut()
            }
        }
    }
    #[cfg(not(all(feature = "vulkan", target_os = "linux")))]
    {
        let _ = (width, height);
        set_last_error("Vulkan requires Linux and a complete --vulkan candidate kit");
        std::ptr::null_mut()
    }
}

/// Feature-absent build: the same symbol exists and reports the explicit
/// diagnostic instead of a missing-symbol dlsym failure.
#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_create(
    _width: i64,
    _height: i64,
    _major: i32,
    _minor: i32,
) -> *mut c_void {
    set_last_error(
        "platform_sdl_runtime was built without the `sdl` feature; no window or \
         GLES session can be created (rebuild the kit with \
         --features rudroid-linux-sdl-runtime/sdl)",
    );
    std::ptr::null_mut()
}

#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_create_native_buffer(
    width: i64,
    height: i64,
    major: i32,
    minor: i32,
) -> *mut c_void {
    rudroid_sdl_session_create(width, height, major, minor)
}

/// `session` from session_create (anything else → error); `out` receives
/// the report. Borrowed strings must be read/copied before
/// session_destroy; querying does not pin them. Do not race querying or
/// borrowed-string reads with activation/destruction.
#[cfg(feature = "sdl")]
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_report(
    session: *const c_void,
    out: Option<&mut CRudroidSdlReport>,
) -> c_int {
    let (Some(report), Some(out)) = (SESSIONS.get(session.cast_mut()), out) else {
        set_last_error("session_report requires a session and an out pointer");
        return -1;
    };
    *out = CRudroidSdlReport {
        graphics_api: report.graphics_api,
        window_w: report.window_w,
        window_h: report.window_h,
        major: report.major,
        minor: report.minor,
        profile_mask: report.profile_mask,
        swap_count: report.swap_count,
        readback_non_background: u32::from(report.readback_non_background),
        video_driver: report.video_driver.as_ptr(),
        gl_vendor: report.gl_vendor.as_ptr(),
        gl_renderer: report.gl_renderer.as_ptr(),
        gl_version: report.gl_version.as_ptr(),
    };
    0
}

/// Feature-absent build: no session can exist, the diagnostic says why.
#[cfg(not(feature = "sdl"))]
#[no_mangle]
pub extern "C" fn rudroid_sdl_session_report(
    _session: *const c_void,
    _out: Option<&mut CRudroidSdlReport>,
) -> c_int {
    set_last_error(
        "platform_sdl_runtime was built without the `sdl` feature; no session \
         report exists",
    );
    -1
}

/// Destroy a create result exactly once on its creating thread (null,
/// unknown or another thread's session: no-op). All guest calls,
/// including cached native function uses, must have stopped and guest
/// threads released their EGL bindings (the activation contract). No
/// report query/string read may race destruction. Successful loader
/// sessions stay process-pinned; failed bring-up and tests destroy.
///
/// # Safety
/// All guest graphics calls and cached native entry uses are quiescent;
/// guest EGL bindings and report borrows have ended before destruction.
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_session_destroy(session: *mut c_void) {
    #[cfg(feature = "sdl")]
    {
        let live = LIVE_SESSIONS.with(|sessions| sessions.borrow_mut().remove(&(session as usize)));
        if let Some(live) = live {
            SESSIONS.remove(session);
            drop(std::mem::ManuallyDrop::into_inner(live));
        }
    }
    #[cfg(not(feature = "sdl"))]
    {
        let _ = session;
    }
}

// ---------------------------------------------------------------- audio

static AUDIO_PUMPS: HandleTable<Mutex<AudioPump>> = HandleTable::new();

fn audio_error_text(err: &AudioError) -> String {
    err.to_string()
}

/// Open an audio pump under the policy values (from the registered
/// `RudroidAudioPolicy`) with either a file sink (path given) or the dummy
/// in-memory sink (path null). Null on error — read last_error.
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_open(
    sample_rate: u32,
    channels: u32,
    open_attempts: u32,
    retry_delay_ms: u32,
    reject_silent_drivers: u32,
    file_path: HostStr<'_>,
) -> *mut c_void {
    if channels == 0 || channels > 8 {
        set_last_error(&format!("channels {channels} out of range 1..=8"));
        return std::ptr::null_mut();
    }
    let path = match file_path.to_cstr().map(CStr::to_str) {
        None => None,
        Some(Ok(text)) => Some(std::path::PathBuf::from(text)),
        Some(Err(err)) => {
            set_last_error(&format!("file sink path is not valid UTF-8: {err}"));
            return std::ptr::null_mut();
        }
    };
    match AudioPump::open(
        AudioPolicyParams {
            sample_rate,
            channels: channels as u8,
            open_attempts,
            retry_delay_ms,
            reject_silent_drivers,
        },
        path.as_deref(),
    ) {
        Ok(pump) => AUDIO_PUMPS.insert(Mutex::new(pump)),
        Err(err) => {
            set_last_error(&audio_error_text(&err));
            std::ptr::null_mut()
        }
    }
}

/// Push `sample_count` interleaved samples; returns FRAMES written
/// (0 on an empty buffer or error — the last-error channel says which).
///
/// # Safety
/// `samples` must be valid for `sample_count` floats when count > 0.
#[no_mangle]
pub unsafe extern "C" fn rudroid_sdl_audio_write(
    pump: *mut c_void,
    samples: *const f32,
    sample_count: usize,
) -> usize {
    let Some(pump) = AUDIO_PUMPS.get(pump) else {
        set_last_error("audio_write requires an open pump");
        return 0;
    };
    if samples.is_null() && sample_count > 0 {
        set_last_error("audio_write got a null sample buffer");
        return 0;
    }
    let slice = if sample_count == 0 {
        &[][..]
    } else {
        // SAFETY: caller guarantees `sample_count` readable floats.
        unsafe { std::slice::from_raw_parts(samples, sample_count) }
    };
    let result = lock(&pump).write(slice);
    match result {
        Ok(frames) => frames as usize,
        Err(err) => {
            set_last_error(&audio_error_text(&err));
            0
        }
    }
}

/// `pump` from audio_open (anything else → 0).
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_frames(pump: *const c_void) -> usize {
    AUDIO_PUMPS
        .get(pump.cast_mut())
        .map_or(0, |pump| lock(&pump).frames_written() as usize)
}

/// `pump` from audio_open (anything else → 0).
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_calls(pump: *const c_void) -> usize {
    AUDIO_PUMPS
        .get(pump.cast_mut())
        .map_or(0, |pump| lock(&pump).write_calls() as usize)
}

/// `pump` from audio_open (anything else → no-op).
#[no_mangle]
pub extern "C" fn rudroid_sdl_audio_destroy(pump: *mut c_void) {
    drop(AUDIO_PUMPS.remove(pump));
}
