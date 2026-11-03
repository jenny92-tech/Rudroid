//! Ticket 64/67 acceptance: dynamic string reads are bounded by DT_STRSZ and
//! use the same PT_LOAD translation that parse validated.
//!
//! The old `str_at` translated DT_STRTAB and then scanned for NUL to the end
//! of the file, so a hostile DT_STRSZ never mattered: with DT_STRSZ=1 the
//! loader still accepted dynsym names living outside the declared string
//! table. These tests pin the new rules:
//!
//! - DT_STRSZ must exist, be non-zero, and DT_STRTAB+DT_STRSZ must be backed
//!   by a PT_LOAD file range (parse-time diagnostics);
//! - a string offset must be < DT_STRSZ and its NUL must appear inside the
//!   table — for dynsym names and DT_NEEDED names alike;
//! - legal tables (empty string, NUL-terminated names, non-identity vaddr
//!   layout) keep resolving.
//!
//! Ticket 67 adds: validation, reads and diagnostics must select the SAME
//! PT_LOAD/file range for the string table. The R2 review probe showed the
//! old split — parse validated the whole table through one segment while
//! `str_at` re-translated only the table start through another (overlapping)
//! segment — reading bytes that were never validated. The deterministic rule
//! under overlap: one segment is chosen for [DT_STRTAB, DT_STRTAB+DT_STRSZ),
//! and that choice is shared by parse validation, `str_at` and diagnostics
//! (see the overlap tests below for the exact rule).

use android_elf_load::elf::ElfView;
use android_elf_load::fixture::build_minimal_aarch64_so;
use android_elf_load::{GuestModule, LoadError};
use std::collections::HashMap;

/// Layout constants of the synthetic fixture (all vaddr == file offset).
const DYNAMIC: usize = 176; // 10 entries * 16
const DYNSYM: usize = 384; // 3 entries * 24
const DATA: usize = 512;

/// The stock fixture's string table is 31 bytes: [0], "rudroid_answer\0" at 1,
/// "rudroid_import\0" at 16. DT_STRSZ lives in dynamic entry 2 (tag 10).
const DYNSTR_SIZE: u64 = 31;

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn set_strsz(bytes: &mut [u8], value: u64) {
    // Dynamic entry 2 = (DT_STRSZ, 31).
    put64(bytes, DYNAMIC + 2 * 16 + 8, value);
}

fn set_dynsym_name(bytes: &mut [u8], index: usize, name_offset: u32) {
    let at = DYNSYM + index * 24;
    bytes[at..at + 4].copy_from_slice(&name_offset.to_le_bytes());
}

/// The review probe: only the initial NUL is inside the declared table, yet
/// both dynsym names (and therefore the load) used to succeed.
#[test]
fn string_outside_dt_strsz_must_fail() {
    let mut bytes = build_minimal_aarch64_so();
    set_strsz(&mut bytes, 1);
    let result = GuestModule::from_bytes(bytes, &HashMap::new());
    match result {
        Err(err) => assert!(
            err.to_string().contains("dynamic string table bounds"),
            "the failure must name the DT_STRSZ bounds: {err}"
        ),
        Ok(_) => panic!("accepted dynsym names outside DT_STRSZ"),
    }
}

/// A dynsym name at exactly strsz (or beyond) is outside the table.
#[test]
fn dynsym_name_at_or_beyond_strsz_must_fail() {
    for name_offset in [DYNSTR_SIZE, DYNSTR_SIZE + 15] {
        let mut bytes = build_minimal_aarch64_so();
        set_dynsym_name(&mut bytes, 2, name_offset as u32);
        let result = GuestModule::from_bytes(bytes, &HashMap::new());
        assert!(
            result.is_err(),
            "accepted dynsym name offset {name_offset} with DT_STRSZ={DYNSTR_SIZE}"
        );
    }
}

