//! Ticket 92: the C-API level of the audio bridge acceptance. A synthetic
//! producer dlopens the DEPLOYED capability cdylib (the same file the
//! loader deploys) and drives the bridge exclusively through the exported
//! C ABI — the "formal dynamic interface" of the ticket, the same topology the guest
//! side will use (no static linking, no Rust-type sharing).
//!
//! Assembly (crates/AGENTS.md dynamic-load rule): the cdylib is located the
//! way cargo does — CARGO_TARGET_DIR (relative to the crates root) or
//! `crates/target` — and rebuilt with this run's profile and the `sdl`
//! feature before dlopen, so one `cargo test` works from an empty target.

#![cfg(feature = "sdl")]

use libloading::{Library, Symbol};
use platform_sdl_runtime::c_api::symbols;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn crates_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <crates>/capability/linux/sdl_runtime.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates root above capability/linux/sdl_runtime")
        .to_owned()
}

fn workspace_target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                crates_root().join(dir)
            }
        }
        None => crates_root().join("target"),
    }
}

/// Build the `sdl` cdylib with this run's cargo, target dir and profile,
/// then return its path; a failure is a test failure, never a skip.
fn build_cdylib() -> PathBuf {
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    let path = workspace_target_dir()
        .join(profile)
        .join("libplatform_sdl_runtime.so");
    let mut command = Command::new(cargo());
    command.args([
        "build",
        "--locked",
        "-p",
        "rudroid-linux-sdl-runtime",
        "--features",
        "rudroid-linux-sdl-runtime/sdl",
    ]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command
        .current_dir(crates_root())
        .status()
        .expect("spawn cargo for the platform_sdl_runtime cdylib");
    assert!(status.success(), "building the sdl cdylib failed");
    assert!(
        path.exists(),
        "cargo build succeeded but no cdylib at {}; the test resolved a \
         different CARGO_TARGET_DIR than the build",
        path.display()
    );
    path
}

struct Entry {
    lib: Library,
}

impl Entry {
    fn open(path: &str) -> Entry {
        // SAFETY: the path points at the deployed cdylib; the handle lives
        // as long as this struct and all Symbols derive from it.
        let lib = unsafe { Library::new(path) }.expect("dlopen deployed platform_sdl_runtime");
        Entry { lib }
    }

    fn fn_ptr(&self, name: &[u8]) -> *mut c_void {
        // SAFETY: symbol lookup on the live handle.
        unsafe {
            let symbol: Symbol<*mut c_void> = self.lib.get(name).expect("export exists");
            *symbol
        }
    }
}

/// One second of 440 Hz stereo sine at 48000 Hz.
fn sine_block(rate: i32, from_frame: usize, frames: usize, freq: f64) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let value = (2.0
                * std::f64::consts::PI
                * freq
                * (from_frame + n) as f64
                / rate as f64) as f32;
            [value, value]
        })
        .collect()
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AudioStats {
    submitted_frames: u64,
    dropped_frames: u64,
    pulled_frames: u64,
    underruns: u64,
    callbacks: u64,
    max_queue_frames: u64,
    open_failures: u64,
    queued_frames: u64,
    capacity_frames: u64,
    submit_ns: u64,
    submit_calls: u64,
    callback_ns: u64,
}

#[repr(C)]
struct AudioSpec {
    requested_rate: i32,
    requested_channels: u8,
    requested_block_frames: u32,
    obtained_rate: i32,
    obtained_channels: u8,
    obtained_block_frames: u32,
    driver: *const std::os::raw::c_char,
    obtained_format: *const std::os::raw::c_char,
    app_format: *const std::os::raw::c_char,
    allowed_changes: u32,
}

