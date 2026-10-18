//! Ticket 85 rework acceptance: failure paths clean up EXACTLY once, and a
//! page that would need to be both executable and writable is rejected
//! structurally instead of silently losing +x on its code half.
//!
//! - Missing strong symbol → relocation fails AFTER both mappings exist:
//!   no metadata or region mapping may survive (single cleanup owner per
//!   mapping — the old path munmapped the metadata mapping twice, once in
//!   the inner error path and once in the outer), and the process must
//!   stay healthy for further loads.
//! - Hostile same-page RX/RW layout → structured rejection before anything
//!   is mapped (the old two-pass W^X overwrote R|X with R|W and reported
//!   success on a layout whose code would then fault).
//!
//! Linux-only: the executable backend is Linux (ticket 49/85).

#![cfg(target_os = "linux")]

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn write_fixture(name: &str, image: &[u8]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::AcqRel);
    let path = std::env::temp_dir().join(format!(
        "rudroid-t85-{}-{}-{name}.so",
        std::process::id(),
        id
    ));
    std::fs::write(&path, image).expect("write fixture");
    path
}

/// True when NO /proc/self/maps line names `path` — a failed load must not
/// leave its metadata (file-backed, MAP_SHARED → named) mapping behind.
fn file_unmapped(path: &PathBuf) -> bool {
    let maps = std::fs::read_to_string("/proc/self/maps").expect("read maps");
    let needle = path.to_str().expect("utf8 fixture path");
    !maps.lines().any(|line| line.contains(needle))
}

fn load(path: &PathBuf) -> *mut android_elf_load::ExecutableModule {
    let c = CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: NUL-terminated path owned for the call.
    unsafe { android_elf_load::rudroid_elf_exec_load(c.as_ptr()) }
}

fn last_error() -> String {
    let mut buffer = [0u8; 512];
    // SAFETY: valid buffer for the call.
    let len =
        unsafe { android_elf_load::rudroid_elf_last_error(buffer.as_mut_ptr(), buffer.len()) };
    let len = len.min(buffer.len());
    String::from_utf8_lossy(&buffer[..len]).into_owned()
}

/// A healthy control load: the minimal fixture executes, the answer export
/// reads back — proves the process stayed healthy after the failures.
fn control_load_still_works() {
    let path = write_fixture("control", &android_elf_load::fixture::build_minimal_aarch64_so());
    let handle = load(&path);
    assert!(!handle.is_null(), "control load must succeed");
    // SAFETY: valid handle from the load above.
    let address = unsafe {
        android_elf_load::rudroid_elf_exec_symbol(
            handle,
            c"rudroid_answer".as_ptr(),
        )
    };
    assert_ne!(address, 0, "control export must resolve");
    // SAFETY: handle we own.
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
    assert!(file_unmapped(&path), "destroyed control must be fully unmapped");
    let _ = std::fs::remove_file(&path);
}

/// Relocation-stage failure (strong undefined import): the error is
/// structured, NOTHING of the failed load survives in the maps, and the
/// process keeps loading correctly — exercised repeatedly so a
/// double-unmap (old bug: meta unmapped by both error paths) or a leak
/// would show up as corruption or a leftover named VMA.
#[test]
fn missing_symbol_failure_cleans_up_exactly_once() {
    let missing = write_fixture(
        "missing_symbol",
        &android_elf_load::fixture::build_missing_symbol_aarch64_so(),
    );
    for round in 0..3 {
        let handle = load(&missing);
        assert!(handle.is_null(), "strong undefined import must fail the load");
        let error = last_error();
        assert!(
            error.contains("rudroid_import"),
            "round {round}: structured missing-symbol diagnostic required: {error}"
        );
        assert!(
            file_unmapped(&missing),
            "round {round}: the failed load must leave NO mapping of the backing file \
             (leak) and must not disturb other mappings (double unmap)"
        );
        control_load_still_works();
    }
    let _ = std::fs::remove_file(&missing);
}

/// Same-page RX/RW layout: rejected structurally before anything is
/// mapped, with a diagnostic that names both page ranges — never a
/// "successful" load whose code half lost +x (the old two-pass bug), and
/// never RWX.
#[test]
fn overlapping_wx_layout_is_rejected_structurally() {
    let overlap = write_fixture(
        "overlap_wx",
        &android_elf_load::fixture::build_overlapping_wx_aarch64_so(),
    );
    for round in 0..3 {
        let handle = load(&overlap);
        assert!(
            handle.is_null(),
            "round {round}: an execute/writable same-page layout must be refused"
        );
        let error = last_error();
        assert!(
            error.contains("share page"),
            "round {round}: structured W^X-overlap diagnostic required: {error}"
        );
        assert!(
            file_unmapped(&overlap),
            "round {round}: the rejected load must leave no mapping behind"
        );
        control_load_still_works();
    }
    let _ = std::fs::remove_file(&overlap);
}
