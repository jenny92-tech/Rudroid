//! INPUT:  private live-table probe plus a synthetic AArch64 image
//! OUTPUT:  file-miss/live-hit and outside-owned inconclusive regressions
//! POS:    separates raw file/live evidence from strict initialized-image binding
use crate::exec::ExecutableModule;
use crate::fixture;
use std::collections::HashMap;
use std::path::PathBuf;

const PHDR: usize = 64;
const DYNAMIC: usize = 176;
const IMAGE_SIZE: usize = 1024;
const ALT_DYNSTR: usize = 768;
const ALT_DYNSYM: usize = 800;
const ALT_GNU_HASH: usize = 896;
const ALT_SYSV_HASH: usize = 944;
const LIVE_SYMBOL: &str = "rudroid_live_only";

fn binding_fixture() -> Vec<u8> {
    let mut image = live_only_fixture();
    image[ALT_DYNSYM + 24 + 4] = 0x11; // GLOBAL OBJECT in the owned RW LOAD.
    image
}

#[test]
fn initialized_live_export_binds_without_diagnostics_and_versioned_lookup_stays_file_only() {
    let (path, module) = map_fixture("binding", &binding_fixture());
    point_live_tables_at_alt(&module);
    // No diagnostic call or trace flag is needed to make this real binding.
    assert_eq!(module.symbol(LIVE_SYMBOL), Some(module.base() + 0x100));
    assert_eq!(module.symbol_all(LIVE_SYMBOL).unwrap(), Some(module.base() + 0x100));
    let version = crate::elf::SymbolVersion { name: "TEST_1", file: "guest.so", hash: 1 };
    assert_eq!(module.symbol_versioned(LIVE_SYMBOL, version).unwrap(), None);
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("file=miss"), "{report}");
    assert!(report.contains("live=hit("), "{report}");
    release_fixture(path, module);
}

#[test]
fn published_image_without_completed_initializers_cannot_bind_live_exports() {
    let image = binding_fixture();
    let path = std::env::temp_dir().join(format!("rudroid-live-preinit-{}.so", std::process::id()));
    std::fs::write(&path, &image).unwrap();
    // SAFETY: private synthetic ELF, stable backing file, no guest code runs.
    let module = unsafe { ExecutableModule::map_with_layers(&path, &HashMap::new()) }.unwrap();
    point_live_tables_at_alt(&module);
    assert_eq!(module.symbol(LIVE_SYMBOL), None);
    assert!(format!("{}", module.symbol_diagnostic(LIVE_SYMBOL)).contains("live=hit("));
    release_fixture(path, module);
}

#[test]
fn file_hit_has_priority_over_a_different_live_definition() {
    let (path, module) = map_fixture("file-priority", &binding_fixture());
    let expected = module.symbol(fixture::ANSWER_SYMBOL).unwrap();
    let mut name = vec![0]; name.extend_from_slice(fixture::ANSWER_SYMBOL.as_bytes()); name.push(0);
    write_bytes(&module, ALT_DYNSTR, &name);
    write_bytes(&module, ALT_DYNSYM + 24 + 8, &0x110u64.to_le_bytes());
    let hash = crate::hash::gnu_hash(fixture::ANSWER_SYMBOL);
    write_bytes(&module, ALT_GNU_HASH + 16, &(1u64 << (hash % 64)).to_le_bytes());
    write_bytes(&module, ALT_GNU_HASH + 28, &((hash | 1) as u32).to_le_bytes());
    point_live_tables_at_alt(&module);
    write_patch(&module, 2, name.len() as u64);
    assert_ne!(expected, module.base() + 0x110);
    assert_eq!(module.symbol(fixture::ANSWER_SYMBOL), Some(expected));
    release_fixture(path, module);
}

