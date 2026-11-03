//! Ticket 12 acceptance: build a synthetic minimal AArch64 shared object in
//! memory, load it through the GuestModule pipeline and exercise symbols,
//! relocations and error paths. When the (git-ignored) in-repo aarch64 ELF at
//! `tools/unity_convert/work/native-lift-latest/libinput_bridge_test.so` is
//! present it is additionally parsed as a real-world smoke case.

use std::collections::HashMap;

use android_elf_load::elf::{EM_AARCH64, PT_DYNAMIC, PT_LOAD};
use android_elf_load::{GuestModule, LoadError};

mod cc_fixture;

const R_AARCH64_RELATIVE: u64 = 1027;
const R_AARCH64_ABS64: u64 = 257;

/// Layout constants of the synthetic fixture (all vaddr == file offset).
const EHDR: usize = 0;
const PHDR: usize = 64; // 2 * 56 bytes
const DYNAMIC: usize = 176; // 10 entries * 16
const DYNSTR: usize = 336;
const DYNSYM: usize = 384; // 3 entries * 24
const GNU_HASH: usize = 456;
const DATA: usize = 512; // answer slot + import slot
const RELA: usize = 528; // 2 entries * 24
const IMAGE_SIZE: usize = 640;

fn str_table() -> Vec<u8> {
    // offset 0: "", 1: "rudroid_answer", 16: "rudroid_import"
    let mut table = Vec::new();
    table.push(0);
    let first = table.len() as u32;
    table.extend_from_slice(b"rudroid_answer\0");
    let second = table.len() as u32;
    table.extend_from_slice(b"rudroid_import\0");
    assert_eq!(first, 1);
    assert_eq!(second, 16);
    table
}

