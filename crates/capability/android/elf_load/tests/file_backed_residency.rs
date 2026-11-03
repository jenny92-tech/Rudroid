//! INPUT:  synthetic AArch64 ELF images, executable filesystem and Linux smaps
//! OUTPUT:  native cold-code residency, tail-zeroing and text/data relocation regressions
//! POS:    verifies demand paging without weakening cache maintenance for modified code

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

use android_elf_load::{fixture, ExecutableModule};
use std::collections::HashMap;
use std::path::PathBuf;

const CODE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
enum Layout { Clean, PartialTail, TextRelocations, DataRelocations }

fn image(layout: Layout) -> Vec<u8> {
    let mut bytes = fixture::build_minimal_aarch64_so();
    bytes.resize(CODE_BYTES, 0x5a);
    bytes[68..72].copy_from_slice(&5u32.to_le_bytes()); // RX PT_LOAD
    bytes[96..104].copy_from_slice(&(CODE_BYTES as u64).to_le_bytes());
    bytes[104..112].copy_from_slice(&(CODE_BYTES as u64).to_le_bytes());
    bytes[512..516].copy_from_slice(&0x5280_0540u32.to_le_bytes()); // mov w0, #42
    bytes[516..520].copy_from_slice(&0xd65f_03c0u32.to_le_bytes()); // ret
    match layout {
        Layout::Clean | Layout::PartialTail => {
            bytes[264..272].copy_from_slice(&0u64.to_le_bytes()); // DT_RELASZ
            if matches!(layout, Layout::PartialTail) {
                bytes[96..104].copy_from_slice(&((CODE_BYTES - 13) as u64).to_le_bytes());
            }
        }
        Layout::TextRelocations | Layout::DataRelocations => {
            let slot = if matches!(layout, Layout::DataRelocations) {
                bytes.resize(CODE_BYTES + 4096, 0);
                // Move the program-header table into a separate RW PT_LOAD;
                // dynamic tables stay in the first, executable segment.
                let headers = bytes[64..176].to_vec();
                let phoff = CODE_BYTES + 64;
                bytes[phoff..phoff + headers.len()].copy_from_slice(&headers);
                bytes[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
                bytes[56..58].copy_from_slice(&3u16.to_le_bytes());
                let third = phoff + 112;
                bytes[third..third + 4].copy_from_slice(&1u32.to_le_bytes());
                bytes[third + 4..third + 8].copy_from_slice(&6u32.to_le_bytes());
                bytes[third + 8..third + 16].copy_from_slice(&(CODE_BYTES as u64).to_le_bytes());
                bytes[third + 16..third + 24].copy_from_slice(&(CODE_BYTES as u64).to_le_bytes());
                bytes[third + 32..third + 40].copy_from_slice(&4096u64.to_le_bytes());
                bytes[third + 40..third + 48].copy_from_slice(&4096u64.to_le_bytes());
                CODE_BYTES + 512
            } else { 1024 };
            bytes[528..536].copy_from_slice(&(slot as u64).to_le_bytes());
            bytes[552..560].copy_from_slice(&((slot + 8) as u64).to_le_bytes());
        }
    }
    bytes
}

fn executable_rss(base: usize) -> usize {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    let mut selected = false;
    let mut rss = 0;
    for line in smaps.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first().is_some_and(|v| v.contains('-')) {
            let (lo, hi) = fields[0].split_once('-').unwrap();
            let lo = usize::from_str_radix(lo, 16).unwrap();
            let hi = usize::from_str_radix(hi, 16).unwrap();
            selected = lo < base + CODE_BYTES && hi > base && fields[1].contains('x');
            if selected { assert!(!fields[1].contains('w'), "native code must remain W^X"); }
        } else if selected && fields.first() == Some(&"Rss:") {
            rss += fields[1].parse::<usize>().unwrap();
        }
    }
    rss
}

fn check(layout: Layout) {
    let directory = std::env::var_os("RUDROID_EXEC_FIXTURE_DIR")
        .map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = directory.join(format!("rudroid-cold-code-{}-{layout:?}.so", std::process::id()));
    assert!(!path.exists(), "never overwrite an existing fixture");
    std::fs::write(&path, image(layout)).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert!(!rudroid_common::filesystem::FilesystemStats::for_file(&file).unwrap().noexec,
        "this test requires an executable filesystem");
    drop(file);
    // SAFETY: synthetic, freestanding code with exact answer(void) ABI;
    // its backing file stays immutable and no guest thread is created.
    let module = unsafe { ExecutableModule::map(&path, &HashMap::new()) }.unwrap();
    let before = executable_rss(module.base());
    if matches!(layout, Layout::TextRelocations) {
        assert!(before >= CODE_BYTES / 2048,
            "modified executable segments retain full explicit cache maintenance: {before} KiB");
    } else {
        assert!(before < 4096,
            "clean 16 MiB code must remain demand paged: {layout:?} RSS={before} KiB");
    }
    let address = module.symbol(fixture::ANSWER_SYMBOL).unwrap();
    // SAFETY: trusted AArch64 mov/ret function, immutable live RX mapping.
    let answer: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(address) };
    assert_eq!(unsafe { answer() }, 42);
    if matches!(layout, Layout::PartialTail) {
        // SAFETY: the last 13 bytes are inside the live, readable image.
        let tail = unsafe { std::slice::from_raw_parts((module.base() + CODE_BYTES - 13) as *const u8, 13) };
        assert_eq!(tail, &[0; 13]);
        assert_eq!(std::fs::read(&path).unwrap().last(), Some(&0x5a), "tail zeroing stays COW");
    }
    if matches!(layout, Layout::TextRelocations | Layout::DataRelocations) {
        let slot = if matches!(layout, Layout::DataRelocations) { CODE_BYTES + 512 } else { 1024 };
        // SAFETY: aligned initialized relocation slots in the retained image.
        let words = unsafe { std::slice::from_raw_parts((module.base() + slot) as *const usize, 2) };
        assert_eq!(words, &[module.base() + 0x100, 0]);
    }
    println!("COLD_CODE layout={layout:?} bytes={CODE_BYTES} initial_code_rss_kib={before} queried_code_rss_kib={}", executable_rss(module.base()));
    drop(module);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn clean_file_code_stays_demand_paged_and_executes() { check(Layout::Clean); }

#[test]
fn partial_executable_tail_stays_zeroed_without_prefaulting_cold_code() { check(Layout::PartialTail); }

#[test]
fn text_relocations_keep_full_cache_maintenance_and_execute() { check(Layout::TextRelocations); }

#[test]
fn data_relocations_do_not_prefault_unmodified_executable_code() { check(Layout::DataRelocations); }