/// A name whose NUL only appears outside the declared table is rejected.
#[test]
fn nul_only_outside_the_table_must_fail() {
    let mut bytes = build_minimal_aarch64_so();
    // Table = [0, 15): NUL + "rudroid_answer" without its terminating NUL.
    set_strsz(&mut bytes, 15);
    let result = GuestModule::from_bytes(bytes, &HashMap::new());
    assert!(
        result.is_err(),
        "accepted a name whose NUL lies outside DT_STRSZ"
    );
}

/// A missing DT_STRSZ is a parse-time diagnostic, never a whole-file scan.
#[test]
fn missing_dt_strsz_is_a_parse_diagnostic() {
    let mut bytes = build_minimal_aarch64_so();
    // Neutralize dynamic entry 2: unknown tags are collected but ignored.
    put64(&mut bytes, DYNAMIC + 2 * 16, 30); // DT_NEEDED+ would matter; 30 is inert
    put64(&mut bytes, DYNAMIC + 2 * 16 + 8, 0);
    let result = GuestModule::from_bytes(bytes, &HashMap::new());
    assert!(
        matches!(result, Err(LoadError::Elf(_))),
        "a missing DT_STRSZ must be a diagnostic"
    );
}

/// DT_STRTAB+DT_STRSZ that is not backed by a PT_LOAD file range fails at
/// parse time instead of scanning arbitrary bytes.
#[test]
fn oversized_dt_strsz_is_a_parse_diagnostic() {
    let mut bytes = build_minimal_aarch64_so();
    set_strsz(&mut bytes, 0x0000_FFFF_FFFF_FFFF);
    let result = GuestModule::from_bytes(bytes, &HashMap::new());
    assert!(
        matches!(result, Err(LoadError::Elf(_))),
        "an overflowing DT_STRSZ must be a parse diagnostic"
    );
}

/// Legal tables keep working: the empty string, NUL-terminated names at the
/// exact table end, and symbol lookups resolve.
#[test]
fn legal_table_layouts_still_resolve() {
    let bytes = build_minimal_aarch64_so();
    let module = GuestModule::from_bytes(bytes.clone(), &HashMap::new())
        .expect("the stock fixture has a valid string table");
    assert_eq!(
        module.symbol("rudroid_answer"),
        Some(module.base() + DATA),
        "defined symbol resolves with DT_STRSZ={DYNSTR_SIZE}"
    );

    // The empty string (offset 0) and the final NUL (offset 30) are inside.
    let view = ElfView::parse(&bytes).expect("parse");
    assert_eq!(view.dynsym(0).expect("null symbol").0, "");
    assert_eq!(view.dynsym(1).expect("answer symbol").0, "rudroid_answer");
    assert_eq!(view.dynsym(2).expect("import symbol").0, "rudroid_import");
}

/// Non-identity vaddr layouts resolve names through the same bounded path.
#[test]
fn non_identity_vaddr_dynstr_layout_resolves() {
    let delta = 0x800u64;
    let mut bytes = build_minimal_aarch64_so();
    // Shift every vaddr up (segment, dynamic, table pointers, targets).
    let patch = [
        (64 + 16, delta),                    // ph0 p_vaddr
        (64 + 56 + 16, delta + 176 as u64),  // PT_DYNAMIC p_vaddr
    ];
    for (at, value) in patch {
        put64(&mut bytes, at, value);
    }
    let dyn_entries = [(0usize, 384u64), (1, 336), (3, 456), (4, 528)];
    for (index, vaddr) in dyn_entries {
        put64(&mut bytes, DYNAMIC + index * 16 + 8, delta + vaddr);
    }
    put64(&mut bytes, 528, delta + DATA as u64 + 8);
    put64(&mut bytes, 528 + 24, delta + DATA as u64 + 16);

    let module =
        GuestModule::from_bytes(bytes, &HashMap::new()).expect("shifted image loads");
    assert_eq!(
        module.symbol("rudroid_answer"),
        Some(module.base() + DATA),
        "names resolve through the translated, DT_STRSZ-bounded table"
    );
}

