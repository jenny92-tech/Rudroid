//! INPUT:  private items of `src/relocations.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/relocations.rs`; declared there with `#[path]`
use super::*;

#[test]
fn written_span_covers_complete_relocated_words_and_ignores_none() {
    let mut raw = crate::fixture::build_minimal_aarch64_so();
    let view = ElfView::parse(&raw).unwrap();
    let mut image = raw.clone();
    // SAFETY: the initialized owned buffer covers the fixture's PT_LOAD;
    // weak undefined symbols resolve to zero and no guest code executes.
    let writes = unsafe { apply(&view, 0x10000, image.as_mut_ptr(), image.len(), &|_, _| None) }.unwrap();
    assert!(!writes.intersects(0, 520));
    assert!(writes.intersects(520, 521));
    assert!(writes.intersects(535, 536));
    assert!(!writes.intersects(536, 640));
    drop(view);
    // R_AARCH64_NONE does not write or require a symbol resolver.
    raw[536..544].fill(0);
    raw[560..568].fill(0);
    let view = ElfView::parse(&raw).unwrap();
    // SAFETY: same initialized exclusive buffer; NONE entries make no access.
    let none = unsafe { apply(&view, 0x10000, image.as_mut_ptr(), image.len(), &|_, _| None) }.unwrap();
    assert!(!none.intersects(0, u64::MAX));
}

#[test]
fn written_span_includes_relr_address_and_bitmap_writes() {
    let mut raw = crate::fixture::build_minimal_aarch64_so();
    raw[264..272].fill(0); // no ordinary RELA entries
    raw[576..584].copy_from_slice(&600u64.to_le_bytes());
    raw[584..592].copy_from_slice(&3u64.to_le_bytes()); // bitmap -> word at 608
    let mut view = ElfView::parse(&raw).unwrap();
    view.relr = Some((576, 16));
    let mut image = raw.clone();
    // SAFETY: both decoded target words are inside the initialized buffer
    // and its PT_LOAD coverage; no guest function is called.
    let writes = unsafe { apply(&view, 0x10000, image.as_mut_ptr(), image.len(), &|_, _| None) }.unwrap();
    assert!(!writes.intersects(0, 600));
    assert!(writes.intersects(600, 601));
    assert!(writes.intersects(615, 616));
    assert!(!writes.intersects(616, 640));
}

fn decode(entries: &[u64]) -> Result<Vec<u64>, (u64, &'static str)> {
    let mut raw = Vec::new();
    for e in entries {
        raw.extend_from_slice(&e.to_le_bytes());
    }
    RelrDecoder::new(&raw).collect()
}

/// A lone even entry is the vaddr of one relocated word.
#[test]
fn single_address_entry_relocates_one_word() {
    assert_eq!(decode(&[0x1000]).unwrap(), vec![0x1000]);
}

/// The reference decode: after an address entry the cursor sits one word
/// past it; odd entries are bitmaps where bit i covers cursor + i*8
/// (0x1000, then bits 0 and 2 of 0b101 -> 0x1008 and 0x1018).
#[test]
fn bitmap_bits_cover_the_words_after_the_address_entry() {
    assert_eq!(
        decode(&[0x1000, 0b1011]).unwrap(),
        vec![0x1000, 0x1008, 0x1018]
    );
}

/// A full bitmap advances the run by 63 words; the next bitmap continues
/// from there (no overlap, no gap) — `8 * sizeof(Elf_Relr) - 1` in the
/// reference. With start = addr+8: the bitmap covers addr+8..addr+504,
/// and the run resumes at addr+8+63*8 = addr+512.
#[test]
fn consecutive_bitmaps_continue_the_run() {
    let entries = [0x1000u64, u64::MAX, 0b11];
    let targets = decode(&entries).unwrap();
    assert_eq!(targets.len(), 65);
    assert_eq!(targets[0], 0x1000);
    assert_eq!(targets[1], 0x1008);
    assert_eq!(targets[63], 0x1008 + 62 * 8);
    assert_eq!(targets[64], 0x1000 + 64 * 8);
}

/// A bitmap before any address entry is malformed; the reference would
/// relocate at base+0 here, this decoder rejects instead.
#[test]
fn leading_bitmap_entry_is_rejected() {
    assert_eq!(
        decode(&[0b101]).unwrap_err(),
        (0b101, "bitmap entry appears before any address entry")
    );
}

#[test]
fn overflowing_address_entry_is_rejected() {
    assert_eq!(
        decode(&[u64::MAX - 1]).unwrap_err(),
        (u64::MAX - 1, "address entry overflows")
    );
}