unsafe fn read_cstr(ptr: *const std::os::raw::c_char) -> String {
    // SAFETY: process-lifetime leaked C string from the capability.
    unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

/// The closed loop through the C ABI: open → submit sine → observe → close
/// → stats after close → release. Output goes to the SDL dummy virtual
/// device (explicitly labeled; audibility not claimed).
#[test]
fn c_api_producer_drives_the_deployed_bridge() {
    let so_path = build_cdylib().to_str().expect("utf8 cdylib path").to_owned();
    std::env::set_var("SDL_AUDIODRIVER", "dummy");
    let entry = Entry::open(&so_path);
    // SAFETY: entry points are extern "C" fns in the cdylib.
    let (audio_open, audio_submit, audio_pause_resume, audio_close_device, audio_release, audio_stats, audio_spec) = unsafe { (
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(i32, u8, u32, u32, u32) -> *mut c_void>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_OPEN.as_bytes()),
        ),
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, *const f32, usize) -> i64>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_SUBMIT.as_bytes()),
        ),
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, i32) -> i32>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_PAUSE_RESUME.as_bytes()),
        ),
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void) -> i32>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_CLOSE.as_bytes()),
        ),
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void) -> i32>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_RELEASE.as_bytes()),
        ),
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const c_void, *mut AudioStats) -> i32>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_STATS.as_bytes()),
        ),
        std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const c_void, *mut AudioSpec) -> i32>(
            entry.fn_ptr(symbols::AUDIO_DEVICE_SPEC.as_bytes()),
        ),
    ) };

    // SAFETY: C ABI contract as declared above.
    let bridge = unsafe { audio_open(48_000, 2, 1024, 4, 50) };
    assert!(!bridge.is_null(), "device opens through the C ABI");

    let mut spec = AudioSpec {
        requested_rate: 0,
        requested_channels: 0,
        requested_block_frames: 0,
        obtained_rate: 0,
        obtained_channels: 0,
        obtained_block_frames: 0,
        driver: std::ptr::null(),
        obtained_format: std::ptr::null(),
        app_format: std::ptr::null(),
        allowed_changes: 0,
    };
    // SAFETY: valid bridge + out pointer.
    unsafe { audio_spec(bridge, &mut spec) };
    assert_eq!(spec.requested_rate, 48_000);
    assert!(spec.obtained_rate > 0, "obtained spec exported");
    let driver = unsafe { read_cstr(spec.driver) };
    assert_eq!(driver, "dummy", "explicitly labeled virtual device");
    assert_eq!(unsafe { read_cstr(spec.app_format) }, "F32LSB");
    assert_eq!(spec.allowed_changes, 8, "reference SDL_AUDIO_ALLOW_SAMPLES_CHANGE");

    // Produce 0.5 s of audio in 10 ms blocks; bounded resubmit loop.
    const RATE: i32 = 48_000;
    let total_frames = RATE as usize / 2;
    let mut offset = 0usize;
    let mut last_stats = AudioStats::default();
    while offset < total_frames {
        let block_frames = 480.min(total_frames - offset);
        let block = sine_block(RATE, offset, block_frames, 440.0);
        let accepted = unsafe { audio_submit(bridge, block.as_ptr(), block_frames) };
        assert!(accepted >= 0, "submit through the C ABI");
        offset += accepted as usize;
        if accepted == 0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // SAFETY: valid bridge + out pointer.
        unsafe { audio_stats(bridge, &mut last_stats) };
        assert!(
            last_stats.queued_frames <= last_stats.capacity_frames,
            "the queue never exceeds its bound through the C ABI"
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while last_stats.pulled_frames < total_frames as u64 {
        assert!(std::time::Instant::now() < deadline, "device never drained");
        std::thread::sleep(std::time::Duration::from_millis(2));
        // SAFETY: valid bridge + out pointer.
        unsafe { audio_stats(bridge, &mut last_stats) };
    }

    // Pause/resume through the C ABI.
    // SAFETY: valid bridge.
    unsafe { assert_eq!(audio_pause_resume(bridge, 1), 0) };
    // SAFETY: valid bridge.
    unsafe { assert_eq!(audio_pause_resume(bridge, 0), 0) };

    // Close: after this, callbacks cannot be in flight; submit is refused.
    // SAFETY: valid bridge.
    unsafe { assert_eq!(audio_close_device(bridge), 0) };
    let rc = unsafe { audio_submit(bridge, sine_block(RATE, 0, 10, 440.0).as_ptr(), 10) };
    assert_eq!(rc, -1, "a closed bridge refuses submissions");
    let mut after_close = AudioStats::default();
    // SAFETY: stats stay readable after close.
    unsafe { audio_stats(bridge, &mut after_close) };
    assert!(after_close.pulled_frames >= total_frames as u64);
    assert!(after_close.submit_calls > 0 && after_close.submit_ns > 0, "producer cost recorded");
    assert!(after_close.callback_ns > 0, "callback write cost recorded");
    eprintln!(
        "T92 C-API OBSERVATION driver={} submitted={} pulled={} underruns={} dropped={} callbacks={} max_queue={} submit_ns={} submit_calls={} callback_ns={}",
        driver,
        after_close.submitted_frames,
        after_close.pulled_frames,
        after_close.underruns,
        after_close.dropped_frames,
        after_close.callbacks,
        after_close.max_queue_frames,
        after_close.submit_ns,
        after_close.submit_calls,
        after_close.callback_ns
    );

    // SAFETY: release the bridge handle once, at the end.
    unsafe { assert_eq!(audio_release(bridge), 0) };

    // Released, foreign and null handles all take the null-handle path.
    let mut stale = AudioStats::default();
    // SAFETY: C ABI contract; handles are looked up, never dereferenced.
    unsafe {
        assert_eq!(audio_stats(bridge, &mut stale), -1, "a released handle is unknown");
        assert_eq!(audio_release(bridge), -1, "a second release is rejected");
        assert_eq!(audio_pause_resume(0xdead as *mut c_void, 1), -1, "a foreign handle is unknown");
        assert_eq!(audio_close_device(std::ptr::null_mut()), -1);
        assert_eq!(audio_stats(std::ptr::null(), &mut stale), -1);
    }
}