/// DT_NEEDED names obey the same bounds.
#[test]
fn dt_needed_names_follow_the_same_bounds() {
    // Legal: DT_NEEDED at offset 1 ("rudroid_answer"). Dynamic entry 7 is the
    // fixture's DT_NULL — parsing stops there, so the new entry takes its
    // place and entry 8 stays the terminator.
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, DYNAMIC + 7 * 16, 1); // DT_NEEDED
    put64(&mut bytes, DYNAMIC + 7 * 16 + 8, 1);
    let view = ElfView::parse(&bytes).expect("parse");
    assert_eq!(view.needed_names().expect("names"), vec!["rudroid_answer"]);

    // Boundary: the offset equals strsz — outside the table.
    let mut bytes = build_minimal_aarch64_so();
    put64(&mut bytes, DYNAMIC + 7 * 16, 1);
    put64(&mut bytes, DYNAMIC + 7 * 16 + 8, DYNSTR_SIZE);
    let view = ElfView::parse(&bytes).expect("parse");
    assert!(
        view.needed_names().is_err(),
        "DT_NEEDED at offset == DT_STRSZ must be rejected"
    );
}

// ---------------------------------------------------------------------------
// Ticket 67: validation and reads must use the same PT_LOAD translation.
// ---------------------------------------------------------------------------

/// The R2 review probe (review-2026-09-15-r2/overlap.rs), verbatim: an
/// earlier short PT_LOAD maps only the initial NUL of the string table to
/// file offset 900 (which holds `\0fakezzz_answer\0fakezzz_import\0`), while the
/// later original PT_LOAD still file-backs the full genuine table. Parse
/// validation selects the later segment (the only one covering the whole
/// range), so string reads must too — dynsym must resolve `rudroid_answer`,
/// never the unvalidated `fakezzz_answer`. The fixture is only parsed, never
/// executed.
#[test]
fn dynstr_reads_the_same_file_range_that_parse_validated() {
    let mut b = build_minimal_aarch64_so();
    let ph = b[64..176].to_vec();
    b.resize(1024, 0);
    put64(&mut b, 32, 640);
    b[56..58].copy_from_slice(&3u16.to_le_bytes());
    // Earlier short LOAD maps only the initial NUL of dynstr to file offset 900.
    b[640..644].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut b, 648, 900);
    put64(&mut b, 656, 336);
    put64(&mut b, 672, 1);
    put64(&mut b, 680, 1);
    // Later original LOAD provides the full genuine string table.
    b[696..808].copy_from_slice(&ph);
    b[900..931].copy_from_slice(b"\0fakezzz_answer\0fakezzz_import\0");
    let view = ElfView::parse(&b).expect("full table is file-backed by second LOAD");
    let validated = view.vaddr_slice(view.strtab, view.strsz).unwrap();
    assert_eq!(&validated[1..15], b"rudroid_answer");
    assert_eq!(
        view.dynsym(1).unwrap().0,
        "rudroid_answer",
        "dynsym must read the validated table, not bytes outside first LOAD"
    );
}

/// Mirror of the probe with the program-header order reversed: the full
/// original LOAD (covering the whole declared table) comes FIRST, and the
/// short NUL-only LOAD (mapping dynstr's initial byte to the fake table at
/// file offset 900) is appended LAST.
///
/// Deterministic rule (ticket 67): the FIRST PT_LOAD in program-header order
/// whose file-backed part covers the WHOLE [DT_STRTAB, DT_STRTAB+DT_STRSZ)
/// range is selected — once — for validation, string reads and diagnostics.
/// Here that is the genuine segment, so a degenerate overlap appended later
/// must not flip a validated table onto unvalidated bytes.
#[test]
fn full_segment_first_short_segment_after_reads_the_validated_table() {
    let mut b = build_minimal_aarch64_so();
    let ph = b[64..176].to_vec(); // original [PT_LOAD, PT_DYNAMIC]
    b.resize(1024, 0);
    put64(&mut b, 32, 640);
    b[56..58].copy_from_slice(&3u16.to_le_bytes());
    // First: the original full LOAD — the only whole-table coverage.
    b[640..696].copy_from_slice(&ph[..56]);
    // Second: PT_DYNAMIC, unchanged.
    b[696..752].copy_from_slice(&ph[56..112]);
    // Last: short LOAD mapping only dynstr's initial NUL to file offset 900.
    b[752..756].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut b, 760, 900);
    put64(&mut b, 768, 336);
    put64(&mut b, 784, 1);
    put64(&mut b, 792, 1);
    b[900..931].copy_from_slice(b"\0fakezzz_answer\0fakezzz_import\0");

    let view = ElfView::parse(&b).expect("genuine table stays file-backed");
    let validated = view.vaddr_slice(view.strtab, view.strsz).unwrap();
    assert_eq!(&validated[1..15], b"rudroid_answer");
    assert_eq!(
        view.dynsym(1).unwrap().0,
        "rudroid_answer",
        "reads follow the same first-full-coverage choice as validation"
    );
}