#[test]
fn live_binding_rejects_invalid_export_kinds_visibility_and_target_ranges() {
    let mutations: &[(usize, &[u8])] = &[
        (4, &[0x01]), (4, &[0xa1]), (4, &[0x16]), (4, &[0x1a]), (4, &[0x12]),
        (5, &[1]), (5, &[2]), (5, &[0x80]), (6, &[0, 0]), (6, &[0xf1, 0xff]),
        (6, &[0xff, 0xff]), (8, &1024u64.to_le_bytes()),
        (8, &u64::MAX.to_le_bytes()), (16, &1024u64.to_le_bytes()),
    ];
    for (case, (offset, bytes)) in mutations.iter().enumerate() {
        let (path, module) = map_fixture(&format!("reject-{case}"), &binding_fixture());
        point_live_tables_at_alt(&module);
        write_bytes(&module, ALT_DYNSYM + 24 + offset, bytes);
        assert_eq!(module.symbol(LIVE_SYMBOL), None, "case {case}");
        if *offset == 4 && *bytes == [0x16] {
            assert!(matches!(module.symbol_all(LIVE_SYMBOL), Err(crate::ExportReject::Tls)));
        }
        release_fixture(path, module);
    }
}

#[test]
fn live_binding_rejects_outside_tables_duplicate_tags_and_version_metadata() {
    for (case, index, value) in [("sym", 0, 0x80000000), ("str", 1, 0x80000000),
        ("size", 2, 0x80000000), ("hash", 3, 0x80000000)] {
        let (path, module) = map_fixture(case, &binding_fixture());
        point_live_tables_at_alt(&module); write_patch(&module, index, value);
        assert_eq!(module.symbol(LIVE_SYMBOL), None, "{case}");
        release_fixture(path, module);
    }
    for (case, tag) in [("version", crate::elf::DT_VERSYM), ("duplicate", crate::elf::DT_SYMTAB)] {
        let (path, module) = map_fixture(case, &binding_fixture());
        point_live_tables_at_alt(&module); write_dynamic_tag(&module, 4, tag);
        assert_eq!(module.symbol(LIVE_SYMBOL), None, "{case}");
        release_fixture(path, module);
    }
}

#[test]
fn live_function_binds_only_in_an_owned_executable_load() {
    for value in [0x100u64, 0x101] {
        let mut image = live_only_fixture();
        image[ALT_DYNSYM + 24 + 8..ALT_DYNSYM + 24 + 16].copy_from_slice(&value.to_le_bytes());
        image[PHDR + 4..PHDR + 8].copy_from_slice(&(crate::elf::PF_R | crate::elf::PF_X).to_le_bytes());
        let (path, module) = map_fixture(&format!("function-rx-{value}"), &image);
        // SAFETY: this is the fixture's complete owned page-aligned mapping. It
        // is made RW (never RWX) for private table writes, then restored to RX.
        assert_eq!(unsafe { libc::mprotect(module.base() as *mut _, module.map_len(), libc::PROT_READ | libc::PROT_WRITE) }, 0);
        point_live_tables_at_alt(&module);
        // SAFETY: same owned mapping; restore its original executable protection.
        assert_eq!(unsafe { libc::mprotect(module.base() as *mut _, module.map_len(), libc::PROT_READ | libc::PROT_EXEC) }, 0);
        let expected = (value % 4 == 0).then_some(module.base() + value as usize);
        assert_eq!(module.symbol(LIVE_SYMBOL), expected);
        release_fixture(path, module);
    }
}

#[test]
fn live_binding_accepts_weak_protected_and_untyped_owned_definitions() {
    for (info, visibility) in [(0x11, 0), (0x21, 0), (0x11, 3), (0x10, 0)] {
        let mut image = binding_fixture();
        image[ALT_DYNSYM + 24 + 4] = info;
        image[ALT_DYNSYM + 24 + 5] = visibility;
        let (path, module) = map_fixture(&format!("eligible-{info}-{visibility}"), &image);
        point_live_tables_at_alt(&module);
        assert_eq!(module.symbol(LIVE_SYMBOL), Some(module.base() + 0x100));
        release_fixture(path, module);
    }
}

