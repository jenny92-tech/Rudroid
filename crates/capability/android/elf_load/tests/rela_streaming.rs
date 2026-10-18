//! INPUT: production ELF parser, relocation pipeline and synthetic ELF bytes
//! OUTPUT: allocation, DT_RELA/PLT ordering and whole-range validation regressions
//! POS: verifies constant-storage relocation decoding on the production path

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use android_elf_load::{elf::ElfView, fixture::build_minimal_aarch64_so, GuestModule};

struct Allocator;
thread_local! { static ALLOCS: Cell<Option<usize>> = const { Cell::new(None) }; }
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|count| { if let Some(n) = count.get() { count.set(Some(n + 1)); } });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|count| { if let Some(n) = count.get() { count.set(Some(n + 1)); } });
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn put64(bytes: &mut [u8], at: usize, value: u64) { bytes[at..at + 8].copy_from_slice(&value.to_le_bytes()); }
fn dynamic(bytes: &mut [u8], index: usize, tag: u64, value: u64) {
    put64(bytes, 176 + index * 16, tag);
    put64(bytes, 184 + index * 16, value);
}

#[test]
fn large_rela_table_decodes_without_allocating() {
    const COUNT: usize = 32_768;
    let mut bytes = build_minimal_aarch64_so();
    bytes.resize(640 + COUNT * 24, 0);
    let size = bytes.len() as u64;
    put64(&mut bytes, 64 + 32, size);
    put64(&mut bytes, 64 + 40, size);
    dynamic(&mut bytes, 4, 7, 640);
    dynamic(&mut bytes, 5, 8, COUNT as u64 * 24);
    for index in 0..COUNT {
        put64(&mut bytes, 640 + index * 24, index as u64 * 8);
        put64(&mut bytes, 648 + index * 24, (2u64 << 32) | 257);
        put64(&mut bytes, 656 + index * 24, (-7i64) as u64);
    }
    let view = ElfView::parse(&bytes).unwrap();
    ALLOCS.with(|count| count.set(Some(0)));
    let mut seen = 0;
    for (offset, kind, symbol, addend) in view.rela_iter().unwrap() {
        assert_eq!((offset, kind, symbol, addend), (seen * 8, 257, 2, -7));
        seen += 1;
    }
    let allocated = ALLOCS.with(|count| count.replace(None).unwrap());
    assert_eq!(seen, COUNT as u64);
    assert_eq!(allocated, 0, "RELA decode must not allocate proportional to the table");
}

fn with_plt(addr: u64) -> Vec<u8> {
    let mut bytes = build_minimal_aarch64_so();
    dynamic(&mut bytes, 7, 23, addr);
    dynamic(&mut bytes, 8, 2, 24);
    dynamic(&mut bytes, 9, 0, 0);
    put64(&mut bytes, 576, 512);
    put64(&mut bytes, 584, 1027);
    put64(&mut bytes, 592, 303);
    bytes
}

#[test]
fn plt_entries_follow_regular_relocations_in_the_production_pipeline() {
    let bytes = with_plt(576);
    let view = ElfView::parse(&bytes).unwrap();
    assert_eq!(view.rela_iter().unwrap().collect::<Vec<_>>(), view.rela_entries().unwrap());
    let entries: Vec<_> = view.rela_iter().unwrap().collect();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[2], (512, 1027, 0, 303));
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).unwrap();
    // SAFETY: the fixture's target is an eight-byte word inside its PT_LOAD.
    let actual = unsafe { ((module.base() + 512) as *const usize).read_unaligned() };
    assert_eq!(actual, module.base() + 303, "PLT is applied after DT_RELA");
}

#[test]
fn invalid_plt_range_is_rejected_before_a_regular_iterator_is_returned() {
    let bytes = with_plt(1000);
    let view = ElfView::parse(&bytes).unwrap();
    assert!(view.rela_iter().is_err());
}
