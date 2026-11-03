//! `.init_array` parse-time validation (reference `so_initialize` scope:
//! DT_INIT_ARRAY/DT_INIT_ARRAYSZ; DT_INIT is deliberately not part of the
//! semantics). Malformed tables must fail with a diagnostic at load, on both
//! backends — these run through the buffer backend so they execute on any
//! host; the exec backend additionally RUNS the constructors (see
//! `init_array_exec.rs`, Linux only).

use std::collections::HashMap;

use android_elf_load::fixture::build_minimal_aarch64_so;
use android_elf_load::GuestModule;

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn set_dynamic(bytes: &mut [u8], index: usize, tag: u64, value: u64) {
    let at = 176 + index * 16;
    put64(bytes, at, tag);
    put64(bytes, at + 8, value);
}

#[test]
fn init_array_outside_file_backed_range_is_rejected() {
    let mut bytes = build_minimal_aarch64_so();
    set_dynamic(&mut bytes, 7, 25, 0x00F0_0000_0000); // DT_INIT_ARRAY, past the image
    set_dynamic(&mut bytes, 8, 27, 8); // DT_INIT_ARRAYSZ
    set_dynamic(&mut bytes, 9, 0, 0);
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("init-array past the image must not load");
    assert!(err.to_string().contains("init-array"), "{err}");
    assert!(err.to_string().contains("not file-backed"), "{err}");
}

#[test]
fn init_array_unaligned_size_is_rejected() {
    let mut bytes = build_minimal_aarch64_so();
    set_dynamic(&mut bytes, 7, 25, 576);
    set_dynamic(&mut bytes, 8, 27, 12); // not a multiple of 8
    set_dynamic(&mut bytes, 9, 0, 0);
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("unaligned init-array size must not load");
    assert!(err.to_string().contains("not word-aligned"), "{err}");
}

#[test]
fn init_array_without_size_is_rejected() {
    let mut bytes = build_minimal_aarch64_so();
    set_dynamic(&mut bytes, 7, 25, 576); // DT_INIT_ARRAY, no DT_INIT_ARRAYSZ
    set_dynamic(&mut bytes, 8, 0, 0);
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("half init-array pair must not load");
    assert!(err.to_string().contains("without DT_INIT_ARRAYSZ"), "{err}");
}
