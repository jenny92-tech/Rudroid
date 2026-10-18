//! Ticket 49 acceptance: execute guest code from a permission-separated
//! executable mapping and verify the W^X property in /proc/self/maps.
//!
//! The guest SO is `tests/fixtures/rudroid49_answer.c` (exports `answer() -> 42`),
//! compiled by the test itself. Self-compiled ELF execution only — this
//! proves nothing about Android/Bionic or any real title.

#![cfg(target_os = "linux")]

use std::ffi::CString;

mod cc_fixture;

#[test]
fn executable_mapping_runs_answer_and_enforces_wx() {
    let so_path = cc_fixture::fixture("rudroid49_answer");

    let handle = unsafe {
        android_elf_load::rudroid_elf_exec_load(CString::new(so_path.clone()).unwrap().as_ptr())
    };
    assert!(!handle.is_null(), "executable mapping must succeed");

    // The function address comes from the guest's own dynsym/hash tables and
    // is an absolute host address inside our mapping.
    let answer = unsafe {
        android_elf_load::rudroid_elf_exec_symbol(handle, b"answer\0".as_ptr() as *const _)
    };
    assert_ne!(answer, 0, "answer must be exported");

    // Real execution — not just an address.
    let answer_fn: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(answer) };
    let result = unsafe { answer_fn() };
    assert_eq!(result, 42, "guest code must actually execute");

    // W^X evidence from the kernel: some mapping covering the module must be
    // executable, and no mapping overlapping the module may be both writable
    // and executable.
    let module_start = answer & !0xfff;
    let maps = std::fs::read_to_string("/proc/self/maps").expect("/proc/self/maps");
    let mut overlapping = Vec::new();
    for line in maps.lines() {
        // Ticket 85: file-backed mappings append the backing path to the
        // line — extract the perms field properly (the whole rest would
        // false-positive RWX on path characters like the "w" in /work).
        let Some((range, rest)) = line.split_once(' ') else {
            continue;
        };
        let perms = rest.split_whitespace().next().unwrap_or_default().to_owned();
        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end)) = (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16))
        else {
            continue;
        };
        if start < module_start + 0x10000 && end > module_start {
            overlapping.push((start, end, perms));
        }
    }
    assert!(
        overlapping.iter().any(|(_, _, perms)| perms.contains('x')),
        "at least one module range must be executable: {overlapping:?}"
    );
    for (_, _, perms) in &overlapping {
        assert!(
            !(perms.contains('w') && perms.contains('x')),
            "no overlapping range may be writable AND executable: {overlapping:?}"
        );
    }

    // Destroy unmaps: after teardown the module ranges are gone.
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
    let maps = std::fs::read_to_string("/proc/self/maps").expect("/proc/self/maps");
    assert!(
        !maps
            .lines()
            .any(|line| line.starts_with(&format!("{module_start:x}-"))),
        "module mapping must be gone after destroy"
    );
}

#[test]
fn exec_load_rejects_missing_files_and_null() {
    let missing = CString::new("/definitely-not-here-rudroid49.so").unwrap();
    let handle = unsafe { android_elf_load::rudroid_elf_exec_load(missing.as_ptr()) };
    assert!(handle.is_null(), "missing file must yield a null handle");
    // Null handle semantics: symbol is a miss, destroy is a no-op.
    assert_eq!(
        unsafe { android_elf_load::rudroid_elf_exec_symbol(std::ptr::null(), b"answer\0".as_ptr() as *const _) },
        0
    );
    unsafe { android_elf_load::rudroid_elf_exec_destroy(std::ptr::null_mut()) };
}
