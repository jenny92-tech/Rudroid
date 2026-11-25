//! INPUT:  private items of `src/mapped_memory.rs` via `super`
//! OUTPUT:  owned-read, empty-read and checked-overflow tests
//! POS:    keeps kernel-mediated memory helper tests out of the source file
use super::*;

#[test]
fn reads_owned_memory_without_a_pointer_dereference() {
    let source = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let mut output = [0u8; 8];
    let memory = MappedMemory::open().expect("open /proc/self/mem");
    memory
        .read_exact(source.as_ptr() as usize, &mut output)
        .expect("read initialized owned storage");
    assert_eq!(output, source);
}

#[test]
fn empty_read_is_a_noop_and_overflow_is_checked() {
    let memory = MappedMemory::open().expect("open /proc/self/mem");
    memory.read_exact(1, &mut []).expect("empty read");
    let error = memory
        .read_exact(usize::MAX, &mut [0u8; 2])
        .expect_err("overflowing range must fail before a read");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}
