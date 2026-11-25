//! Standard initializer execution: DT_INIT must run exactly once before
//! DT_INIT_ARRAY, and a DT_INIT-only module must be initialized.

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

mod cc_fixture;

use std::ffi::CString;

use android_elf_load::elf::ElfView;

fn load(path: &str) -> *mut android_elf_load::ExecutableModule {
    let path = CString::new(path).unwrap();
    let handle = unsafe { android_elf_load::rudroid_elf_exec_load(path.as_ptr()) };
    assert!(!handle.is_null(), "executable mapping must succeed");
    handle
}

fn call_i32(handle: *mut android_elf_load::ExecutableModule, name: &str) -> i32 {
    let name = CString::new(name).unwrap();
    let address =
        unsafe { android_elf_load::rudroid_elf_exec_symbol(handle, name.as_ptr()) };
    assert_ne!(address, 0, "fixture accessor must be exported");
    let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { function() }
}

fn parsed(path: &str) -> (bool, bool) {
    let bytes = std::fs::read(path).unwrap();
    let view = ElfView::parse(&bytes).unwrap();
    (view.init.is_some(), view.init_array.is_some())
}

#[test]
fn dt_init_only_runs_without_an_init_array() {
    let path = cc_fixture::compile(
        "rudroid_dt_init_only",
        "rudroid_dt_init_only",
        &["-Wl,-init,rudroid_dt_init_only"],
    );
    let path = path.to_str().unwrap();
    assert_eq!(parsed(path), (true, false));
    let handle = load(path);
    assert_eq!(call_i32(handle, "rudroid_dt_init_only_slot"), 42);
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
}

#[test]
fn dt_init_runs_before_each_init_array_constructor() {
    let path = cc_fixture::compile(
        "rudroid_dt_init_both",
        "rudroid_dt_init_both",
        &["-Wl,-init,rudroid_dt_init_both"],
    );
    let path = path.to_str().unwrap();
    assert_eq!(parsed(path), (true, true));
    let handle = load(path);
    assert_eq!(call_i32(handle, "rudroid_dt_init_both_slot"), 12);
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
}
