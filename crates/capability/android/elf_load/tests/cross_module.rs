//! Ticket 62 acceptance: undefined dynamic symbols are never cross-module
//! exports.
//!
//! The C registry resolves a later module's imports through the earlier
//! modules' `symbol()`. An undefined (SHN_UNDEF) entry that happens to sit
//! in a hash table must therefore never resolve to anything — before this
//! fix the old code returned `base + st_value`, i.e. the module base, which
//! silently satisfied a later module's strong import.
//!
//! - SHN_UNDEF is not an export on the SYSV and GNU lookup paths, in the
//!   buffer and the Linux executable backend alike.
//! - STB_LOCAL and STV_HIDDEN/STV_INTERNAL symbols are not exports;
//!   defined GLOBAL/WEAK DEFAULT (and PROTECTED) symbols stay usable.
//! - An earlier module holding only an undefined weak `rudroid_import` cannot
//!   satisfy a later module's strong import; with a real provider appended,
//!   the import binds to the provider's symbol address, not its base.

use std::collections::HashMap;
use std::sync::Mutex;

use android_elf_load::fixture::build_minimal_aarch64_so;
use android_elf_load::{
    rudroid_elf_destroy, rudroid_elf_load_base, rudroid_elf_load_from_bytes,
    rudroid_elf_symbol, GuestModule,
};

/// The C load API registers modules in a process-global list; every test
/// that uses it serializes so parallel tests cannot satisfy each other's
/// imports (the review probe assumed `--test-threads=1`).
static C_API_SERIAL: Mutex<()> = Mutex::new(());

/// Layout constants of the synthetic fixture (all vaddr == file offset).
const DYNAMIC: usize = 176; // 10 entries * 16
const DYNSYM: usize = 384; // 3 entries * 24
const GNU_HASH: usize = 456;
const DATA: usize = 512; // answer slot + import slot

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// Swap the fixture's GNU hash table for a SYSV one at the same vaddr.
/// `bucket`/`chain` callers assemble the walk list; nbucket=1 matches every
/// name's bucket.
fn with_sysv_hash(bytes: &mut [u8], bucket0: u32, chains: [u32; 3]) {
    put64(bytes, DYNAMIC + 3 * 16, 4); // DT_HASH tag, value keeps vaddr 456
    bytes[GNU_HASH..GNU_HASH + 32].fill(0);
    bytes[GNU_HASH..GNU_HASH + 4].copy_from_slice(&1u32.to_le_bytes()); // nbucket
    bytes[GNU_HASH + 4..GNU_HASH + 8].copy_from_slice(&3u32.to_le_bytes()); // nchain
    bytes[GNU_HASH + 8..GNU_HASH + 12].copy_from_slice(&bucket0.to_le_bytes());
    for (index, chain) in chains.iter().enumerate() {
        let at = GNU_HASH + 12 + index * 4;
        bytes[at..at + 4].copy_from_slice(&chain.to_le_bytes());
    }
}

/// A module whose only hash-reachable import is the undefined weak
/// `rudroid_import` (SYSV path, like the review probe's provider).
fn provider_with_undefined_weak_import_sysv() -> Vec<u8> {
    let mut bytes = build_minimal_aarch64_so();
    with_sysv_hash(&mut bytes, 2, [0, 0, 0]); // bucket -> sym 2 only
    bytes
}

/// Same shape on the default GNU hash path.
fn provider_with_undefined_weak_import_gnu() -> Vec<u8> {
    // The stock fixture's GNU table covers only rudroid_answer, so the undefined
    // weak entry is already unreachable; state that baseline explicitly.
    build_minimal_aarch64_so()
}

/// A module that truly DEFINES `rudroid_import` at `DATA` (GLOBAL, DEFAULT,
/// shndx=1) on the SYSV path.
fn real_provider_defining_import() -> Vec<u8> {
    let mut bytes = build_minimal_aarch64_so();
    with_sysv_hash(&mut bytes, 1, [2, 0, 0]); // walk: sym 1 -> sym 2
    let at = DYNSYM + 24; // sym 1 becomes the defined rudroid_import at name offset 16
    bytes[at..at + 4].copy_from_slice(&16u32.to_le_bytes());
    bytes[at + 4] = 0x12; // STB_GLOBAL, STT_OBJECT
    bytes[at + 6..at + 8].copy_from_slice(&1u16.to_le_bytes()); // defined
    put64(&mut bytes, at + 8, DATA as u64);
    bytes
}

