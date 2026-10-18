//! Ticket 65 + 68 acceptance: ELF load failures keep their specific reason
//! across the C ABI, the executable backend's C entries go through the same
//! error channel, and the whole suite runs from one supported command on an
//! empty target dir.
//!
//! The C load API used to return a bare null pointer, so the loader could
//! only say "android_elf_load reported a load failure". The
//! `rudroid_elf_last_error` symbol closes that gap with a thread-local,
//! C-compatible protocol:
//!
//! - every C entry point clears the thread's error on entry and stores a
//!   UTF-8, NUL-terminated message on failure; successful calls leave no
//!   error behind;
//! - `rudroid_elf_last_error(buffer, capacity)` returns the full length
//!   INCLUDING the NUL terminator (0 = no error) and copies the message when
//!   the buffer is large enough — a two-phase (query, then fetch) pattern
//!   works, truncation never overruns;
//! - errors are per-thread, so concurrent failing loads never swap reasons.
//!
//! Assembly contract (ticket 68): `cargo test` does not produce cdylib
//! artifacts, and dlopening whatever `.so` happens to sit in the target dir
//! misloaded stale pre-ticket-65 kits (the R2 review's red). These tests
//! therefore locate the cdylib the way cargo does (CARGO_TARGET_DIR or the
//! workspace `target`, plus this run's profile) and rebuild it with the same
//! cargo before dlopen — one `cargo test ...` command works from an empty
//! target dir and can never load an artifact older than the current source.
//!
//! The Linux-only executable tests are `#[cfg(target_os = "linux")]`: on
//! other hosts they are compiled out and never report as executed. They only
//! map/parse synthetic fixture images — guest code is never executed here
//! (execution proof lives in `executable.rs` with a compiler-produced SO).

use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};

use android_elf_load::fixture::build_minimal_aarch64_so;

type LastErrorFn = unsafe extern "C" fn(*mut c_char, usize) -> usize;
type LoadBytesFn = unsafe extern "C" fn(*const u8, usize) -> *mut std::ffi::c_void;
type LoadPathFn = unsafe extern "C" fn(*const c_char) -> *mut std::ffi::c_void;
type DestroyFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type ExecLoadFn = unsafe extern "C" fn(*const c_char) -> *mut std::ffi::c_void;
type ExecSymbolFn = unsafe extern "C" fn(*const std::ffi::c_void, *const c_char) -> usize;
type ExecDestroyFn = unsafe extern "C" fn(*mut std::ffi::c_void);

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn crates_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <crates>/capability/android/elf_load: three
    // ancestor hops up to the workspace root (the unity_loader tests live one
    // level shallower — do not copy this helper blindly).
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates root above capability/android/elf_load")
        .to_owned()
}

/// The target dir cargo itself uses for this run: CARGO_TARGET_DIR when the
/// invocation set one (relative paths resolve against the crates root, the
/// workspace cargo cwd), else the workspace `target`. Never a hardcoded
/// default that could point at a foreign build.
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

fn lib_name(artifact: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{artifact}.dylib")
    } else {
        format!("lib{artifact}.so")
    }
}

fn cdylib_path() -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    workspace_target_dir()
        .join(profile)
        .join(lib_name("android_elf_load"))
}

/// Build the cdylib with the same cargo, target dir and profile as this test
/// run, then return its path. One `cargo test ...` command therefore works
/// from an empty target dir (ticket 68), and the dlopen below can never pick
/// up a stale artifact: cargo rebuilds whenever the crate sources changed,
/// and profile + target dir always match this run.
fn build_cdylib() -> PathBuf {
    let path = cdylib_path();
    let mut command = Command::new(cargo());
    command.args(["build", "--locked", "-p", "rudroid-android-elf-load"]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command
        .current_dir(crates_root())
        .status()
        .expect("spawn cargo for the elf_load cdylib");
    assert!(
        status.success(),
        "building the elf_load cdylib failed; c_error_diagnostics cannot run"
    );
    assert!(
        path.exists(),
        "cargo build succeeded but no cdylib at {}; \
         the test resolved a different CARGO_TARGET_DIR than the build",
        path.display()
    );
    path
}

/// Linux-only executable-backend entries (ticket 49); resolved and exercised
/// only on Linux (ticket 68). Off Linux the tests are compiled out — they
/// never pretend to have run.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct ExecApi {
    load: ExecLoadFn,
    symbol: ExecSymbolFn,
    destroy: ExecDestroyFn,
}

