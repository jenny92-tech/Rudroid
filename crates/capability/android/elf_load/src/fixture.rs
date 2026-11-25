//! INPUT: synthetic segment and symbol definitions
//! OUTPUT: ELF fixture bytes for loader tests
//! POS: feature-gated cross-crate test support
use super::elf::{EM_AARCH64, PT_DYNAMIC, PT_LOAD};
use super::hash;

const EHDR: usize = 0;
const PHDR: usize = 64; // 2 * 56 bytes
const DYNAMIC: usize = 176; // 10 entries * 16
const DYNSTR: usize = 336;
const DYNSYM: usize = 384; // 3 entries * 24
const GNU_HASH: usize = 456;
const DATA: usize = 512; // answer slot + import slot
const RELA: usize = 528; // 2 entries * 24
const IMAGE_SIZE: usize = 640;

pub const ANSWER_SYMBOL: &str = "rudroid_answer";

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

/// Build the fixture image: one RW LOAD segment, PT_DYNAMIC, GNU hash,
/// two dynsyms and two RELA entries (RELATIVE +0x100, weak ABS64).
pub fn build_minimal_aarch64_so() -> Vec<u8> {
    let mut image = vec![0u8; IMAGE_SIZE];

    image[EHDR..EHDR + 4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    image[4] = 2; // ELFCLASS64
    image[5] = 1; // little-endian
    image[6] = 1; // EV_CURRENT
    image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    image[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
    image[32..40].copy_from_slice(&(PHDR as u64).to_le_bytes()); // e_phoff
    image[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    image[56..58].copy_from_slice(&2u16.to_le_bytes()); // e_phnum

    let mut ph = |base: usize, p_type: u32, vaddr: u64, filesz: u64| {
        image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
        image[base + 8..base + 16].copy_from_slice(&0u64.to_le_bytes()); // offset == vaddr
        image[base + 16..base + 24].copy_from_slice(&vaddr.to_le_bytes());
        image[base + 32..base + 40].copy_from_slice(&filesz.to_le_bytes());
        image[base + 40..base + 48].copy_from_slice(&filesz.to_le_bytes());
    };
    ph(PHDR, PT_LOAD, 0, IMAGE_SIZE as u64);
    ph(PHDR + 56, PT_DYNAMIC, DYNAMIC as u64, 10 * 16);

    let dyn_entries: Vec<(u64, u64)> = vec![
        (6, DYNSYM as u64),            // DT_SYMTAB
        (5, DYNSTR as u64),            // DT_STRTAB
        (10, 31),                      // DT_STRSZ
        (0x6ffffef5, GNU_HASH as u64), // DT_GNU_HASH
        (7, RELA as u64),              // DT_RELA
        (8, 48),                       // DT_RELASZ
        (11, 24),                      // DT_SYMENT
        (0, 0),                        // DT_NULL
    ];
    for (index, (tag, value)) in dyn_entries.iter().enumerate() {
        let at = DYNAMIC + index * 16;
        image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }

    let strtab = str_table();
    image[DYNSTR..DYNSTR + strtab.len()].copy_from_slice(&strtab);

    let mut write_sym = |at: usize, name: u32, info: u8, shndx: u16, value: u64| {
        image[at..at + 4].copy_from_slice(&name.to_le_bytes());
        image[at + 4] = info;
        image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    };
    write_sym(DYNSYM, 0, 0, 0, 0);
    write_sym(DYNSYM + 24, 1, 0x12, 1, DATA as u64); // GLOBAL, defined
    write_sym(DYNSYM + 48, 16, 0x22, 0, 0); // WEAK, undefined

    let h_answer = hash::gnu_hash(ANSWER_SYMBOL);
    image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 12..GNU_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
    image[GNU_HASH + 16..GNU_HASH + 24]
        .copy_from_slice(&(1u64 << (h_answer % 64)).to_le_bytes());
    image[GNU_HASH + 24..GNU_HASH + 28].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 28..GNU_HASH + 32].copy_from_slice(&((h_answer | 1) as u32).to_le_bytes());

    image[RELA..RELA + 8].copy_from_slice(&((DATA + 8) as u64).to_le_bytes());
    image[RELA + 8..RELA + 16].copy_from_slice(&((0u64 << 32) | 1027).to_le_bytes()); // RELATIVE
    image[RELA + 16..RELA + 24].copy_from_slice(&0x100i64.to_le_bytes());

    image[RELA + 24..RELA + 32].copy_from_slice(&((DATA + 16) as u64).to_le_bytes());
    image[RELA + 32..RELA + 40].copy_from_slice(&((2u64 << 32) | 257).to_le_bytes()); // ABS64
    image[RELA + 40..RELA + 48].copy_from_slice(&0i64.to_le_bytes());

    image
}

/// Ticket 85 rework failure-injection fixture: the minimal image with
/// `rudroid_import` flipped from WEAK-undefined (resolves to 0) to
/// GLOBAL-undefined. Relocation then fails with a structured
/// `MissingSymbol` AFTER both mappings exist — the probe for the
/// single-owner cleanup path (`exec.rs`): no metadata or region mapping
/// may survive a failed load, and nothing may be unmapped twice.
pub fn build_missing_symbol_aarch64_so() -> Vec<u8> {
    let mut image = build_minimal_aarch64_so();
    // ST_BIND(Global)=1 << 4 | STT(Object)=2 → 0x12 (the defined
    // `rudroid_answer` uses the same binding); shndx stays 0 (undefined).
    image[DYNSYM + 48 + 4] = 0x12;
    image
}

/// Ticket 85 rework rejection fixture: two PT_LOADs sharing ONE page —
/// executable [0x0,0x400) and writable [0x400,0x440) both land in page
/// 0 (4096 bytes). Real linker output separates segments page-apart;
/// this layout cannot honor W^X without either RWX or losing +x on the
/// code half, so `exec.rs` must reject it structurally instead of
/// reporting a load whose code then faults.
pub fn build_overlapping_wx_aarch64_so() -> Vec<u8> {
    // Body layout with room for 3 program headers (64 + 3*56 = 232):
    const PHDR: usize = 64;
    const DYNAMIC: usize = 232; // 10 entries * 16 → ends 392
    const DYNSTR: usize = 392;
    const DYNSYM: usize = 424; // 3 * 24 → ends 496
    const GNU_HASH: usize = 496;
    const DATA: usize = 528;
    const RELA: usize = 560; // 2 * 24 → ends 608
    const LOAD2: usize = 0x400; // writable segment start, same page
    const IMAGE_SIZE: usize = 0x440;
    const PF_R: u32 = 4;
    const PF_W: u32 = 2;
    const PF_X: u32 = 1;

    let mut image = vec![0u8; IMAGE_SIZE];
    image[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    image[4] = 2; // ELFCLASS64
    image[5] = 1; // little-endian
    image[6] = 1; // EV_CURRENT
    image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    image[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
    image[32..40].copy_from_slice(&(PHDR as u64).to_le_bytes()); // e_phoff
    image[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    image[56..58].copy_from_slice(&3u16.to_le_bytes()); // e_phnum

    let mut ph = |base: usize, p_type: u32, flags: u32, off: u64, vaddr: u64, filesz: u64| {
        image[base..base + 4].copy_from_slice(&p_type.to_le_bytes());
        image[base + 4..base + 8].copy_from_slice(&flags.to_le_bytes());
        image[base + 8..base + 16].copy_from_slice(&off.to_le_bytes());
        image[base + 16..base + 24].copy_from_slice(&vaddr.to_le_bytes());
        image[base + 32..base + 40].copy_from_slice(&filesz.to_le_bytes());
        image[base + 40..base + 48].copy_from_slice(&filesz.to_le_bytes());
    };
    // R|X covers page 0's first 1024 bytes; R|W starts at 1024 — same
    // 4096-byte page, so the two final protections cannot both hold.
    ph(PHDR, PT_LOAD, PF_R | PF_X, 0, 0, LOAD2 as u64);
    ph(
        PHDR + 56,
        PT_LOAD,
        PF_R | PF_W,
        LOAD2 as u64,
        LOAD2 as u64,
        (IMAGE_SIZE - LOAD2) as u64,
    );
    ph(
        PHDR + 112,
        PT_DYNAMIC,
        PF_R,
        DYNAMIC as u64,
        DYNAMIC as u64,
        10 * 16,
    );

    let dyn_entries: Vec<(u64, u64)> = vec![
        (6, DYNSYM as u64),            // DT_SYMTAB
        (5, DYNSTR as u64),            // DT_STRTAB
        (10, 31),                      // DT_STRSZ
        (0x6ffffef5, GNU_HASH as u64), // DT_GNU_HASH
        (7, RELA as u64),              // DT_RELA
        (8, 48),                       // DT_RELASZ
        (11, 24),                      // DT_SYMENT
        (0, 0),                        // DT_NULL
    ];
    for (index, (tag, value)) in dyn_entries.iter().enumerate() {
        let at = DYNAMIC + index * 16;
        image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }

    let strtab = str_table();
    image[DYNSTR..DYNSTR + strtab.len()].copy_from_slice(&strtab);

    let mut write_sym = |at: usize, name: u32, info: u8, shndx: u16, value: u64| {
        image[at..at + 4].copy_from_slice(&name.to_le_bytes());
        image[at + 4] = info;
        image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    };
    write_sym(DYNSYM, 0, 0, 0, 0);
    write_sym(DYNSYM + 24, 1, 0x12, 1, DATA as u64); // GLOBAL, defined
    write_sym(DYNSYM + 48, 16, 0x22, 0, 0); // WEAK, undefined

    let h_answer = hash::gnu_hash(ANSWER_SYMBOL);
    image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 12..GNU_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
    image[GNU_HASH + 16..GNU_HASH + 24]
        .copy_from_slice(&(1u64 << (h_answer % 64)).to_le_bytes());
    image[GNU_HASH + 24..GNU_HASH + 28].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 28..GNU_HASH + 32].copy_from_slice(&((h_answer | 1) as u32).to_le_bytes());

    image[RELA..RELA + 8].copy_from_slice(&((DATA + 8) as u64).to_le_bytes());
    image[RELA + 8..RELA + 16].copy_from_slice(&((0u64 << 32) | 1027).to_le_bytes()); // RELATIVE
    image[RELA + 16..RELA + 24].copy_from_slice(&0x100i64.to_le_bytes());
    image[RELA + 24..RELA + 32].copy_from_slice(&((DATA + 16) as u64).to_le_bytes());
    image[RELA + 32..RELA + 40].copy_from_slice(&((2u64 << 32) | 257).to_le_bytes()); // ABS64 (weak → 0)
    image[RELA + 40..RELA + 48].copy_from_slice(&0i64.to_le_bytes());

    // Distinguishable bytes in the writable segment (the load must fail
    // before any of this is mapped).
    image[LOAD2..LOAD2 + 4].copy_from_slice(&0xB16D_B16Du32.to_le_bytes());
    image
}

/// Modes for [`build_versioned_aarch64_so`] (ticket 86).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionMode {
    Strong,
    Weak,
    Baseline,
}

/// Ticket 86 rework: the minimal image extended with GNU symbol
/// versioning — DT_VERSYM/DT_VERNEED/DT_VERNEEDNUM, version string
/// `TESTVER_1.0`, requiring file `libc.so.6`, and `rudroid_import`
/// carrying version index 2. `mode` selects the negative cases:
/// - `Strong`: versioned strong undefined import → the load must be
///   REFUSED with a structured diagnostic (never bound by bare name).
/// - `Weak`: versioned weak undefined import → binds 0, load succeeds.
/// - `Baseline`: VERSYM index 1 (`*global*`, no requirement) → normal
///   resolution path, unaffected by the version gate.
pub fn build_versioned_aarch64_so(mode: VersionMode) -> Vec<u8> {
    let mut image = build_minimal_aarch64_so();
    const VERSTR: usize = 576; // "TESTVER_1.0\0" (12) at file vaddr
    const FILESTR: usize = 588; // "libc.so.6\0" (10) at file vaddr
                                // VERNEED/VERNAUX name fields are DYNSTR OFFSETS (relative to
                                // DT_STRTAB at 336), not absolute vaddrs — mirrors dynsym st_name.
    const VERSTR_OFF: u32 = (VERSTR - DYNSTR) as u32; // 240
    const FILESTR_OFF: u32 = (FILESTR - DYNSTR) as u32; // 252
    const VERSYM: usize = 598; // 3 * u16
    const VERNEED: usize = 604; // Verneed (16)
    const VERNAUX: usize = 620; // Vernaux (16)
    assert_eq!(image.len(), 640);
    image[VERSTR..VERSTR + 12].copy_from_slice(b"TESTVER_1.0\0");
    image[FILESTR..FILESTR + 10].copy_from_slice(b"libc.so.6\0");
    // VERSYM per dynsym: [0]=LOCAL, [1]=rudroid_answer (global baseline),
    // [2]=rudroid_import (version index 2, or baseline in Baseline mode).
    let import_versym: u16 = match mode {
        VersionMode::Baseline => 1,
        _ => 2,
    };
    image[VERSYM..VERSYM + 2].copy_from_slice(&0u16.to_le_bytes());
    image[VERSYM + 2..VERSYM + 4].copy_from_slice(&1u16.to_le_bytes());
    image[VERSYM + 4..VERSYM + 6].copy_from_slice(&import_versym.to_le_bytes());
    // Verneed: version=1, cnt=1, file=FILESTR, aux=16, next=0.
    image[VERNEED..VERNEED + 2].copy_from_slice(&1u16.to_le_bytes());
    image[VERNEED + 2..VERNEED + 4].copy_from_slice(&1u16.to_le_bytes());
    image[VERNEED + 4..VERNEED + 8].copy_from_slice(&FILESTR_OFF.to_le_bytes());
    image[VERNEED + 8..VERNEED + 12].copy_from_slice(&16u32.to_le_bytes());
    image[VERNEED + 12..VERNEED + 16].copy_from_slice(&0u32.to_le_bytes());
    // Vernaux: hash, flags=0, other=2, name=VERSTR, next=0.
    let vhash = hash::sysv_hash("TESTVER_1.0");
    image[VERNAUX..VERNAUX + 4].copy_from_slice(&vhash.to_le_bytes());
    image[VERNAUX + 4..VERNAUX + 6].copy_from_slice(&0u16.to_le_bytes());
    image[VERNAUX + 6..VERNAUX + 8].copy_from_slice(&2u16.to_le_bytes());
    image[VERNAUX + 8..VERNAUX + 12].copy_from_slice(&VERSTR_OFF.to_le_bytes());
    image[VERNAUX + 12..VERNAUX + 16].copy_from_slice(&0u32.to_le_bytes());
    // Dynamic table has 10 slots, 8 used (index 7 is DT_NULL): retarget
    // 7/8/9 at the version tables. STRSZ (slot 2) must cover the new
    // version strings: VERSTR+12 = 588 ≤ DYNSTR+STRSZ → STRSZ = 262.
    let dyn_at = |slot: usize| DYNAMIC + slot * 16;
    image[dyn_at(2) + 8..dyn_at(2) + 16].copy_from_slice(&262u64.to_le_bytes());
    image[dyn_at(7)..dyn_at(7) + 8].copy_from_slice(&super::elf::DT_VERSYM.to_le_bytes());
    image[dyn_at(7) + 8..dyn_at(7) + 16].copy_from_slice(&(VERSYM as u64).to_le_bytes());
    image[dyn_at(8)..dyn_at(8) + 8].copy_from_slice(&super::elf::DT_VERNEED.to_le_bytes());
    image[dyn_at(8) + 8..dyn_at(8) + 16].copy_from_slice(&(VERNEED as u64).to_le_bytes());
    image[dyn_at(9)..dyn_at(9) + 8].copy_from_slice(&super::elf::DT_VERNEEDNUM.to_le_bytes());
    image[dyn_at(9) + 8..dyn_at(9) + 16].copy_from_slice(&1u64.to_le_bytes());
    // The stock import is WEAK-undefined (0x22); Strong mode flips it
    // to GLOBAL-undefined (0x12) so the gate's refusal path runs.
    if matches!(mode, VersionMode::Strong) {
        image[DYNSYM + 48 + 4] = 0x12;
    }
    image
}

/// A normal import-bearing DSO whose single SYSV bucket reaches its
/// undefined `rudroid_import@TESTVER_1.0`. It has VERNEED but deliberately
/// no VERDEF, so definition-only version validation must not run for this
/// non-export candidate.
pub fn build_sysv_versioned_consumer_aarch64_so() -> Vec<u8> {
    let mut image = build_versioned_aarch64_so(VersionMode::Strong);
    // SYSV layout: nbucket=1, nchain=3, bucket[0]=2, chain[0..3]=0.
    image[GNU_HASH..GNU_HASH + 24].fill(0);
    image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&3u32.to_le_bytes());
    image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&2u32.to_le_bytes());
    // The third dynamic pair is DT_GNU_HASH in the base image.
    image[DYNAMIC + 3 * 16..DYNAMIC + 3 * 16 + 8]
        .copy_from_slice(&super::elf::DT_HASH.to_le_bytes());
    image
}

/// Synthetic provider for versioned guest-export tests. The module
/// defines `rudroid_import` in a SYSV hash table, optionally publishes
/// `DT_SONAME`, and optionally records `version` as the symbol's VERDEF
/// definition (VERSYM index 2). The caller can mutate the returned image
/// to exercise wrong-name, wrong-hash, malformed-boundary and bare-export
/// cases without touching a real game library.
pub fn build_versioned_provider_aarch64_so(
    version: Option<&str>,
    soname: Option<&str>,
) -> Vec<u8> {
    let mut image = build_minimal_aarch64_so();
    const VERSYM: usize = 488; // 3 * u16 in the unused gap before DATA
    const STRINGS: usize = 576; // after the stock RELA table
    const IMAGE_END: usize = 640;

    // The stock symbol 2 is WEAK/UNDEF; make it a real GLOBAL definition
    // named `rudroid_import` at DATA.
    let import = DYNSYM + 48;
    image[import..import + 4].copy_from_slice(&16u32.to_le_bytes());
    image[import + 4] = 0x12;
    image[import + 6..import + 8].copy_from_slice(&1u16.to_le_bytes());
    image[import + 8..import + 16].copy_from_slice(&(DATA as u64).to_le_bytes());

    // Replace GNU hash with a compact SYSV table whose single bucket
    // walks symbol 1 then the defined import at symbol 2.
    image[GNU_HASH..GNU_HASH + 32].fill(0);
    image[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&3u32.to_le_bytes());
    image[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
    image[GNU_HASH + 16..GNU_HASH + 20].copy_from_slice(&2u32.to_le_bytes());

    let mut cursor = STRINGS;
    let version_at = version.map(|name| {
        let at = cursor;
        image[at..at + name.len()].copy_from_slice(name.as_bytes());
        image[at + name.len()] = 0;
        cursor += name.len() + 1;
        at
    });
    let soname_at = soname.map(|name| {
        let at = cursor;
        image[at..at + name.len()].copy_from_slice(name.as_bytes());
        image[at + name.len()] = 0;
        cursor += name.len() + 1;
        at
    });
    let strings_end = cursor;
    cursor = (cursor + 3) & !3;
    let verdef_at = version_at.map(|_| {
        let at = cursor;
        cursor += 20;
        at
    });
    let verdaux_at = verdef_at.map(|_| {
        let at = cursor;
        cursor += 8;
        at
    });
    assert!(
        cursor <= IMAGE_END,
        "version/name fixture exceeds its image"
    );

    let mut entries = vec![
        (super::elf::DT_SYMTAB, DYNSYM as u64),
        (super::elf::DT_STRTAB, DYNSTR as u64),
        (super::elf::DT_STRSZ, (strings_end - DYNSTR) as u64),
        (super::elf::DT_HASH, GNU_HASH as u64),
    ];
    if let Some(at) = soname_at {
        entries.push((super::elf::DT_SONAME, (at - DYNSTR) as u64));
    }
    if let (Some(version_at), Some(verdef_at), Some(verdaux_at)) =
        (version_at, verdef_at, verdaux_at)
    {
        entries.push((super::elf::DT_VERSYM, VERSYM as u64));
        entries.push((super::elf::DT_VERDEF, verdef_at as u64));
        entries.push((super::elf::DT_VERDEFNUM, 1));
        image[VERSYM..VERSYM + 2].copy_from_slice(&0u16.to_le_bytes());
        image[VERSYM + 2..VERSYM + 4].copy_from_slice(&1u16.to_le_bytes());
        image[VERSYM + 4..VERSYM + 6].copy_from_slice(&2u16.to_le_bytes());
        image[verdef_at..verdef_at + 2].copy_from_slice(&1u16.to_le_bytes());
        image[verdef_at + 2..verdef_at + 4].copy_from_slice(&0u16.to_le_bytes());
        image[verdef_at + 4..verdef_at + 6].copy_from_slice(&2u16.to_le_bytes());
        image[verdef_at + 6..verdef_at + 8].copy_from_slice(&1u16.to_le_bytes());
        image[verdef_at + 8..verdef_at + 12]
            .copy_from_slice(&hash::sysv_hash(version.as_ref().expect("version")).to_le_bytes());
        image[verdef_at + 12..verdef_at + 16].copy_from_slice(&20u32.to_le_bytes());
        image[verdef_at + 16..verdef_at + 20].copy_from_slice(&0u32.to_le_bytes());
        image[verdaux_at..verdaux_at + 4]
            .copy_from_slice(&((version_at - DYNSTR) as u32).to_le_bytes());
        image[verdaux_at + 4..verdaux_at + 8].copy_from_slice(&0u32.to_le_bytes());
    }
    entries.push((super::elf::DT_NULL, 0));
    assert!(entries.len() <= 10, "provider dynamic table overflow");
    image[DYNAMIC..DYNAMIC + 10 * 16].fill(0);
    for (slot, (tag, value)) in entries.into_iter().enumerate() {
        let at = DYNAMIC + slot * 16;
        image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }
    image
}

pub const MULTIVERSION_V1_VALUE: u64 = 520;
pub const MULTIVERSION_V2_VALUE: u64 = 528;

/// A provider exporting the same bare name twice (`rudroid_import@V1`
/// and `rudroid_import@V2`). There is intentionally no DT_SONAME: the
/// caller stages this image as `libmv.so`; provider identity does not
/// scope the version-aware symbol walk.
pub fn build_multiversion_provider_aarch64_so() -> Vec<u8> {
    let mut image = build_minimal_aarch64_so();
    const HASH: usize = 480; // four dynsyms end here
    const VERSYM: usize = 508; // 4 * u16
    const STRINGS: usize = 576;

    image[DYNSYM..HASH].fill(0);
    let mut sym = |index: usize, name: u32, info: u8, shndx: u16, value: u64| {
        let at = DYNSYM + index * 24;
        image[at..at + 4].copy_from_slice(&name.to_le_bytes());
        image[at + 4] = info;
        image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    };
    sym(0, 0, 0, 0, 0);
    sym(1, 1, 0x12, 1, DATA as u64);
    sym(2, 16, 0x12, 1, MULTIVERSION_V1_VALUE);
    sym(3, 16, 0x12, 1, MULTIVERSION_V2_VALUE);

    image[HASH..VERSYM].fill(0);
    image[HASH..HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[HASH + 4..HASH + 8].copy_from_slice(&4u32.to_le_bytes());
    image[HASH + 8..HASH + 12].copy_from_slice(&1u32.to_le_bytes());
    image[HASH + 16..HASH + 20].copy_from_slice(&2u32.to_le_bytes());
    image[HASH + 20..HASH + 24].copy_from_slice(&3u32.to_le_bytes());

    image[VERSYM..VERSYM + 2].copy_from_slice(&0u16.to_le_bytes());
    image[VERSYM + 2..VERSYM + 4].copy_from_slice(&1u16.to_le_bytes());
    image[VERSYM + 4..VERSYM + 6].copy_from_slice(&2u16.to_le_bytes());
    image[VERSYM + 6..VERSYM + 8].copy_from_slice(&3u16.to_le_bytes());

    image[STRINGS..STRINGS + 3].copy_from_slice(b"V1\0");
    image[STRINGS + 3..STRINGS + 6].copy_from_slice(b"V2\0");
    let strings_end = STRINGS + 6;
    let definitions = (strings_end + 3) & !3;
    for (index, (name_offset, value)) in [
        ((STRINGS - DYNSTR) as u32, 2u16),
        ((STRINGS + 3 - DYNSTR) as u32, 3u16),
    ]
    .into_iter()
    .enumerate()
    {
        let definition = definitions + index * 28;
        let aux = definition + 20;
        let version = if index == 0 { "V1" } else { "V2" };
        image[definition..definition + 2].copy_from_slice(&1u16.to_le_bytes());
        image[definition + 2..definition + 4].copy_from_slice(&0u16.to_le_bytes());
        image[definition + 4..definition + 6].copy_from_slice(&value.to_le_bytes());
        image[definition + 6..definition + 8].copy_from_slice(&1u16.to_le_bytes());
        image[definition + 8..definition + 12]
            .copy_from_slice(&hash::sysv_hash(version).to_le_bytes());
        image[definition + 12..definition + 16].copy_from_slice(&20u32.to_le_bytes());
        let next = if index == 0 { 28u32 } else { 0u32 };
        image[definition + 16..definition + 20].copy_from_slice(&next.to_le_bytes());
        image[aux..aux + 4].copy_from_slice(&name_offset.to_le_bytes());
        image[aux + 4..aux + 8].copy_from_slice(&0u32.to_le_bytes());
    }

    let entries = [
        (super::elf::DT_SYMTAB, DYNSYM as u64),
        (super::elf::DT_STRTAB, DYNSTR as u64),
        (super::elf::DT_STRSZ, (strings_end - DYNSTR) as u64),
        (super::elf::DT_HASH, HASH as u64),
        (super::elf::DT_VERSYM, VERSYM as u64),
        (super::elf::DT_VERDEF, definitions as u64),
        (super::elf::DT_VERDEFNUM, 2),
        (super::elf::DT_NULL, 0),
    ];
    image[DYNAMIC..DYNAMIC + 10 * 16].fill(0);
    for (slot, (tag, value)) in entries.into_iter().enumerate() {
        let at = DYNAMIC + slot * 16;
        image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }
    image
}

pub const MULTIVERSION_GNU_V1_VALUE: u64 = 544;
pub const MULTIVERSION_GNU_V2_VALUE: u64 = 552;

/// GNU-hash counterpart of [`build_multiversion_provider_aarch64_so`].
/// The same bare name has two versioned definitions in one GNU chain,
/// proving version-aware lookup walks past the first wrong node when the
/// authoritative hash table is `DT_GNU_HASH` rather than `DT_HASH`.
pub fn build_multiversion_gnu_provider_aarch64_so() -> Vec<u8> {
    let mut image = build_minimal_aarch64_so();
    const HASH: usize = 480; // four dynsyms end here
    const VERSYM: usize = 516; // after the compact GNU table
    const STRINGS: usize = 576;

    image[DYNSYM..].fill(0);
    let mut sym = |index: usize, name: u32, info: u8, shndx: u16, value: u64| {
        let at = DYNSYM + index * 24;
        image[at..at + 4].copy_from_slice(&name.to_le_bytes());
        image[at + 4] = info;
        image[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    };
    sym(0, 0, 0, 0, 0);
    sym(1, 1, 0x12, 1, DATA as u64);
    sym(2, 16, 0x12, 1, MULTIVERSION_GNU_V1_VALUE);
    sym(3, 16, 0x12, 1, MULTIVERSION_GNU_V2_VALUE);

    let h = hash::gnu_hash("rudroid_import");
    image[HASH..HASH + 4].copy_from_slice(&1u32.to_le_bytes()); // nbuckets
    image[HASH + 4..HASH + 8].copy_from_slice(&2u32.to_le_bytes()); // symoffset
    image[HASH + 8..HASH + 12].copy_from_slice(&1u32.to_le_bytes()); // bloom_size
    image[HASH + 12..HASH + 16].copy_from_slice(&0u32.to_le_bytes()); // bloom_shift
    image[HASH + 16..HASH + 24].copy_from_slice(&(1u64 << (h % 64)).to_le_bytes());
    image[HASH + 24..HASH + 28].copy_from_slice(&2u32.to_le_bytes()); // bucket
    image[HASH + 28..HASH + 32].copy_from_slice(&(h & !1).to_le_bytes()); // V1 continues
    image[HASH + 32..HASH + 36].copy_from_slice(&(h | 1).to_le_bytes()); // V2 terminator

    image[VERSYM..VERSYM + 2].copy_from_slice(&0u16.to_le_bytes());
    image[VERSYM + 2..VERSYM + 4].copy_from_slice(&1u16.to_le_bytes());
    image[VERSYM + 4..VERSYM + 6].copy_from_slice(&2u16.to_le_bytes());
    image[VERSYM + 6..VERSYM + 8].copy_from_slice(&3u16.to_le_bytes());

    image[STRINGS..STRINGS + 3].copy_from_slice(b"V1\0");
    image[STRINGS + 3..STRINGS + 6].copy_from_slice(b"V2\0");
    let strings_end = STRINGS + 6;
    let definitions = (strings_end + 3) & !3;
    for (index, (name_offset, value)) in [
        ((STRINGS - DYNSTR) as u32, 2u16),
        ((STRINGS + 3 - DYNSTR) as u32, 3u16),
    ]
    .into_iter()
    .enumerate()
    {
        let definition = definitions + index * 28;
        let aux = definition + 20;
        let version = if index == 0 { "V1" } else { "V2" };
        image[definition..definition + 2].copy_from_slice(&1u16.to_le_bytes());
        image[definition + 2..definition + 4].copy_from_slice(&0u16.to_le_bytes());
        image[definition + 4..definition + 6].copy_from_slice(&value.to_le_bytes());
        image[definition + 6..definition + 8].copy_from_slice(&1u16.to_le_bytes());
        image[definition + 8..definition + 12]
            .copy_from_slice(&hash::sysv_hash(version).to_le_bytes());
        image[definition + 12..definition + 16].copy_from_slice(&20u32.to_le_bytes());
        let next = if index == 0 { 28u32 } else { 0u32 };
        image[definition + 16..definition + 20].copy_from_slice(&next.to_le_bytes());
        image[aux..aux + 4].copy_from_slice(&name_offset.to_le_bytes());
        image[aux + 4..aux + 8].copy_from_slice(&0u32.to_le_bytes());
    }

    let entries = [
        (super::elf::DT_SYMTAB, DYNSYM as u64),
        (super::elf::DT_STRTAB, DYNSTR as u64),
        (super::elf::DT_STRSZ, (strings_end - DYNSTR) as u64),
        (super::elf::DT_GNU_HASH, HASH as u64),
        (super::elf::DT_VERSYM, VERSYM as u64),
        (super::elf::DT_VERDEF, definitions as u64),
        (super::elf::DT_VERDEFNUM, 2),
        (super::elf::DT_NULL, 0),
    ];
    image[DYNAMIC..DYNAMIC + 10 * 16].fill(0);
    for (slot, (tag, value)) in entries.into_iter().enumerate() {
        let at = DYNAMIC + slot * 16;
        image[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        image[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }
    image
}
