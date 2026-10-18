//! `.init_array` execution acceptance: a compiler-produced AArch64 library
//! whose constructor writes an observable value must have that constructor
//! run during `rudroid_elf_exec_load`, before any exported call.
//!
//! The guest SO is `tests/fixtures/rudroid_init.c`, compiled by the test itself with
//! the container's `cc` (a missing compiler fails loudly).
//!
//! Without running the constructor `rudroid_slot1()` returns the linker-zeroed
//! 0; with it, 41. Self-compiled ELF execution only — this proves nothing
//! about Android/Bionic or any real title.

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

mod cc_fixture;

use std::ffi::CString;

#[test]
fn executable_mapping_runs_init_array_constructors() {
    let so_path = cc_fixture::fixture("rudroid_init");

    let handle = unsafe {
        android_elf_load::rudroid_elf_exec_load(CString::new(so_path).unwrap().as_ptr())
    };
    assert!(!handle.is_null(), "executable mapping must succeed");

    let slot = |name: &str| {
        let addr = unsafe {
            android_elf_load::rudroid_elf_exec_symbol(
                handle,
                CString::new(name).unwrap().as_ptr(),
            )
        };
        assert_ne!(addr, 0, "{name} must be exported");
        let f: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(addr) };
        unsafe { f() }
    };

    assert_eq!(slot("rudroid_slot0"), 40, "data must be loaded verbatim");
    assert_eq!(
        slot("rudroid_slot1"),
        41,
        "the .init_array constructor must have run during load"
    );

    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
}
