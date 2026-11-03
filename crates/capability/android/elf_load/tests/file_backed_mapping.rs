//! Ticket 85 acceptance: exec mappings are FILE-BACKED (MAP_PRIVATE over the
//! deployed .so) so clean code pages are page-cache shared and reclaimable;
//! BSS tails zeroed; relocations land as COW private pages; W^X holds.
//!
//! Red evidence (pre-85): the mapping ranges are anonymous — /proc/self/maps
//! shows no backing file for the module's base.
//!
//! Fixture: `tests/fixtures/rudroid_init.c` (ticket 79, with constructor), compiled
//! by the test itself with the container's `cc`.

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

mod cc_fixture;

use std::ffi::CString;
use std::path::Path;

fn load(path: &Path) -> *mut android_elf_load::ExecutableModule {
    // SAFETY: NUL-free path; handle contract per C API.
    let handle = unsafe {
        android_elf_load::rudroid_elf_exec_load(
            CString::new(path.to_str().unwrap().to_owned())
                .unwrap()
                .as_ptr(),
        )
    };
    assert!(!handle.is_null(), "exec load must succeed for {path:?}");
    handle
}

fn base_of(handle: *const android_elf_load::ExecutableModule) -> usize {
    // SAFETY: live handle.
    unsafe { android_elf_load::rudroid_elf_exec_load_base(handle) }
}

fn symbol(handle: *const android_elf_load::ExecutableModule, name: &str) -> usize {
    // SAFETY: live handle; NUL-free name.
    unsafe {
        android_elf_load::rudroid_elf_exec_symbol(
            handle,
            CString::new(name).unwrap().as_ptr(),
        )
    }
}

/// The map lines covering [base, base+0x10000) with their permission sets.
fn maps_overlapping(base: usize) -> Vec<(String, String)> {
    let maps = std::fs::read_to_string("/proc/self/maps").expect("/proc/self/maps");
    let mut out = Vec::new();
    for line in maps.lines() {
        let Some((range, rest)) = line.split_once(' ') else {
            continue;
        };
        let perms = rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end)) =
            (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16))
        else {
            continue;
        };
        if start < base + 0x10000 && end > base {
            out.push((line.to_owned(), perms));
        }
    }
    out
}

#[test]
fn exec_mapping_is_file_backed_and_still_runs() {
    let so_path = cc_fixture::fixture("rudroid_init");
    let _serial_guard = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let handle = load(Path::new(&so_path));
    let base = base_of(handle);
    assert_ne!(base, 0);

    // Ticket 85 core: the module's ranges are backed by the deployed file —
    // /proc/self/maps names the .so for ranges overlapping the base.
    let overlapping = maps_overlapping(base);
    assert!(
        overlapping.iter().any(|(line, _)| line.contains("rudroid_init.so")),
        "file-backed mapping must name the backing file: {overlapping:#?}"
    );

    // Execution, constructor semantics and symbols unchanged (79 contract).
    let slot = |name: &str| {
        let address = symbol(handle, name);
        assert_ne!(address, 0, "{name} must be exported");
        // SAFETY: exported getter on the mapped image.
        let f: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        unsafe { f() }
    };
    assert_eq!(slot("rudroid_slot0"), 40, "data loaded verbatim");
    assert_eq!(slot("rudroid_slot1"), 41, "constructor ran once");

    // W^X: no overlapping range may be writable AND executable.
    for (_, perms) in &overlapping {
        assert!(
            !(perms.contains('w') && perms.contains('x')),
            "no RWX range: {overlapping:#?}"
        );
    }

    // Destroy unmaps: the file-backed ranges are gone afterwards.
    // SAFETY: handle from exec_load.
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
    let maps = std::fs::read_to_string("/proc/self/maps").expect("/proc/self/maps");
    assert!(
        !maps
            .lines()
            .any(|line| line.contains("rudroid_init.so") && {
                let range = line.split(' ').next().unwrap_or_default();
                let Some((start, _)) = range.split_once('-') else {
                    return false;
                };
                usize::from_str_radix(start, 16)
                    .map(|s| s & !0xfff == base & !0xfff)
                    .unwrap_or(false)
            }),
        "file-backed ranges must be gone after destroy"
    );
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
