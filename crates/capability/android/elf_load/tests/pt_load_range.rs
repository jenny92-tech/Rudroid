//! Ticket 63 acceptance: the full 8-byte relocation write range must lie
//! inside real PT_LOAD coverage, not merely inside the image allocation.
//!
//! The review probe maps LOADs [0,640) and [4096,8192) (BSS-only) and shows
//! the old total-span check still wrote into the unmapped gap at 0x800.
//! These tests pin the merged-coverage rule:
//!
//! - a target fully inside a PT_LOAD gap, below the first segment, or
//!   overflowing u64 fails;
//! - a write whose tail crosses a segment end into a gap fails;
//! - a write ending exactly at a segment end stays legal;
//! - adjacent (and overlapping) segments are merged, so writes spanning the
//!   boundary of contiguous coverage are NOT rejected.
//!
//! Fixtures are hand-built variants of the synthetic AArch64 SO; everything
//! runs inside Docker on the buffer backend (the executable backend shares
//! the same `relocations::apply` path).

use std::collections::HashMap;

use android_elf_load::elf::PT_LOAD;
use android_elf_load::fixture::build_minimal_aarch64_so;
use android_elf_load::GuestModule;

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

/// Move every vaddr of the single-LOAD fixture up by `delta` (segment,
/// dynamic table, dynamic entries, relocation targets) so the image's memory
/// range becomes [delta, delta+640) while file offsets stay put.
fn shifted_image(delta: u64) -> Vec<u8> {
    let mut bytes = build_minimal_aarch64_so();
    // ph0 (PT_LOAD) p_vaddr.
    put64(&mut bytes, 64 + 16, delta);
    // ph1 (PT_DYNAMIC) p_vaddr (file offset 64+56, vaddr field at +16).
    put64(&mut bytes, 64 + 56 + 16, delta + 176);
    // Dynamic vaddr entries: DT_SYMTAB(384), DT_STRTAB(336), DT_GNU_HASH(456),
    // DT_RELA(528) — the vaddr stays in the value slot.
    for (index, vaddr) in [(0usize, 384u64), (1, 336), (3, 456), (4, 528)] {
        let at = 176 + index * 16 + 8;
        put64(&mut bytes, at, delta + vaddr);
    }
    // Relocation targets: entry 0 r_offset (DATA+8) and entry 1 (DATA+16).
    put64(&mut bytes, 528, delta + 512 + 8);
    put64(&mut bytes, 528 + 24, delta + 512 + 16);
    bytes
}

/// Append a BSS-only second PT_LOAD at [vaddr, vaddr+memsz) and relocate the
/// program header table to the tail of the file (like the review probe).
fn with_second_load(mut bytes: Vec<u8>, vaddr: u64, memsz: u64) -> Vec<u8> {
    let old_phdr = bytes[64..176].to_vec();
    bytes.resize(640 + 168, 0);
    bytes[640..752].copy_from_slice(&old_phdr);
    put64(&mut bytes, 32, 640); // e_phoff
    put16(&mut bytes, 56, 3); // e_phnum
    let base = 752; // third phdr
    bytes[base..base + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
    put64(&mut bytes, base + 16, vaddr);
    put64(&mut bytes, base + 40, memsz); // p_memsz; p_filesz stays 0 (BSS)
    bytes
}

fn set_relative_target(bytes: &mut [u8], r_offset: u64) {
    put64(bytes, 528, r_offset); // RELA entry 0: RELATIVE, addend 0x100
}

/// The review probe, verbatim construction: target 0x800 lies in the gap
/// between [0,640) and [4096,8192).
#[test]
fn relocation_in_unmapped_pt_load_gap_must_fail() {
    let mut bytes = with_second_load(build_minimal_aarch64_so(), 0x1000, 0x1000);
    set_relative_target(&mut bytes, 0x800);
    assert!(
        GuestModule::from_bytes(bytes, &HashMap::new()).is_err(),
        "accepted relocation into PT_LOAD gap at 0x800"
    );
}

/// The target starts inside LOAD [delta, delta+640) but its 8-byte write
/// crosses into the unmapped tail.
#[test]
fn relocation_tail_crossing_the_segment_end_must_fail() {
    let delta = 0x800u64;
    let mut bytes = shifted_image(delta);
    set_relative_target(&mut bytes, delta + 640 - 4); // write [636..644)+delta
    assert!(
        GuestModule::from_bytes(bytes, &HashMap::new()).is_err(),
        "a write crossing the segment end into unmapped space must fail"
    );
}

/// A target entirely below the first PT_LOAD is rejected.
#[test]
fn relocation_below_the_first_segment_must_fail() {
    let delta = 0x800u64;
    let mut bytes = shifted_image(delta);
    set_relative_target(&mut bytes, delta - 8); // fully below [delta, ...)
    assert!(
        GuestModule::from_bytes(bytes, &HashMap::new()).is_err(),
        "a write below the first PT_LOAD must fail"
    );
}

/// End-point arithmetic stays checked: vaddr near u64::MAX must fail, not
/// wrap into a covered range.
#[test]
fn relocation_with_overflowing_end_must_fail() {
    let mut bytes = build_minimal_aarch64_so();
    set_relative_target(&mut bytes, u64::MAX - 4);
    assert!(
        GuestModule::from_bytes(bytes, &HashMap::new()).is_err(),
        "overflowing write range must fail"
    );
}

/// A write ending exactly at the segment end is legal.
#[test]
fn relocation_at_exact_segment_end_stays_legal() {
    let delta = 0x800u64;
    let mut bytes = shifted_image(delta);
    set_relative_target(&mut bytes, delta + 640 - 8);
    let module = GuestModule::from_bytes(bytes, &HashMap::new())
        .expect("the last word of the segment is a legal target");
    let word =
        unsafe { ((module.base() + delta as usize + 632) as *const usize).read_unaligned() };
    assert_eq!(word, module.base() + 0x100);
}

/// Adjacent LOADs are merged: [0,640) file-backed + [640,1280) BSS means a
/// write spanning the 640 boundary is fully covered and must stay legal.
#[test]
fn adjacent_load_segments_are_merged_not_rejected() {
    let bytes = with_second_load(build_minimal_aarch64_so(), 640, 640);
    let mut bytes = bytes;
    set_relative_target(&mut bytes, 636); // spans [636,644) across the boundary
    let module = GuestModule::from_bytes(bytes, &HashMap::new())
        .expect("contiguous coverage must not be rejected at segment boundaries");
    let word = unsafe { ((module.base() + 636) as *const usize).read_unaligned() };
    assert_eq!(word, module.base() + 0x100);
}