fn live_only_fixture() -> Vec<u8> {
    let mut image = fixture::build_minimal_aarch64_so();
    image.resize(IMAGE_SIZE, 0);
    // The fixture builder leaves p_flags clear; this probe needs a readable,
    // writable runtime LOAD so the test can alter only its private copy.
    image[PHDR + 4..PHDR + 8].copy_from_slice(&(crate::elf::PF_R | crate::elf::PF_W).to_le_bytes());
    image[PHDR + 32..PHDR + 40].copy_from_slice(&(IMAGE_SIZE as u64).to_le_bytes());
    image[PHDR + 40..PHDR + 48].copy_from_slice(&(IMAGE_SIZE as u64).to_le_bytes());

    let mut strings = vec![0u8];
    strings.extend_from_slice(LIVE_SYMBOL.as_bytes());
    strings.push(0);
    image[ALT_DYNSTR..ALT_DYNSTR + strings.len()].copy_from_slice(&strings);

    let mut symbol = [0u8; 24];
    symbol[0..4].copy_from_slice(&1u32.to_le_bytes());
    symbol[4] = 0x12; // GLOBAL OBJECT
    symbol[6..8].copy_from_slice(&1u16.to_le_bytes());
    symbol[8..16].copy_from_slice(&0x100u64.to_le_bytes());
    image[ALT_DYNSYM + 24..ALT_DYNSYM + 48].copy_from_slice(&symbol);

    let hash = crate::hash::gnu_hash(LIVE_SYMBOL);
    image[ALT_GNU_HASH..ALT_GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[ALT_GNU_HASH + 4..ALT_GNU_HASH + 8].copy_from_slice(&1u32.to_le_bytes());
    image[ALT_GNU_HASH + 8..ALT_GNU_HASH + 12].copy_from_slice(&1u32.to_le_bytes());
    image[ALT_GNU_HASH + 12..ALT_GNU_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
    image[ALT_GNU_HASH + 16..ALT_GNU_HASH + 24]
        .copy_from_slice(&(1u64 << (hash % 64)).to_le_bytes());
    image[ALT_GNU_HASH + 24..ALT_GNU_HASH + 28].copy_from_slice(&1u32.to_le_bytes());
    image[ALT_GNU_HASH + 28..ALT_GNU_HASH + 32].copy_from_slice(&((hash | 1) as u32).to_le_bytes());
    image
}

fn write_patch(module: &ExecutableModule, dynamic_index: usize, value: u64) {
    let address = module.base() + DYNAMIC + dynamic_index * 16 + 8;
    // SAFETY: the test fixture's only LOAD is RW and the dynamic table is
    // inside it; this changes the private runtime copy, never the file.
    unsafe { std::ptr::write_unaligned(address as *mut u64, value) };
}

fn write_bytes(module: &ExecutableModule, vaddr: usize, bytes: &[u8]) {
    let address = module.base() + vaddr;
    // SAFETY: callers patch only addresses in the fixture's writable private
    // LOAD. The backing file remains unchanged for the module lifetime.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len()) };
}

fn write_dynamic_tag(module: &ExecutableModule, dynamic_index: usize, tag: u64) {
    write_bytes(module, DYNAMIC + dynamic_index * 16, &tag.to_le_bytes());
}

fn map_fixture(suffix: &str, image: &[u8]) -> (PathBuf, ExecutableModule) {
    let path = std::env::temp_dir().join(format!(
        "rudroid-live-{suffix}-{}-{}.so",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::write(&path, image).expect("write live-symbol fixture");
    // SAFETY: the synthetic image is parsed and relocated but its code is not
    // executed; the file remains unchanged for the module lifetime.
    let module =
        unsafe { ExecutableModule::map(&path, &HashMap::new()) }.expect("map live-symbol fixture");
    (path, module)
}

fn release_fixture(path: PathBuf, module: ExecutableModule) {
    drop(module);
    std::fs::remove_file(path).expect("remove live-symbol fixture");
}

fn point_live_tables_at_alt(module: &ExecutableModule) {
    write_patch(module, 0, ALT_DYNSYM as u64);
    write_patch(module, 1, ALT_DYNSTR as u64);
    write_patch(module, 2, 1 + LIVE_SYMBOL.len() as u64 + 1);
    write_patch(module, 3, ALT_GNU_HASH as u64);
}

fn use_live_sysv_table(module: &ExecutableModule) {
    write_dynamic_tag(module, 3, crate::elf::DT_HASH);
    write_patch(module, 3, ALT_SYSV_HASH as u64);
}

fn sysv_live_fixture(bucket: u32, chain_one: u32) -> Vec<u8> {
    let mut image = live_only_fixture();
    image[ALT_SYSV_HASH..ALT_SYSV_HASH + 4].copy_from_slice(&1u32.to_le_bytes());
    image[ALT_SYSV_HASH + 4..ALT_SYSV_HASH + 8].copy_from_slice(&2u32.to_le_bytes());
    image[ALT_SYSV_HASH + 8..ALT_SYSV_HASH + 12].copy_from_slice(&bucket.to_le_bytes());
    image[ALT_SYSV_HASH + 12..ALT_SYSV_HASH + 16].copy_from_slice(&0u32.to_le_bytes());
    image[ALT_SYSV_HASH + 16..ALT_SYSV_HASH + 20].copy_from_slice(&chain_one.to_le_bytes());
    image
}

fn duplicate_name_fixture(first_info: u8, first_shndx: u16, first_visibility: u8) -> Vec<u8> {
    let mut image = live_only_fixture();
    let first = ALT_DYNSYM + 24;
    image[first + 4] = first_info;
    image[first + 5] = first_visibility;
    image[first + 6..first + 8].copy_from_slice(&first_shndx.to_le_bytes());

    let second = ALT_DYNSYM + 48;
    let mut symbol = [0u8; 24];
    symbol[0..4].copy_from_slice(&1u32.to_le_bytes());
    symbol[4] = 0x12;
    symbol[6..8].copy_from_slice(&1u16.to_le_bytes());
    symbol[8..16].copy_from_slice(&0x110u64.to_le_bytes());
    image[second..second + 24].copy_from_slice(&symbol);

    let hash = crate::hash::gnu_hash(LIVE_SYMBOL);
    image[ALT_GNU_HASH + 28..ALT_GNU_HASH + 32]
        .copy_from_slice(&(hash & !1).to_le_bytes());
    image[ALT_GNU_HASH + 32..ALT_GNU_HASH + 36]
        .copy_from_slice(&(hash | 1).to_le_bytes());
    image
}

#[test]
fn file_miss_with_live_hit_is_evidence_not_a_binding() {
    let path = std::env::temp_dir().join(format!(
        "rudroid-live-symbols-{}-{}.so",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::write(&path, live_only_fixture()).expect("write live-only fixture");
    // SAFETY: the synthetic image is parsed and relocated but its code is not
    // executed; the file remains unchanged for the module lifetime.
    let module =
        unsafe { ExecutableModule::map(&path, &HashMap::new()) }.expect("map live-only fixture");

    // Original file metadata still points at the original tables. The runtime
    // copy is the only place where the alternate table pointers exist.
    assert_eq!(module.symbol(LIVE_SYMBOL), None);
    write_patch(&module, 0, ALT_DYNSYM as u64);
    write_patch(&module, 1, ALT_DYNSTR as u64);
    write_patch(&module, 2, 1 + LIVE_SYMBOL.len() as u64 + 1);
    write_patch(&module, 3, ALT_GNU_HASH as u64);

    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("file=miss"), "{report}");
    assert!(report.contains("live=hit(0x"), "{report}");
    assert!(
        report.contains("tags=SYMTAB,STRTAB,STRSZ,GNU_HASH"),
        "{report}"
    );

    drop(module);
    std::fs::remove_file(path).expect("remove live-only fixture");
}

#[test]
fn outside_owned_live_table_is_inconclusive() {
    let path = std::env::temp_dir().join(format!("rudroid-live-outside-{}.so", std::process::id()));
    std::fs::write(&path, live_only_fixture()).expect("write live-only fixture");
    // SAFETY: as above; no fixture code is executed.
    let module =
        unsafe { ExecutableModule::map(&path, &HashMap::new()) }.expect("map live-only fixture");
    write_patch(&module, 3, 0x8000_0000);
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("live=inconclusive"), "{report}");
    drop(module);
    std::fs::remove_file(path).expect("remove live-only fixture");
}

#[test]
fn original_file_tables_still_supply_the_ordinary_symbol() {
    let path =
        std::env::temp_dir().join(format!("rudroid-live-original-{}.so", std::process::id()));
    std::fs::write(&path, live_only_fixture()).expect("write live-only fixture");
    // SAFETY: as above; no fixture code is executed.
    let module =
        unsafe { ExecutableModule::map(&path, &HashMap::new()) }.expect("map live-only fixture");
    let report = format!("{}", module.symbol_diagnostic(fixture::ANSWER_SYMBOL));
    assert!(report.contains("file=hit(0x"), "{report}");
    assert!(report.contains("live=hit(0x"), "{report}");
    assert!(report.contains("tags=none"), "{report}");
    drop(module);
    std::fs::remove_file(path).expect("remove live-only fixture");
}

#[test]
fn gnu_zero_bucket_is_miss_but_nonzero_below_symoffset_is_inconclusive() {
    let (path, module) = map_fixture("gnu-bucket", &live_only_fixture());
    point_live_tables_at_alt(&module);

    write_bytes(&module, ALT_GNU_HASH + 24, &0u32.to_le_bytes());
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("live=miss"), "{report}");

    write_bytes(&module, ALT_GNU_HASH + 4, &2u32.to_le_bytes());
    write_bytes(&module, ALT_GNU_HASH + 24, &1u32.to_le_bytes());
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("live=inconclusive"), "{report}");
    assert!(report.contains("below symoffset"), "{report}");
    release_fixture(path, module);
}

#[test]
fn sysv_zero_index_is_miss_but_out_of_range_index_is_inconclusive() {
    let (path, module) = map_fixture("sysv-bucket", &sysv_live_fixture(0, 2));
    use_live_sysv_table(&module);
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("live=miss"), "{report}");

    write_bytes(&module, ALT_SYSV_HASH + 8, &2u32.to_le_bytes());
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("live=inconclusive"), "{report}");
    assert!(report.contains("outside nchain"), "{report}");
    release_fixture(path, module);
}

