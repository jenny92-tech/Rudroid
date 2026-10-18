//! RELR packed-relative relocation support (decode ported from the
//! reference `so_relr_relocate`).
//!
//! The synthetic fixture's free file range [576, 640) backs the RELR table;
//! its dynamic table has spare slots from index 7 on (the DT_NULL that used
//! to sit there moves to the last slot). Everything runs on the buffer
//! backend; the executable backend shares the same `relocations::apply`
//! path.

use std::collections::HashMap;

use android_elf_load::fixture::build_minimal_aarch64_so;
use android_elf_load::GuestModule;

const RELR_TABLE: u64 = 576;

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// Overwrite dynamic-table slot `index` (7..10 are free once DT_NULL moves).
fn set_dynamic(bytes: &mut [u8], index: usize, tag: u64, value: u64) {
    let at = 176 + index * 16;
    put64(bytes, at, tag);
    put64(bytes, at + 8, value);
}

/// Write the RELR payload at [576, ..) and declare it via slot 7/8.
fn with_relr(mut bytes: Vec<u8>, tag_addr: u64, tag_size: u64, entries: &[u64]) -> Vec<u8> {
    for (slot, entry) in entries.iter().enumerate() {
        put64(&mut bytes, RELR_TABLE as usize + slot * 8, *entry);
    }
    set_dynamic(&mut bytes, 7, tag_addr, RELR_TABLE);
    set_dynamic(&mut bytes, 8, tag_size, (entries.len() * 8) as u64);
    set_dynamic(&mut bytes, 9, 0, 0); // DT_NULL
    bytes
}

fn word(module: &GuestModule, vaddr: u64) -> usize {
    // SAFETY: vaddr lies inside the fixture's single PT_LOAD; the test never
    // hands out the pointer.
    unsafe { ((module.base() + vaddr as usize) as *const usize).read_unaligned() }
}

/// A lone even entry relocates that one word: value += base.
#[test]
fn relr_address_entry_adds_base() {
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, 600, 0x2000);
    let bytes = with_relr(bytes, 36, 35, &[600]); // DT_RELR / DT_RELRSZ
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("RELR load must succeed");
    assert_eq!(word(&module, 600), module.base() + 0x2000);
}

/// The transitional Android tags carry the identical payload.
#[test]
fn android_transitional_tags_are_accepted() {
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, 600, 0x2000);
    let bytes = with_relr(bytes, 0x6fffe000, 0x6fffe001, &[600]);
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("Android RELR load");
    assert_eq!(word(&module, 600), module.base() + 0x2000);
}

/// Odd entries are 63-bit bitmaps over the words after the address entry
/// (0b1011 -> 600, 608, 624).
#[test]
fn bitmap_entries_decode_like_the_reference() {
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, 600, 0x2000);
    put64(&mut bytes, 608, 0x3000);
    put64(&mut bytes, 624, 0x4000);
    let bytes = with_relr(bytes, 36, 35, &[600, 0b1011]);
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("RELR bitmap load");
    assert_eq!(word(&module, 600), module.base() + 0x2000);
    assert_eq!(word(&module, 608), module.base() + 0x3000);
    assert_eq!(word(&module, 624), module.base() + 0x4000);
}

/// A real packed RELA address without its size must fail instead of silently skipping.
#[test]
fn real_android_rela_address_without_size_is_rejected_not_skipped() {
    let mut bytes = build_minimal_aarch64_so();
    set_dynamic(&mut bytes, 7, 0x60000011, 1); // real DT_ANDROID_RELA; missing size must fail
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("packed sleb128 must not load");
    assert!(err.to_string().contains("packed sleb128"), "{err}");
}

/// An address tag without a size tag (or vice versa) is a malformed table.
#[test]
fn relr_without_size_is_rejected() {
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, 600, 0x2000);
    set_dynamic(&mut bytes, 7, 36, RELR_TABLE); // DT_RELR, no DT_RELRSZ
    set_dynamic(&mut bytes, 8, 0, 0);
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("RELR without DT_RELRSZ must not load");
    assert!(err.to_string().contains("without DT_RELRSZ"), "{err}");
}

/// A RELR target outside PT_LOAD coverage fails on the same channel as the
/// RELA path (tickets 47/63) instead of writing out of bounds.
#[test]
fn relr_target_outside_coverage_is_rejected() {
    let bytes = with_relr(build_minimal_aarch64_so(), 36, 35, &[1000]);
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("target past the image must not load");
    assert!(
        err.to_string()
            .contains("RELR target write range is not fully covered"),
        "{err}"
    );
}

/// A declared DT_RELRENT other than 8 is a malformed table, not a guess.
#[test]
fn wrong_relrent_is_rejected() {
    let mut bytes = with_relr(build_minimal_aarch64_so(), 36, 35, &[600]);
    set_dynamic(&mut bytes, 9, 37, 16); // DT_RELRENT != 8
    let err = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .expect("bad DT_RELRENT must not load");
    assert!(err.to_string().contains("entry size is not 8"), "{err}");
}

fn sleb(mut value: i64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 127) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 64 == 0) || (value == -1 && byte & 64 != 0);
        if !done {
            byte |= 128;
        }
        out.push(byte);
        if done {
            break;
        }
    }
}
fn with_android_packed(mut bytes: Vec<u8>, rela: bool, values: &[i64]) -> Vec<u8> {
    let mut packed = b"APS2".to_vec();
    for value in values {
        sleb(*value, &mut packed);
    }
    assert!(packed.len() <= 24);
    bytes[576..576 + packed.len()].copy_from_slice(&packed);
    set_dynamic(
        &mut bytes,
        7,
        if rela { 0x60000011 } else { 0x6000000f },
        576,
    );
    set_dynamic(
        &mut bytes,
        8,
        if rela { 0x60000012 } else { 0x60000010 },
        packed.len() as u64,
    );
    set_dynamic(&mut bytes, 9, 0, 0);
    bytes
}
#[test]
fn aps2_rela_applies_grouped_and_negative_addends_through_the_normal_checked_writer() {
    let bytes = with_android_packed(
        build_minimal_aarch64_so(),
        true,
        &[2, 592, 2, 11, 8, 1027, 8192, -4096],
    );
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).unwrap();
    assert_eq!(word(&module, 600), module.base() + 8192);
    assert_eq!(word(&module, 608), module.base() + 4096);
}
#[test]
fn aps2_rel_uses_the_target_word_as_implicit_addend() {
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, 600, 0x2340);
    let bytes = with_android_packed(bytes, false, &[1, 592, 1, 3, 8, 1027]);
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).unwrap();
    assert_eq!(word(&module, 600), module.base() + 0x2340);
}
#[test]
fn packed_targets_outside_load_coverage_and_truncated_groups_are_rejected() {
    for values in [&[1, 992, 1, 3, 8, 1027][..], &[1, 592, 1, 0, 8][..]] {
        let bytes = with_android_packed(build_minimal_aarch64_so(), true, values);
        assert!(GuestModule::from_bytes(bytes, &HashMap::new()).is_err());
    }
}
#[test]
fn android_relrent_uses_the_real_003_tag_and_rejects_non_word_entries() {
    let mut bytes = with_relr(build_minimal_aarch64_so(), 0x6fffe000, 0x6fffe001, &[600]);
    set_dynamic(&mut bytes, 9, 0x6fffe003, 4);
    set_dynamic(&mut bytes, 10, 0, 0);
    let error = GuestModule::from_bytes(bytes, &HashMap::new())
        .err()
        .unwrap();
    assert!(error.to_string().contains("entry size is not 8"));
}