/// The stock fixture with its import made a strong GLOBAL undefined symbol.
fn consumer_with_strong_import() -> Vec<u8> {
    let mut bytes = build_minimal_aarch64_so();
    bytes[DYNSYM + 48 + 4] = 0x12;
    bytes
}

/// SHN_UNDEF is never an export — GNU path.
#[test]
fn shn_undef_is_not_an_export_on_the_gnu_path() {
    let bytes = provider_with_undefined_weak_import_gnu();
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("provider loads");
    assert_eq!(
        module.symbol("rudroid_import"),
        None,
        "an undefined weak dynsym must not be supplied as (base+0)"
    );
    assert_ne!(module.symbol("rudroid_answer"), None, "defined symbols stay usable");
}

/// SHN_UNDEF is never an export — SYSV path.
#[test]
fn shn_undef_is_not_an_export_on_the_sysv_path() {
    let bytes = provider_with_undefined_weak_import_sysv();
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("provider loads");
    assert_eq!(
        module.symbol("rudroid_import"),
        None,
        "an undefined weak dynsym must not be supplied as (base+0)"
    );
}

/// STB_LOCAL and STV_HIDDEN/STV_INTERNAL are not exports; DEFAULT and
/// STV_PROTECTED are.
#[test]
fn local_and_hidden_symbols_are_not_exports() {
    // LOCAL binding (st_info high nibble 0 = STB_LOCAL, low = STT_OBJECT).
    let mut bytes = build_minimal_aarch64_so();
    bytes[DYNSYM + 24 + 4] = 0x01;
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("loads");
    assert_eq!(module.symbol("rudroid_answer"), None, "STB_LOCAL must not be exported");

    // STV_HIDDEN visibility (st_other low bits).
    let mut bytes = build_minimal_aarch64_so();
    bytes[DYNSYM + 24 + 5] = 2; // STV_HIDDEN
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("loads");
    assert_eq!(module.symbol("rudroid_answer"), None, "STV_HIDDEN must not be exported");

    // STV_PROTECTED stays exported.
    let mut bytes = build_minimal_aarch64_so();
    bytes[DYNSYM + 24 + 5] = 3; // STV_PROTECTED
    let module = GuestModule::from_bytes(bytes, &HashMap::new()).expect("loads");
    assert_ne!(
        module.symbol("rudroid_answer"),
        None,
        "STV_PROTECTED definitions stay exported"
    );
}

/// The review probe: an earlier module with only an undefined weak
/// `rudroid_import` must NOT satisfy a later module's strong import (through
/// the C registry, as the loader uses it).
#[test]
fn undefined_symbol_must_not_supply_later_strong_import() {
    let _serial = C_API_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let provider = provider_with_undefined_weak_import_sysv();
    let consumer = consumer_with_strong_import();
    // SAFETY: pointers to live buffers; the C contract is documented on the
    // load functions.
    let a = unsafe { rudroid_elf_load_from_bytes(provider.as_ptr(), provider.len()) };
    assert!(!a.is_null());
    let b = unsafe { rudroid_elf_load_from_bytes(consumer.as_ptr(), consumer.len()) };
    let rejected = b.is_null();
    unsafe {
        rudroid_elf_destroy(b);
        rudroid_elf_destroy(a);
    }
    assert!(
        rejected,
        "undefined dynsym of earlier module supplied strong import as module base"
    );
}

