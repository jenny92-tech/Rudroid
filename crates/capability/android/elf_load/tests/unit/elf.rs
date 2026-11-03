//! INPUT:  private items of `src/elf.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/elf.rs`; declared there with `#[path]`
use super::*;

#[test]
fn rejects_non_elf_and_wrong_machine() {
    assert_eq!(ElfView::parse(b"").unwrap_err(), ElfError::TooSmall);
    assert_eq!(ElfView::parse(&[0u8; 64]).unwrap_err(), ElfError::BadMagic);
    // 32-bit class byte.
    let mut bytes = vec![0u8; 64];
    bytes[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    bytes[4] = 1;
    assert_eq!(ElfView::parse(&bytes).unwrap_err(), ElfError::Not64Bit);
}
