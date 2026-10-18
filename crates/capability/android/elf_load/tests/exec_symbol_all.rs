//! Reference `so_symbol_all` parity: `rudroid_elf_exec_symbol_all` tries
//! the dynamic symbols first and, on a miss, the file's section-header
//! `.symtab` — hidden functions of an unstripped library (e.g. Swappy's
//! `SwappyGL_getSwapIntervalNS`) are only reachable there. A stripped file
//! or a malformed section-header table misses (0); TLS matches are refused
//! on both paths (ticket 86).
//!
//! Fixtures are compiled from `tests/fixtures/` by the test itself with the
//! canonical container's `cc`; a missing compiler fails loudly.

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

mod cc_fixture;

use std::ffi::{c_char, CString};
use std::path::Path;

use android_elf_load::ExecutableModule;
use cc_fixture::compile;

fn load(path: &Path) -> *mut ExecutableModule {
    let c = CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: NUL-terminated path owned for the call.
    let handle = unsafe { android_elf_load::rudroid_elf_exec_load(c.as_ptr()) };
    assert!(!handle.is_null(), "exec load must succeed for {}: {}", path.display(), last_error());
    handle
}

fn symbol(handle: *const ExecutableModule, name: &str) -> usize {
    let c = CString::new(name).unwrap();
    // SAFETY: live handle; NUL-free name.
    unsafe { android_elf_load::rudroid_elf_exec_symbol(handle, c.as_ptr()) }
}

fn symbol_all(handle: *const ExecutableModule, name: &str) -> usize {
    let c = CString::new(name).unwrap();
    // SAFETY: live handle; NUL-free name.
    unsafe { android_elf_load::rudroid_elf_exec_symbol_all(handle, c.as_ptr()) }
}

fn last_error() -> String {
    let mut buffer = [0 as c_char; 512];
    // SAFETY: buffer outlives the call.
    let written = unsafe {
        android_elf_load::rudroid_elf_last_error(buffer.as_mut_ptr(), buffer.len())
    };
    if written == 0 {
        return String::new();
    }
    // SAFETY: NUL-terminated by the error channel.
    unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn call(address: usize) -> i32 {
    assert_ne!(address, 0);
    // SAFETY: the fixture defines `int f(void)` at this address.
    let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { function() }
}

/// Patch helper over a copy of an ELF file: (e_shoff, e_shnum) and the
/// section-header index of the first SHT_SYMTAB.
fn symtab_header(bytes: &[u8]) -> (usize, usize, usize) {
    let shoff = u64::from_le_bytes(bytes[0x28..0x30].try_into().unwrap()) as usize;
    let shnum = u16::from_le_bytes(bytes[0x3c..0x3e].try_into().unwrap()) as usize;
    let symtab = (0..shnum)
        .find(|i| {
            let at = shoff + i * 64 + 4;
            u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) == 2
        })
        .expect("unstripped fixture has a SHT_SYMTAB");
    (shoff, shnum, symtab)
}

#[test]
fn dynsym_miss_falls_back_to_the_symtab() {
    let module = load(&compile("rudroid_symtab_fallback", "full", &[]));
    assert_eq!(symbol(module, "rudroid_symtab_hidden"), 0, "hidden: not a dynamic export");
    let hidden = symbol_all(module, "rudroid_symtab_hidden");
    assert!(last_error().is_empty());
    assert_eq!(call(hidden), 5150, ".symtab hit is a callable address in the mapped image");
    let base = unsafe { android_elf_load::rudroid_elf_exec_load_base(module) };
    assert!(hidden > base, "same image identity as the load base");
    // Exported names keep answering from the dynamic symbols.
    let exported = symbol_all(module, "rudroid_symtab_exported");
    assert_eq!(exported, symbol(module, "rudroid_symtab_exported"));
    assert_eq!(call(exported), 5151);
    assert_eq!(symbol_all(module, "rudroid_symtab_absent"), 0);
    assert!(last_error().is_empty(), "a plain miss records no error");
}

#[test]
fn stripped_file_misses_the_symtab_only_symbol() {
    let module = load(&compile("rudroid_symtab_fallback", "stripped", &["-s"]));
    assert_eq!(symbol_all(module, "rudroid_symtab_hidden"), 0, "no .symtab after strip");
    assert!(last_error().is_empty());
    assert_eq!(call(symbol_all(module, "rudroid_symtab_exported")), 5151);
}

#[test]
fn tls_matches_are_refused_on_both_paths() {
    let module = load(&compile("rudroid_symtab_fallback", "tls", &[]));
    assert_eq!(symbol_all(module, "rudroid_symtab_tls"), 0);
    let reason = last_error();
    assert!(reason.contains("rudroid_symtab_tls") && reason.contains("TLS"), ".symtab TLS: {reason}");

    let dynamic = load(&compile("rudroid86_tls", "dyn-tls", &[]));
    assert_eq!(symbol_all(dynamic, "rudroid86_tls"), 0);
    let reason = last_error();
    assert!(reason.contains("rudroid86_tls") && reason.contains("TLS"), "dynsym TLS: {reason}");
}

#[test]
fn malformed_section_headers_miss_instead_of_reading_out_of_bounds() {
    let full = compile("rudroid_symtab_fallback", "malformed-src", &[]);
    let pristine = std::fs::read(&full).unwrap();
    let (shoff, shnum, symtab) = symtab_header(&pristine);
    let dir = full.parent().unwrap().to_owned();
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();

    let mut beyond = pristine.clone();
    let past_end = (pristine.len() as u64 + 64).to_le_bytes();
    beyond[0x28..0x30].copy_from_slice(&past_end);
    cases.push(("shoff-past-end", beyond));

    let mut bad_link = pristine.clone();
    let link_at = shoff + symtab * 64 + 40;
    bad_link[link_at..link_at + 4].copy_from_slice(&(shnum as u32).to_le_bytes());
    cases.push(("link-out-of-range", bad_link));

    let mut oversized = pristine.clone();
    let size_at = shoff + symtab * 64 + 32;
    oversized[size_at..size_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    cases.push(("symtab-size-overflow", oversized));

    for (label, bytes) in cases {
        let path = dir.join(format!("{label}.so"));
        std::fs::write(&path, &bytes).unwrap();
        let module = load(&path);
        assert_eq!(symbol_all(module, "rudroid_symtab_hidden"), 0, "{label} must miss");
        assert_eq!(call(symbol_all(module, "rudroid_symtab_exported")), 5151, "{label}: dynsym intact");
    }
}

#[test]
fn null_arguments_are_diagnosed() {
    // SAFETY: null handle/name are the diagnosed cases of the C contract.
    let missing_handle = unsafe {
        android_elf_load::rudroid_elf_exec_symbol_all(std::ptr::null(), c"x".as_ptr())
    };
    assert_eq!(missing_handle, 0);
    assert!(last_error().contains("handle is null"));
    let module = load(&compile("rudroid_symtab_fallback", "null-name", &[]));
    // SAFETY: live handle, null name.
    let missing_name =
        unsafe { android_elf_load::rudroid_elf_exec_symbol_all(module, std::ptr::null()) };
    assert_eq!(missing_name, 0);
    assert!(last_error().contains("name is null"));
}