#[test]
fn sysv_out_of_range_chain_is_inconclusive() {
    let (path, module) = map_fixture("sysv-chain", &sysv_live_fixture(1, 2));
    use_live_sysv_table(&module);
    let report = format!("{}", module.symbol_diagnostic("rudroid_absent"));
    assert!(report.contains("live=inconclusive"), "{report}");
    assert!(report.contains("outside nchain"), "{report}");
    release_fixture(path, module);
}

#[test]
fn sysv_cyclic_chain_hits_the_independent_step_bound() {
    let (path, module) = map_fixture("sysv-cycle", &sysv_live_fixture(1, 1));
    use_live_sysv_table(&module);
    let report = format!("{}", module.symbol_diagnostic("rudroid_absent"));
    assert!(report.contains("live=inconclusive"), "{report}");
    assert!(report.contains("step bound"), "{report}");
    release_fixture(path, module);
}

#[test]
fn earlier_unexportable_duplicate_cannot_prove_a_later_definition_absent() {
    let cases = [
        ("undefined", 0x12u8, 0u16, 0u8),
        ("local", 0x02, 1, 0),
        ("hidden", 0x12, 1, 2),
    ];
    for (case, info, shndx, visibility) in cases {
        let image = duplicate_name_fixture(info, shndx, visibility);
        let (path, module) = map_fixture(&format!("duplicate-{case}"), &image);
        point_live_tables_at_alt(&module);
        let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
        assert!(report.contains("live=inconclusive"), "{case}: {report}");
        assert!(
            report.contains("later duplicate definitions were not examined"),
            "{case}: {report}"
        );
        release_fixture(path, module);
    }
}