struct Api {
    last_error: LastErrorFn,
    load_bytes: LoadBytesFn,
    load_path: LoadPathFn,
    destroy: DestroyFn,
    /// `None` off Linux (see ExecApi).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    exec: Option<ExecApi>,
    // SAFETY note: the Library handle must outlive every call below.
    _library: libloading::Library,
}

/// Resolve the C API from the just-built cdylib, requiring every error-channel
/// symbol. A missing symbol is a precise failure naming the loaded path —
/// that can only mean a broken kit ABI, never a silently stale load.
fn resolve() -> Api {
    let path = build_cdylib();
    eprintln!("c_error_diagnostics: dlopen {}", path.display());
    // SAFETY: dlopen of a workspace-built cdylib, kept alive in the struct.
    let library = unsafe { libloading::Library::new(&path) }.expect("dlopen elf_load cdylib");
    let missing = |error: libloading::Error, what: &str| -> ! {
        panic!(
            "{what} must be exported by {}; a build missing it means a broken \
             kit ABI, not a stale load: {error}",
            path.display()
        )
    };
    // SAFETY: the pinned library stays alive in the Api below.
    let last_error = unsafe { library.get::<LastErrorFn>(b"rudroid_elf_last_error") }
        .unwrap_or_else(|error| missing(error, "rudroid_elf_last_error (ticket 65)"));
    // SAFETY: see above.
    let load_bytes = unsafe { library.get::<LoadBytesFn>(b"rudroid_elf_load_from_bytes") }
        .unwrap_or_else(|error| missing(error, "rudroid_elf_load_from_bytes (ticket 12)"));
    // SAFETY: see above.
    let load_path = unsafe { library.get::<LoadPathFn>(b"rudroid_elf_load_from_path") }
        .unwrap_or_else(|error| missing(error, "rudroid_elf_load_from_path (ticket 12)"));
    // SAFETY: see above.
    let destroy = unsafe { library.get::<DestroyFn>(b"rudroid_elf_destroy") }
        .unwrap_or_else(|error| missing(error, "rudroid_elf_destroy (ticket 12)"));
    #[cfg(target_os = "linux")]
    let exec = {
        // SAFETY: see above.
        let load = unsafe { library.get::<ExecLoadFn>(b"rudroid_elf_exec_load") }
            .unwrap_or_else(|error| missing(error, "rudroid_elf_exec_load (ticket 49)"));
        // SAFETY: see above.
        let symbol = unsafe { library.get::<ExecSymbolFn>(b"rudroid_elf_exec_symbol") }
            .unwrap_or_else(|error| missing(error, "rudroid_elf_exec_symbol (ticket 49)"));
        // SAFETY: see above.
        let destroy = unsafe { library.get::<ExecDestroyFn>(b"rudroid_elf_exec_destroy") }
            .unwrap_or_else(|error| missing(error, "rudroid_elf_exec_destroy (ticket 49)"));
        Some(ExecApi {
            load: *load,
            symbol: *symbol,
            destroy: *destroy,
        })
    };
    #[cfg(not(target_os = "linux"))]
    let exec = None;
    Api {
        last_error: *last_error,
        load_bytes: *load_bytes,
        load_path: *load_path,
        destroy: *destroy,
        exec,
        _library: library,
    }
}