/// Both PT_LOADs FULLY cover the declared table but map it to different file
/// bytes: the first segment points at the fake table (file offset 900), the
/// second at the genuine bytes (file offset 336).
///
/// Deterministic rule (ticket 67), written out: when several segments fully
/// cover the range, the FIRST one in program-header order wins for
/// validation, reads and diagnostics alike. The loader does not judge which
/// bytes are "genuine" — it guarantees there is exactly ONE consistent
/// choice, so the split-brain failure (validate one segment, read another)
/// is impossible regardless of layout.
#[test]
fn two_fully_covering_segments_select_the_first_in_header_order() {
    let mut b = build_minimal_aarch64_so();
    let ph = b[64..176].to_vec();
    b.resize(1024, 0);
    put64(&mut b, 32, 640);
    b[56..58].copy_from_slice(&3u16.to_le_bytes());
    // First LOAD: vaddr 336, file offset 900, filesz 31 — fully covers the
    // declared table, but with the fake bytes.
    b[640..644].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut b, 648, 900);
    put64(&mut b, 656, 336);
    put64(&mut b, 672, DYNSTR_SIZE);
    put64(&mut b, 680, DYNSTR_SIZE);
    // Second LOAD: the original full LOAD — also fully covers [336, 367),
    // with the genuine bytes.
    b[696..752].copy_from_slice(&ph[..56]);
    // Third: PT_DYNAMIC, unchanged.
    b[752..808].copy_from_slice(&ph[56..112]);
    b[900..931].copy_from_slice(b"\0fakezzz_answer\0fakezzz_import\0");

    let view = ElfView::parse(&b).expect("both segments fully cover the table");
    let validated = view.vaddr_slice(view.strtab, view.strsz).unwrap();
    assert_eq!(
        &validated[1..15],
        b"fakezzz_answer",
        "validation selects the FIRST fully-covering segment"
    );
    assert_eq!(
        view.dynsym(1).unwrap().0,
        "fakezzz_answer",
        "reads select the SAME first segment as validation"
    );
    assert_eq!(
        view.dynsym(2).unwrap().0,
        "fakezzz_import",
        "the whole table comes from the one selected segment"
    );
}

/// dynsym and DT_NEEDED must BOTH resolve through the validated range in the
/// probe layout (short LOAD first, full table later). Under the old split
/// translation, DT_NEEDED read `fakezzz_answer` from the unvalidated segment.
#[test]
fn dynsym_and_dt_needed_read_validated_bytes_under_overlap() {
    let mut b = build_minimal_aarch64_so();
    let ph = b[64..176].to_vec();
    b.resize(1024, 0);
    put64(&mut b, 32, 640);
    b[56..58].copy_from_slice(&3u16.to_le_bytes());
    b[640..644].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut b, 648, 900);
    put64(&mut b, 656, 336);
    put64(&mut b, 672, 1);
    put64(&mut b, 680, 1);
    b[696..808].copy_from_slice(&ph);
    // DT_NEEDED (offset 1 -> "rudroid_answer") replaces the fixture's DT_NULL;
    // entry 8 stays the zero terminator (same technique as the ticket-64
    // DT_NEEDED regression above).
    put64(&mut b, DYNAMIC + 7 * 16, 1);
    put64(&mut b, DYNAMIC + 7 * 16 + 8, 1);
    b[900..931].copy_from_slice(b"\0fakezzz_answer\0fakezzz_import\0");

    let view = ElfView::parse(&b).expect("full table is file-backed by the later LOAD");
    assert_eq!(
        view.needed_names().expect("names"),
        vec!["rudroid_answer"],
        "DT_NEEDED must read the validated bytes, not the overlapping short segment"
    );
    assert_eq!(
        view.dynsym(1).unwrap().0,
        "rudroid_answer",
        "dynsym must read the validated bytes"
    );
}