#[test]
fn absolute_symbol_is_inconclusive_and_never_rebased_to_a_hit() {
    let mut image = live_only_fixture();
    let first = ALT_DYNSYM + 24;
    image[first + 6..first + 8].copy_from_slice(&0xfff1u16.to_le_bytes());
    let (path, module) = map_fixture("absolute", &image);
    point_live_tables_at_alt(&module);
    let report = format!("{}", module.symbol_diagnostic(LIVE_SYMBOL));
    assert!(report.contains("live=inconclusive"), "{report}");
    assert!(report.contains("absolute symbol"), "{report}");
    assert!(!report.contains("live=hit"), "{report}");
    release_fixture(path, module);
}

#[test]
fn absent_syment_on_both_sides_is_not_reported_as_changed() {
    let mut image = live_only_fixture();
    let syment = DYNAMIC + 6 * 16;
    assert_eq!(
        u64::from_le_bytes(image[syment..syment + 8].try_into().expect("8 bytes")),
        crate::elf::DT_SYMENT
    );
    image[syment..syment + 8].copy_from_slice(&crate::elf::DT_NULL.to_le_bytes());

    let (path, module) = map_fixture("absent-syment", &image);
    let report = format!("{}", module.symbol_diagnostic(fixture::ANSWER_SYMBOL));
    assert!(report.contains("file=hit(0x"), "{report}");
    assert!(report.contains("live=hit(0x"), "{report}");
    assert!(report.contains("tags=none"), "{report}");
    release_fixture(path, module);
}
