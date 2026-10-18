//! Ticket 86: TLS symbols are distinguished from function/data symbols and
//! rejected precisely. An STT_TLS `st_value` is an offset into the module's
//! TLS block, not an image address, so neither an export lookup nor an
//! ABS64/GLOB_DAT/JUMP_SLOT relocation may turn it into `base + st_value`.

use std::collections::HashMap;

use android_elf_load::fixture::build_minimal_aarch64_so;
use android_elf_load::GuestModule;

/// Layout constant of the synthetic fixture (all vaddr == file offset).
const DYNSYM: usize = 384; // 3 entries * 24

/// The defined `rudroid_answer` export (sym 1) retyped as STT_TLS: the lookup
/// misses instead of answering `base + st_value`.
#[test]
fn defined_tls_export_is_not_an_address() {
    let mut bytes = build_minimal_aarch64_so();
    bytes[DYNSYM + 24 + 4] = 0x16; // STB_GLOBAL, STT_TLS
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("module loads");
    assert_eq!(module.symbol("rudroid_answer"), None, "STT_TLS must not resolve to an address");
}

/// The weak `rudroid_import` (sym 2, ABS64 target) retyped as STT_TLS: binding
/// it would silently write 0 (or a TLS offset) into an address slot, so the
/// load fails naming the symbol and the unsupported class.
#[test]
fn tls_symbol_relocation_fails_the_load() {
    let mut bytes = build_minimal_aarch64_so();
    bytes[DYNSYM + 48 + 4] = 0x26; // STB_WEAK, STT_TLS
    let error = match GuestModule::from_bytes(bytes, &HashMap::new()) {
        Ok(_) => panic!("a TLS symbol relocation must not load"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("rudroid_import") && error.contains("TLS"),
        "diagnostic names the symbol and the TLS class: {error}"
    );
}