/// The buffer and executable backends share `ElfView`, so an overlap layout
/// resolves identically on both (`exec` is Linux-only; the buffer backend
/// runs everywhere). A layout with NO whole-table coverage keeps failing at
/// parse with the specific `DynstrBounds` reason, preserved verbatim through
/// `LoadError` and (ticket 65) the C `rudroid_elf_last_error` channel.
#[test]
fn backends_agree_and_failures_keep_their_specific_reason() {
    let overlap_layout = || {
        let mut b = build_minimal_aarch64_so();
        let ph = b[64..176].to_vec();
        b.resize(1024, 0);
        put64(&mut b, 32, 640);
        b[56..58].copy_from_slice(&3u16.to_le_bytes());
        // Short NUL-only LOAD first, genuine full LOAD later (probe layout).
        b[640..644].copy_from_slice(&1u32.to_le_bytes());
        put64(&mut b, 648, 900);
        put64(&mut b, 656, 336);
        put64(&mut b, 672, 1);
        put64(&mut b, 680, 1);
        b[696..808].copy_from_slice(&ph);
        b[900..931].copy_from_slice(b"\0fakezzz_answer\0fakezzz_import\0");
        b
    };

    // Buffer backend: the name resolves from the validated table bytes.
    let module = GuestModule::from_bytes(overlap_layout(), &HashMap::new())
        .expect("buffer backend loads the overlap layout");
    assert_eq!(
        module.symbol("rudroid_answer"),
        Some(module.base() + DATA),
        "buffer backend resolves the name from the validated table"
    );

    // Executable backend shares the same ElfView path — identical answer.
    #[cfg(target_os = "linux")]
    {
        let path = std::env::temp_dir().join("rudroid67-overlap-layout.so");
        std::fs::write(&path, overlap_layout()).expect("write overlap fixture");
        // SAFETY: inert, self-built image with no constructors/imports;
        // its backing file remains unchanged until the mapping is dropped.
        let exec = unsafe { android_elf_load::exec::ExecutableModule::map(&path, &HashMap::new()) }
            .expect("executable backend maps the overlap layout");
        assert_eq!(
            exec.symbol("rudroid_answer"),
            Some(exec.base() + DATA),
            "executable backend agrees with the buffer backend"
        );
        std::fs::remove_file(&path).ok();
    }

    // Negative control: trim the only LOAD so it still file-backs the
    // dynamic table and even touches dynstr's first byte, but can NEVER
    // cover the WHOLE table — no whole-table coverage means the table is
    // rejected at parse with the SPECIFIC reason, which stays intact through
    // LoadError::Elf(...) into the ticket-65 error channel.
    let mut b = build_minimal_aarch64_so();
    put64(&mut b, 64 + 32, 337); // ph0 p_filesz: covers [0, 337), table ends at 367
    put64(&mut b, 64 + 40, 337); // ph0 p_memsz
    match GuestModule::from_bytes(b, &HashMap::new()) {
        Err(LoadError::Elf(android_elf_load::elf::ElfError::DynstrBounds(reason))) => {
            assert_eq!(reason, "table range is not file-backed");
        }
        Ok(_) => panic!("accepted a dynstr table no PT_LOAD fully covers"),
        Err(other) => panic!("expected a DynstrBounds parse diagnostic, got {other}"),
    }
}