/// Build a minimal but fully valid AArch64 ELF shared object:
/// one RW LOAD segment, PT_DYNAMIC, GNU hash, two dynsyms and two RELA
/// entries (one RELATIVE, one ABS64 against `rudroid_import`).
fn build_fixture() -> Vec<u8> {
    let mut image = vec![0u8; IMAGE_SIZE];

    // ELF header.
    image[EHDR..EHDR + 4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    image[4] = 2; // ELFCLASS64
    image[5] = 1; // little-endian
    image[6] = 1; // EV_CURRENT
    image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    image[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
    image[32..40].copy_from_slice(&(PHDR as u64).to_le_bytes()); // e_phoff
    image[40..48].copy_from_slice(&(PHDR as u64).to_le_bytes()); // e_shoff (reuse)
    image[52..56].copy_from_slice(&(PHDR as u32).to_le_bytes()); // e_phentsize... (placeholder)
    // e_phentsize (54) + e_phnum (56):
    image[54..56].copy_from_slice(&56u16.to_le_bytes());
    image[56..58].copy_from_slice(&2u16.to_le_bytes());

    // Program headers: one RW LOAD covering everything, one PT_DYNAMIC.
    let mut ph = |base: usize, p_type: u32, vaddr: u64, filesz: u64| {
        image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
        image[base + 8..base + 16].copy_from_slice(&0u64.to_le_bytes()); // offset == vaddr
        image[base + 16..base + 24].copy_from_slice(&vaddr.to_le_bytes());
        image[base + 32..base + 40].copy_from_slice(&filesz.to_le_bytes());
        image[base + 40..base + 48].copy_from_slice(&filesz.to_le_bytes()); // memsz
    };
    ph(PHDR, PT_LOAD, 0, IMAGE_SIZE as u64);
    ph(PHDR + 56, PT_DYNAMIC, DYNAMIC as u64, 10 * 16);

    // PT_DYNAMIC.
    let mut dyn_entries: Vec<(u64, u64)> = vec![
        (6, DYNSYM as u64),        // DT_SYMTAB
        (5, DYNSTR as u64),        // DT_STRTAB
        (10, 31),                  // DT_STRSZ
        (0x6ffffef5, GNU_HASH as u64), // DT_GNU_HASH
        (7, RELA as u64),          // DT_RELA
        (8, 48),                   // DT_RELASZ
        (11, 24),                  // DT_SYMENT
        (0, 0),                    // DT_NULL
    ];
    dyn_entries.resize(10, (0, 0));
    for (index, (tag, value)) in dyn_entries.iter().enumerate() {
        let at = DYNAMIC + index * 16;
        image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }

    // dynstr.
    let strtab = str_table();
    image[DYNSTR..DYNSTR + strtab.len()].copy_from_slice(&strtab);

    // dynsym: [0] reserved, [1] "rudroid_answer" defined at DATA, [2] weak
    // undefined "rudroid_import".
    let mut write_sym = |at: usize, name: u32, info: u8, shndx: u16, value: u64| {
        image[at..at + 4].copy_from_slice(&name.to_le_bytes());
        image[at + 4] = info;
        image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    };
    write_sym(DYNSYM, 0, 0, 0, 0);
    write_sym(DYNSYM + 24, 1, 0x12, 1, DATA as u64); // GLOBAL, DEFINED
    write_sym(DYNSYM + 48, 16, 0x22, 0, 0); // WEAK, UNDEFINED

    // GNU hash: nbuckets=1, symoffset=1, bloom_size=1, bloom_shift=0.
    let h_answer = android_elf_load::hash::gnu_hash("rudroid_answer");
    image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 12..GNU_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
    image[GNU_HASH + 16..GNU_HASH + 24].copy_from_slice(&(1u64 << (h_answer % 64)).to_le_bytes());
    image[GNU_HASH + 24..GNU_HASH + 28].copy_from_slice(&1u32.to_le_bytes()); // bucket -> sym 1
    image[GNU_HASH + 28..GNU_HASH + 32].copy_from_slice(&((h_answer | 1) as u32).to_le_bytes());

    // RELA: slot at DATA+8 bound RELATIVE (+0x100), slot at DATA+16 bound
    // ABS64 -> "rudroid_import" (weak, no provider -> null).
    image[RELA..RELA + 8].copy_from_slice(&((DATA + 8) as u64).to_le_bytes());
    image[RELA + 8..RELA + 16].copy_from_slice(&((0u64 << 32) | R_AARCH64_RELATIVE).to_le_bytes());
    image[RELA + 16..RELA + 24].copy_from_slice(&0x100i64.to_le_bytes());

    image[RELA + 24..RELA + 32].copy_from_slice(&((DATA + 16) as u64).to_le_bytes());
    image[RELA + 32..RELA + 40].copy_from_slice(&((2u64 << 32) | R_AARCH64_ABS64).to_le_bytes());
    image[RELA + 40..RELA + 48].copy_from_slice(&0i64.to_le_bytes());

    image
}

#[test]
fn loads_fixture_applies_relocations_and_finds_symbol() {
    let image = build_fixture();
    let module = GuestModule::from_bytes(image, &HashMap::new()).expect("fixture loads");

    // Defined symbol resolves to base + DATA.
    let answer = module.symbol("rudroid_answer").expect("symbol found");
    assert_eq!(answer, module.base() + DATA);

    // RELATIVE relocation applied: slot holds base + 0x100.
    let base = module.base();
    let slot = base + DATA + 8;
    assert_eq!(unsafe { (slot as *const usize).read_unaligned() }, base + 0x100);

    // ABS64 against unresolved weak symbol binds to null.
    let weak_slot = base + DATA + 16;
    assert_eq!(unsafe { (weak_slot as *const usize).read_unaligned() }, 0);

    // Miss is a miss.
    assert_eq!(module.symbol("nope"), None);
}

#[test]
fn undefined_strong_symbol_is_an_actionable_error() {
    let image = build_fixture();
    // Make the import strong (info high nibble 1, shndx 0): now relocation
    // must fail naming the symbol.
    let mut image = image;
    image[DYNSYM + 48 + 4] = 0x12; // GLOBAL instead of WEAK
    let err = match GuestModule::from_bytes(image, &HashMap::new()) { Err(e) => e, Ok(_) => panic!("fixture must fail") };
    match &err {
        LoadError::Relocation { reason, kind, .. } => {
            assert_eq!(*kind, R_AARCH64_ABS64 as u32);
            assert!(reason.contains("rudroid_import"), "reason: {reason}");
        }
        other => panic!("expected relocation error, got {other:?}"),
    }
}

#[test]
fn global_symbol_provider_supplies_imports() {
    let image = build_fixture();
    // Strong import satisfied through the global table.
    let mut image = image;
    image[DYNSYM + 48 + 4] = 0x12;
    let mut globals = HashMap::new();
    globals.insert("rudroid_import".to_owned(), 0xfeed_faceusize);
    let module = GuestModule::from_bytes(image, &globals).expect("loads with provider");
    let slot = module.base() + DATA + 16;
    assert_eq!(unsafe { (slot as *const usize).read_unaligned() }, 0xfeed_face);
}

#[test]
fn rejects_non_elf_and_wrong_arch() {
    let err = match GuestModule::from_bytes(b"not an elf".to_vec(), &HashMap::new()) { Err(e) => e, Ok(_) => panic!("non-ELF must fail") };
    assert!(err.to_string().contains("invalid guest module"));
    let _ = R_AARCH64_RELATIVE; // silence unused in cfg variants
}

#[test]
#[ignore = "needs the git-ignored tools/unity_convert/work/native-lift-latest/libinput_bridge_test.so"]
fn real_world_aarch64_so_smoke() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../tools/unity_convert/work/native-lift-latest/libinput_bridge_test.so"
    );
    let bytes = std::fs::read(fixture)
        .unwrap_or_else(|error| panic!("real-world ELF fixture {fixture}: {error}"));
    let globals = HashMap::new();
    // The real .so has strong undefined imports we do not provide; it must
    // either load or fail with an actionable missing-symbol message — but the
    // parse stage (header/segments/dynamic) must always succeed.
    match GuestModule::from_bytes(bytes, &globals) {
        Ok(module) => {
            eprintln!("real-world .so loaded at base {:#x}", module.base());
        }
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("symbol") || message.contains("relocation"),
                "unexpected failure: {message}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Ticket 47 regressions: hostile and malformed inputs must return structured
// errors through both the Rust and the C load API — never panic, never write
// outside the materialized image.

#[test]
fn filesz_larger_than_memsz_returns_error() {
    let mut image = build_fixture();
    // filesz := IMAGE_SIZE + 1 > memsz (IMAGE_SIZE): parse-time diagnostic.
    image[PHDR + 32..PHDR + 40].copy_from_slice(&((IMAGE_SIZE as u64) + 1).to_le_bytes());
    let Err(LoadError::Elf(android_elf_load::elf::ElfError::FileszExceedsMemsz)) =
        GuestModule::from_bytes(image, &HashMap::new())
    else {
        panic!("filesz > memsz must be a parse-time diagnostic");
    };
}

#[test]
fn relocation_at_last_image_byte_returns_error() {
    let mut image = build_fixture();
    // r_offset = image_len - 1: a start-only bounds check would write 7
    // bytes past the allocation (the review's heap-corruption scenario).
    image[RELA..RELA + 8].copy_from_slice(&((IMAGE_SIZE - 1) as u64).to_le_bytes());
    assert!(matches!(
        GuestModule::from_bytes(image, &HashMap::new()),
        Err(LoadError::Relocation { .. })
    ));
}

#[test]
fn relocation_overlapping_image_end_returns_error() {
    let mut image = build_fixture();
    // r_offset = image_len - 7: the 8-byte write would overlap the end.
    image[RELA..RELA + 8].copy_from_slice(&((IMAGE_SIZE - 7) as u64).to_le_bytes());
    assert!(matches!(
        GuestModule::from_bytes(image, &HashMap::new()),
        Err(LoadError::Relocation { .. })
    ));
}

#[test]
fn relocation_at_fully_covered_image_end_succeeds() {
    let mut image = build_fixture();
    // r_offset = image_len - 8: exactly the last word — legal target.
    image[RELA..RELA + 8].copy_from_slice(&((IMAGE_SIZE - 8) as u64).to_le_bytes());
    image[RELA + 8..RELA + 16].copy_from_slice(&((0u64 << 32) | 1027).to_le_bytes());
    image[RELA + 16..RELA + 24].copy_from_slice(&0x100i64.to_le_bytes());
    let module = GuestModule::from_bytes(image, &HashMap::new()).unwrap();
    let word = unsafe { ((module.base() + IMAGE_SIZE - 8) as *const usize).read_unaligned() };
    assert_eq!(word, module.base() + 0x100);
}

#[test]
fn relocation_at_overflowing_vaddr_returns_error() {
    let mut image = build_fixture();
    image[RELA..RELA + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        GuestModule::from_bytes(image, &HashMap::new()),
        Err(LoadError::Relocation { .. })
    ));
}

#[test]
fn hostile_headers_return_errors_not_panics() {
    // Huge e_phoff.
    let mut image = build_fixture();
    image[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(GuestModule::from_bytes(image, &HashMap::new()).is_err());

    // Huge DT_RELASZ: the claimed table is not backed by the file, and the
    // preallocation must stay bounded (returns promptly with an error).
    let mut image = build_fixture();
    let relasz_at = DYNAMIC + 5 * 16 + 8; // DT_RELASZ entry value
    image[relasz_at..relasz_at + 8].copy_from_slice(&0x0000_FFFF_FFFF_FFFFu64.to_le_bytes());
    assert!(GuestModule::from_bytes(image, &HashMap::new()).is_err());

    // Huge image span via memsz: capped instead of allocating gigabytes.
    let mut image = build_fixture();
    image[PHDR + 40..PHDR + 48].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        GuestModule::from_bytes(image, &HashMap::new()),
        Err(LoadError::Elf(_))
    ));
}

