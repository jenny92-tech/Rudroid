//! Ticket 04 + 71 acceptance: the catalog C API must be callable from outside
//! the process through `libloading`, exactly like the loader/jni_vm will do —
//! and the whole suite must run from one supported command on an empty target
//! dir.
//!
//! Assembly contract (ticket 71, the same defect shape elf_load's ticket 68
//! fixed): `cargo test` does not produce cdylib artifacts, and the old test
//! hardcoded `../../../target/debug/...` while demanding a manual warm-up
//! build. From an empty target both tests failed with exit 101, and whatever
//! `.so` happened to sit in the shared target dir would have been dlopened
//! without any freshness check. These tests therefore locate the cdylib the
//! way cargo does (CARGO_TARGET_DIR when the invocation set one, else the
//! workspace `target`, plus this run's profile) and rebuild it with the same
//! cargo before dlopen — one `cargo test ...` command works from an empty
//! target dir and can never load an artifact older than the current source.
//! A missing artifact or a missing contract symbol fails with a message
//! naming the exact path and what is wrong, never a silent skip or a stale
//! misload.

use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::{Path, PathBuf};
use std::process::Command;

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn crates_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <crates>/capability/android/reply_catalog: three
    // ancestor hops up to the workspace root (helpers that hardcode ../..
    // instead live in crates one level shallower — do not copy them blindly).
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates root above capability/android/reply_catalog")
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

fn lib_name() -> String {
    if cfg!(target_os = "macos") {
        "librudroid_android_reply_catalog.dylib".into()
    } else {
        "librudroid_android_reply_catalog.so".into()
    }
}

fn cdylib_path() -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    workspace_target_dir().join(profile).join(lib_name())
}

/// Build the cdylib with the same cargo, target dir and profile as this test
/// run, then return its path. One `cargo test ...` command therefore works
/// from an empty target dir (ticket 71), and the dlopen below can never pick
/// up a stale artifact: cargo rebuilds whenever the crate sources changed,
/// and profile + target dir always match this run.
fn build_cdylib() -> PathBuf {
    let path = cdylib_path();
    let mut command = Command::new(cargo());
    command.args(["build", "--locked", "-p", "rudroid-android-reply-catalog"]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command
        .current_dir(crates_root())
        .status()
        .expect("spawn cargo for the reply_catalog cdylib");
    assert!(
        status.success(),
        "building the reply_catalog cdylib failed; ffi_load cannot run"
    );
    assert!(
        path.exists(),
        "cargo build succeeded but no cdylib at {}; \
         the test resolved a different CARGO_TARGET_DIR than the build",
        path.display()
    );
    path
}

#[test]
fn loads_and_queries_through_dlopen() {
    let path = build_cdylib();
    eprintln!("ffi_load: dlopen {}", path.display());
    // SAFETY: dlopen of a workspace-built cdylib; the handle is used only
    // inside this test and dropped at its end.
    let lib = unsafe { Library::new(&path) }.expect("dlopen catalog cdylib");

    let blob = rudroid_android_reply_catalog::encode(&[
        rudroid_android_reply_catalog::Entry {
            class: "android/media/AudioManager",
            method: "getStreamVolume",
            sig: "(I)I",
            value: rudroid_android_reply_catalog::ReplyValue::Int(100),
        },
        rudroid_android_reply_catalog::Entry {
            class: "android/media/AudioManager",
            method: "isBluetoothA2dpOn",
            sig: "()Z",
            value: rudroid_android_reply_catalog::ReplyValue::Bool(false),
        },
    ]);

    unsafe {
        let from_bytes: Symbol<unsafe extern "C" fn(*const u8, usize) -> *mut c_void> =
            lib.get(b"rudroid_reply_catalog_from_bytes").unwrap();
        let handle = from_bytes(blob.as_ptr(), blob.len());
        assert!(!handle.is_null());

        let class = CString::new("android/media/AudioManager").unwrap();
        let sig_int = CString::new("(I)I").unwrap();

        let query_int: Symbol<
            unsafe extern "C" fn(*const c_void, *const c_char, *const c_char, *const c_char, *mut i64) -> c_int,
        > = lib.get(b"rudroid_reply_catalog_query_int").unwrap();
        let method = CString::new("getStreamVolume").unwrap();
        let mut out: i64 = -1;
        assert_eq!(
            query_int(handle, class.as_ptr(), method.as_ptr(), sig_int.as_ptr(), &mut out),
            1
        );
        assert_eq!(out, 100);

        let query_bool: Symbol<
            unsafe extern "C" fn(*const c_void, *const c_char, *const c_char, *const c_char, *mut c_int) -> c_int,
        > = lib.get(b"rudroid_reply_catalog_query_bool").unwrap();
        let bmethod = CString::new("isBluetoothA2dpOn").unwrap();
        let sig_bool = CString::new("()Z").unwrap();
        let mut bout: c_int = 1;
        assert_eq!(
            query_bool(handle, class.as_ptr(), bmethod.as_ptr(), sig_bool.as_ptr(), &mut bout),
            1
        );
        assert_eq!(bout, 0);

        // Miss: out-param untouched, return 0.
        let missing = CString::new("nope").unwrap();
        bout = 1;
        assert_eq!(
            query_bool(handle, class.as_ptr(), missing.as_ptr(), sig_bool.as_ptr(), &mut bout),
            0
        );
        assert_eq!(bout, 1);

        let destroy: Symbol<unsafe extern "C" fn(*mut c_void)> =
            lib.get(b"rudroid_reply_catalog_destroy").unwrap();
        destroy(handle);
    }
}

#[test]
fn symbol_names_match_common_contract() {
    use rudroid_common::symbols::reply_catalog;
    let path = build_cdylib();
    eprintln!("ffi_load: dlopen {}", path.display());
    // SAFETY: dlopen of the same workspace-built cdylib as above.
    let lib = unsafe { Library::new(&path) }.expect("dlopen catalog cdylib");
    for name in reply_catalog::ALL {
        let bytes = std::ffi::CString::new(name).unwrap();
        unsafe {
            lib.get::<*mut c_void>(bytes.as_bytes_with_nul())
                .unwrap_or_else(|e| {
                    panic!(
                        "symbol {name} must be exported by {}; a build missing it means a \
                         broken kit ABI, not a stale load: {e}",
                        path.display()
                    )
                });
        }
    }
}