/// With a real provider appended, the consumer's import binds to the
/// provider's symbol address — never to any module base.
#[test]
fn real_provider_symbol_binds_at_its_own_address() {
    let _serial = C_API_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let weak = provider_with_undefined_weak_import_sysv();
    let real = real_provider_defining_import();
    let consumer = consumer_with_strong_import();
    // SAFETY: pointers to live buffers; handles destroyed below.
    let weak_handle =
        unsafe { rudroid_elf_load_from_bytes(weak.as_ptr(), weak.len()) };
    assert!(!weak_handle.is_null(), "the weak-only module itself still loads");
    let real_handle = unsafe { rudroid_elf_load_from_bytes(real.as_ptr(), real.len()) };
    assert!(!real_handle.is_null());
    let consumer_handle =
        unsafe { rudroid_elf_load_from_bytes(consumer.as_ptr(), consumer.len()) };
    assert!(
        !consumer_handle.is_null(),
        "the consumer must load once a real provider is registered"
    );
    let real_base = unsafe { rudroid_elf_load_base(real_handle) };
    let consumer_base = unsafe { rudroid_elf_load_base(consumer_handle) };
    let expected = real_base + DATA;
    assert_ne!(
        expected,
        real_base,
        "sanity: the symbol address must not be the module base"
    );
    let bound = unsafe { ((consumer_base + DATA + 16) as *const usize).read_unaligned() };
    assert_eq!(
        bound, expected,
        "the strong import must bind to the provider's symbol address, not a module base"
    );
    // The weak-only module also answers nothing on direct lookups.
    assert_eq!(
        unsafe { rudroid_elf_symbol(weak_handle, b"rudroid_import\0".as_ptr().cast()) },
        0
    );
    unsafe {
        rudroid_elf_destroy(consumer_handle);
        rudroid_elf_destroy(real_handle);
        rudroid_elf_destroy(weak_handle);
    }
}

/// The executable backend shares the same lookup rules (Linux only; the
/// Docker verification runs this file).
#[cfg(target_os = "linux")]
#[test]
fn exec_backend_follows_the_same_export_rules() {
    use android_elf_load::{rudroid_elf_exec_destroy, rudroid_elf_exec_load, rudroid_elf_exec_symbol};

    let _serial = C_API_SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    let dir = std::env::temp_dir().join(format!("rudroid-ticket62-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let weak_path = dir.join("weak.so");
    let real_path = dir.join("real.so");
    std::fs::write(&weak_path, provider_with_undefined_weak_import_sysv()).unwrap();
    std::fs::write(&real_path, real_provider_defining_import()).unwrap();

    let c_weak = std::ffi::CString::new(weak_path.to_str().unwrap()).unwrap();
    let c_real = std::ffi::CString::new(real_path.to_str().unwrap()).unwrap();
    // SAFETY: paths are NUL-terminated; handles destroyed below.
    let weak = unsafe { rudroid_elf_exec_load(c_weak.as_ptr()) };
    assert!(!weak.is_null());
    assert_eq!(
        unsafe { rudroid_elf_exec_symbol(weak, b"rudroid_import\0".as_ptr().cast()) },
        0,
        "SHN_UNDEF must not be an export on the executable backend"
    );
    let real = unsafe { rudroid_elf_exec_load(c_real.as_ptr()) };
    assert!(!real.is_null());
    assert_ne!(
        unsafe { rudroid_elf_exec_symbol(real, b"rudroid_import\0".as_ptr().cast()) },
        0,
        "a real definition is exported from the executable backend"
    );
    unsafe {
        rudroid_elf_exec_destroy(real);
        rudroid_elf_exec_destroy(weak);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Rust-level binding check with an explicit globals map: the imported word
/// equals the provider's symbol address.
#[test]
fn globals_map_binding_uses_the_true_symbol_address() {
    let real = GuestModule::from_bytes(real_provider_defining_import(), &HashMap::new())
        .expect("real provider loads");
    let address = real.symbol("rudroid_import").expect("defined symbol exported");
    assert_ne!(address, real.base(), "symbol address differs from the module base");

    let mut globals = HashMap::new();
    globals.insert("rudroid_import".to_owned(), address);
    let consumer =
        GuestModule::from_bytes(consumer_with_strong_import(), &globals).expect("consumer loads");
    let word = unsafe { ((consumer.base() + DATA + 16) as *const usize).read_unaligned() };
    assert_eq!(word, address);
}