impl Api {
    /// Fetch the current thread's error as a Rust string (two-phase).
    fn fetch(&self) -> Option<String> {
        unsafe {
            let need = (self.last_error)(std::ptr::null_mut(), 0);
            if need == 0 {
                return None;
            }
            let mut buffer = vec![0u8; need];
            (self.last_error)(buffer.as_mut_ptr() as *mut c_char, need);
            buffer.truncate(need - 1); // drop the NUL
            String::from_utf8(buffer).ok()
        }
    }
}

#[test]
fn failures_keep_their_reason_and_successes_clear_it() {
    let api = resolve();

    // No error before any failing call.
    assert_eq!(api.fetch(), None, "a fresh thread starts without an error");

    // A malformed image load fails with a parse reason.
    let garbage = [0u8; 64];
    // SAFETY: pointers to live data; handles destroyed below.
    let handle = unsafe { (api.load_bytes)(garbage.as_ptr(), garbage.len()) };
    assert!(handle.is_null());
    let reason = api.fetch().expect("a failure must record an error");
    assert!(
        reason.contains("magic"),
        "the parse diagnostic must survive the C boundary: {reason}"
    );

    // A successful load clears the stale error.
    let fixture = build_minimal_aarch64_so();
    let handle = unsafe { (api.load_bytes)(fixture.as_ptr(), fixture.len()) };
    assert!(!handle.is_null());
    assert_eq!(
        api.fetch(),
        None,
        "a successful load must not leave the old error masquerading as its result"
    );
    unsafe { (api.destroy)(handle) };

    // Path loads name the module path.
    let dir = std::env::temp_dir().join(format!("rudroid-ticket65-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad_path = dir.join("broken-guest.so");
    std::fs::write(&bad_path, &garbage).unwrap();
    let c_path = CString::new(bad_path.to_str().unwrap()).unwrap();
    // SAFETY: c_path outlives the call.
    let handle = unsafe { (api.load_path)(c_path.as_ptr()) };
    assert!(handle.is_null());
    let reason = api.fetch().expect("a path failure must record an error");
    assert!(
        reason.contains("broken-guest.so") && reason.contains("magic"),
        "path failures must name the module and the reason: {reason}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn null_and_invalid_arguments_are_diagnosed_without_panics() {
    let api = resolve();

    // SAFETY: deliberately hostile arguments, all diagnosed.
    let handle = unsafe { (api.load_path)(std::ptr::null()) };
    assert!(handle.is_null());
    assert!(
        api.fetch().is_some(),
        "a null path must be diagnosable through the error channel"
    );

    let handle = unsafe { (api.load_bytes)(std::ptr::null(), 16) };
    assert!(handle.is_null());
    assert!(
        api.fetch().is_some(),
        "a null bytes pointer must be diagnosable"
    );

    // A zero-length image is a parse diagnostic, not a panic.
    let empty = [0u8; 0];
    let handle = unsafe { (api.load_bytes)(empty.as_ptr(), 0) };
    assert!(handle.is_null());
    assert!(api.fetch().is_some());
}

#[test]
fn the_two_phase_query_survives_truncation() {
    let api = resolve();
    let garbage = [0u8; 64];
    // SAFETY: pointers to live data.
    let handle = unsafe { (api.load_bytes)(garbage.as_ptr(), garbage.len()) };
    assert!(handle.is_null());

    unsafe {
        let need = (api.last_error)(std::ptr::null_mut(), 0);
        assert!(
            need > 1,
            "query phase returns the full length including NUL"
        );

        // A too-small buffer must not overrun and reports the same length.
        let mut small = vec![0x7Au8; 4]; // sentinel filler ('z')
        let reported = (api.last_error)(small.as_mut_ptr() as *mut c_char, small.len());
        assert_eq!(
            reported, need,
            "the reported length never depends on capacity"
        );
        assert_ne!(
            &small[..3],
            b"zzz",
            "message content copied (sentinel overwritten), truncated to capacity - 1"
        );
        assert_eq!(small[3], 0, "always NUL-terminated within capacity");

        // The full fetch round-trips exactly.
        let mut full = vec![0u8; need];
        (api.last_error)(full.as_mut_ptr() as *mut c_char, need);
        assert_eq!(full[need - 1], 0, "fetched message is NUL-terminated");
    }
}

#[test]
fn concurrent_failing_loads_do_not_swap_errors() {
    let api = std::sync::Arc::new(resolve());
    let dir = std::env::temp_dir().join(format!("rudroid-ticket65-conc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let garbage = [0u8; 64];

    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for index in 0..2 {
        let api = api.clone();
        let path = dir.join(format!("bad-{index}.so"));
        std::fs::write(&path, &garbage).unwrap();
        let c_path = CString::new(path.to_str().unwrap()).unwrap();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            // SAFETY: c_path outlives the call; api keeps the library alive.
            let handle = unsafe { (api.load_path)(c_path.as_ptr()) };
            assert!(handle.is_null());
            (index, api.fetch())
        }));
    }
    barrier.wait();
    for thread in threads {
        let (index, message) = thread.join().unwrap();
        let message = message.expect("each thread sees its own failure");
        assert!(
            message.contains(&format!("bad-{index}.so")),
            "thread {index} must see its own path, not another's: {message}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Ticket 68: the executable backend (ticket 49) shares the last-error
// channel. Linux only; off Linux the cfg compiles these out and they never
// report as executed. Synthetic fixture images are mapped and queried but
// their code is never entered (execution proof: tests/executable.rs).

#[test]
#[cfg(target_os = "linux")]
fn exec_load_failures_name_path_and_reason() {
    let api = resolve();
    let exec = api.exec.as_ref().expect("exec entries on Linux");

    assert_eq!(api.fetch(), None, "a fresh thread starts without an error");

    // A null path is diagnosable, not a crash.
    // SAFETY: deliberately null.
    let handle = unsafe { (exec.load)(std::ptr::null()) };
    assert!(handle.is_null());
    assert!(
        api.fetch().is_some(),
        "a null path must be diagnosable through the error channel"
    );

    // A missing file reports the io error with the requested path.
    let missing = std::env::temp_dir().join(format!("rudroid-t68-absent-{}.so", std::process::id()));
    let c_missing = CString::new(missing.to_str().unwrap()).unwrap();
    // SAFETY: c_missing outlives the call.
    let handle = unsafe { (exec.load)(c_missing.as_ptr()) };
    assert!(handle.is_null());
    let reason = api.fetch().expect("a missing file must record an error");
    assert!(
        reason.contains("rudroid-t68-absent") && reason.contains("No such file"),
        "exec load failures must name the path and the io reason: {reason}"
    );

    // A non-ELF file keeps the parse diagnostic with the module path.
    let dir = std::env::temp_dir().join(format!("rudroid-t68-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let garbage = dir.join("not-an-elf.so");
    std::fs::write(&garbage, [0u8; 64]).unwrap();
    let c_garbage = CString::new(garbage.to_str().unwrap()).unwrap();
    // SAFETY: c_garbage outlives the call.
    let handle = unsafe { (exec.load)(c_garbage.as_ptr()) };
    assert!(handle.is_null());
    let reason = api.fetch().expect("a parse failure must record an error");
    assert!(
        reason.contains("not-an-elf.so") && reason.contains("magic"),
        "exec load failures must keep the parse diagnostic: {reason}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(target_os = "linux")]
fn exec_success_clears_stale_error_and_symbol_misses_stay_silent() {
    let api = resolve();
    let exec = api.exec.as_ref().expect("exec entries on Linux");

    // Seed a stale error from a failing buffer load.
    let garbage = [0u8; 64];
    // SAFETY: pointers to live data.
    let stale = unsafe { (api.load_bytes)(garbage.as_ptr(), garbage.len()) };
    assert!(stale.is_null());
    assert!(api.fetch().is_some(), "precondition: an error is recorded");

    // ...then map the synthetic fixture through the exec backend: the
    // success must clear the old error instead of letting it masquerade as
    // the exec result.
    let dir = std::env::temp_dir().join(format!("rudroid-t68-exec-ok-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let so = dir.join("minimal-answer.so");
    std::fs::write(&so, build_minimal_aarch64_so()).unwrap();
    let c_so = CString::new(so.to_str().unwrap()).unwrap();
    // SAFETY: c_so outlives the call; the fixture is parsed, never executed.
    let handle = unsafe { (exec.load)(c_so.as_ptr()) };
    assert!(!handle.is_null(), "the synthetic fixture must map for exec");
    assert_eq!(
        api.fetch(),
        None,
        "a successful exec load must clear the stale error"
    );

    // The defined export resolves to a real address; a miss stays the
    // documented 0 and must not invent an error.
    // SAFETY: handle is live; names are NUL-terminated literals.
    let answer = unsafe { (exec.symbol)(handle, b"rudroid_answer\0".as_ptr() as *const _) };
    assert_ne!(
        answer, 0,
        "rudroid_answer must resolve through the exec backend"
    );
    // SAFETY: handle is live.
    let miss = unsafe { (exec.symbol)(handle, b"rudroid_missing\0".as_ptr() as *const _) };
    assert_eq!(miss, 0, "a symbol miss is 0");
    assert_eq!(api.fetch(), None, "a symbol miss is not an error");

    // Null handle / null name are diagnosed, not crashes.
    // SAFETY: deliberately hostile arguments.
    assert_eq!(
        unsafe { (exec.symbol)(std::ptr::null(), b"x\0".as_ptr() as *const _) },
        0
    );
    assert!(api.fetch().is_some(), "a null handle must be diagnosable");
    // SAFETY: deliberately hostile argument.
    assert_eq!(unsafe { (exec.symbol)(handle, std::ptr::null()) }, 0);
    assert!(api.fetch().is_some(), "a null name must be diagnosable");

    // Destroy unmaps (success leaves no error); a null destroy is a
    // diagnosed no-op.
    // SAFETY: the handle came from exec_load above, destroyed exactly once.
    unsafe { (exec.destroy)(handle) };
    assert_eq!(api.fetch(), None, "a successful destroy clears the channel");
    // SAFETY: deliberately null.
    unsafe { (exec.destroy)(std::ptr::null_mut()) };
    assert!(
        api.fetch().is_some(),
        "a null destroy must be diagnosable through the error channel"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(target_os = "linux")]
fn exec_failure_error_channel_survives_truncation_and_repeat() {
    let api = resolve();
    let exec = api.exec.as_ref().expect("exec entries on Linux");
    let dir = std::env::temp_dir().join(format!("rudroid-t68-exec-trunc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let garbage = dir.join("truncated-query.so");
    std::fs::write(&garbage, [0u8; 64]).unwrap();
    let c_garbage = CString::new(garbage.to_str().unwrap()).unwrap();
    // SAFETY: c_garbage outlives the call.
    let handle = unsafe { (exec.load)(c_garbage.as_ptr()) };
    assert!(handle.is_null());

    unsafe {
        let need = (api.last_error)(std::ptr::null_mut(), 0);
        assert!(
            need > 1,
            "query phase returns the full length including NUL"
        );

        // A too-small buffer must not overrun and reports the same length.
        let mut small = vec![0x7Au8; 4]; // sentinel filler ('z')
        let reported = (api.last_error)(small.as_mut_ptr() as *mut c_char, small.len());
        assert_eq!(
            reported, need,
            "the reported length never depends on capacity"
        );
        assert_eq!(small[3], 0, "always NUL-terminated within capacity");

        // Queries never clear the message: repeating the two-phase pattern
        // sees the exact same length (valid until the NEXT entry point).
        let repeated = (api.last_error)(std::ptr::null_mut(), 0);
        assert_eq!(repeated, need, "repeated queries observe the same message");
        let mut full = vec![0u8; need];
        (api.last_error)(full.as_mut_ptr() as *mut c_char, need);
        assert_eq!(
            full[need - 1],
            0,
            "the full fetch round-trips NUL-terminated"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