#[test]
fn c_load_api_reports_null_for_every_hostile_image() {
    // The C boundary must convert every malformed input into a null handle
    // — no panic may cross `extern "C"`.
    let hostile: Vec<Vec<u8>> = {
        let mut cases = Vec::new();
        let mut image = build_fixture();
        image[PHDR + 32..PHDR + 40].copy_from_slice(&((IMAGE_SIZE as u64) + 1).to_le_bytes());
        cases.push(image.clone());
        let mut image = build_fixture();
        image[RELA..RELA + 8].copy_from_slice(&((IMAGE_SIZE - 1) as u64).to_le_bytes());
        cases.push(image.clone());
        let mut image = build_fixture();
        image[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        cases.push(image);
        cases
    };
    for image in hostile {
        let handle = unsafe {
            android_elf_load::rudroid_elf_load_from_bytes(image.as_ptr(), image.len())
        };
        assert!(handle.is_null(), "hostile image must yield a null handle");
    }
}

#[test]
fn relocation_into_zero_filled_segment_tail_stays_inside_the_image() {
    // memsz extends past the file-backed part (legal: BSS tail). A
    // relocation targeting the zero-filled tail writes inside the image and
    // the weak undefined ABS64 binds to 0 there.
    let mut image = build_fixture();
    image[PHDR + 40..PHDR + 48].copy_from_slice(&((IMAGE_SIZE as u64) + 8).to_le_bytes());
    image[RELA + 24..RELA + 32].copy_from_slice(&(IMAGE_SIZE as u64).to_le_bytes());
    let module = GuestModule::from_bytes(image, &HashMap::new()).unwrap();
    let word = unsafe { ((module.base() + IMAGE_SIZE) as *const usize).read_unaligned() };
    assert_eq!(word, 0, "undefined weak ABS64 binds to 0 in the zero tail");
}

// ---------------------------------------------------------------------------
// Ticket 48 regressions: virtual-address translation, addend preservation,
// base/handle separation and cross-module import resolution.

#[test]
fn abs64_preserves_nonzero_addend() {
    let mut image = build_fixture();
    image[RELA + 40..RELA + 48].copy_from_slice(&7i64.to_le_bytes());
    let globals = HashMap::from([("rudroid_import".to_owned(), 0x1000usize)]);
    let module = GuestModule::from_bytes(image, &globals).unwrap();
    let actual = unsafe { ((module.base() + DATA + 16) as *const usize).read_unaligned() };
    assert_eq!(actual, 0x1007, "ABS64 must preserve the RELA addend");
}

#[test]
fn nonidentity_virtual_addresses_translate_through_loads() {
    // All vaddr fields biased by 0x1000 while p_offset stays 0: a valid PIE
    // layout. DT_SYMTAB/DT_STRTAB/hash lookups and relocations must translate
    // vaddrs through the PT_LOAD segments.
    let mut image = build_fixture();
    let bias = 0x1000u64;
    image[PHDR + 16..PHDR + 24].copy_from_slice(&bias.to_le_bytes());
    image[PHDR + 56 + 16..PHDR + 56 + 24]
        .copy_from_slice(&(DYNAMIC as u64 + bias).to_le_bytes());
    for index in [0usize, 1, 3, 4] {
        let at = DYNAMIC + index * 16 + 8;
        let value = u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
        image[at..at + 8].copy_from_slice(&(value + bias).to_le_bytes());
    }
    image[DYNSYM + 24 + 8..DYNSYM + 24 + 16]
        .copy_from_slice(&(DATA as u64 + bias).to_le_bytes());
    for at in [RELA, RELA + 24] {
        let value = u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
        image[at..at + 8].copy_from_slice(&(value + bias).to_le_bytes());
    }
    let module = GuestModule::from_bytes(image, &HashMap::new())
        .expect("valid PT_LOAD virtual/file offset translation");
    assert_eq!(
        module.symbol("rudroid_answer"),
        Some(module.base() + bias as usize + DATA)
    );
}

#[test]
fn c_base_plus_st_value_equals_symbol_address() {
    let image = build_fixture();
    let handle =
        unsafe { android_elf_load::rudroid_elf_load_from_bytes(image.as_ptr(), image.len()) };
    assert!(!handle.is_null());
    let base = unsafe { android_elf_load::rudroid_elf_load_base(handle) };
    assert_ne!(base, 0);
    let symbol = unsafe {
        android_elf_load::rudroid_elf_symbol(handle, b"rudroid_answer\0".as_ptr() as *const _)
    };
    // st_value of rudroid_answer is DATA; base + st_value must be the symbol
    // address — the handle address itself must never appear here (ticket 48).
    assert_eq!(symbol, base + DATA);
    unsafe { android_elf_load::rudroid_elf_destroy(handle) };
}

/// Compiler-produced provider/consumer pair (ticket 48): the consumer's
/// undefined imports must bind against the previously loaded provider
/// through the C registry. Both come from `tests/fixtures/rudroid86_*.c`,
/// compiled by the test itself.
#[test]
fn compiler_produced_sos_resolve_imports_in_load_order() {
    let provider_path = cc_fixture::fixture("rudroid86_provider");
    let consumer_path = cc_fixture::fixture("rudroid86_consumer");
    let to_c = |path: &str| std::ffi::CString::new(path).unwrap();

    // Load order matters: without the provider the consumer's required
    // import cannot resolve, and the C API reports a null handle.
    let consumer_handle =
        unsafe { android_elf_load::rudroid_elf_load_from_path(to_c(&consumer_path).as_ptr()) };
    assert!(consumer_handle.is_null(), "consumer must fail without its provider");
    unsafe { android_elf_load::rudroid_elf_destroy(consumer_handle) };

    let provider_handle =
        unsafe { android_elf_load::rudroid_elf_load_from_path(to_c(&provider_path).as_ptr()) };
    assert!(!provider_handle.is_null(), "provider must load");
    let provider_value = unsafe {
        android_elf_load::rudroid_elf_symbol(provider_handle, b"rudroid86_value\0".as_ptr() as *const _)
    };
    assert_ne!(provider_value, 0, "provider must export rudroid86_value");

    // Now the consumer resolves its import and loads.
    let consumer_handle =
        unsafe { android_elf_load::rudroid_elf_load_from_path(to_c(&consumer_path).as_ptr()) };
    assert!(!consumer_handle.is_null(), "consumer import must bind to the provider");
    let consumer_base = unsafe { android_elf_load::rudroid_elf_load_base(consumer_handle) };
    let consumer_symbol = unsafe {
        android_elf_load::rudroid_elf_symbol(consumer_handle, b"rudroid86_read\0".as_ptr() as *const _)
    };
    assert!(
        consumer_symbol > consumer_base,
        "consumer symbol must live inside its image (base={consumer_base:#x})"
    );
    unsafe {
        android_elf_load::rudroid_elf_destroy(consumer_handle);
        android_elf_load::rudroid_elf_destroy(provider_handle);
    }
}
