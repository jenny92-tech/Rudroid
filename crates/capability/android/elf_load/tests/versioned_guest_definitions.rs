//! Synthetic provider/consumer coverage for Bionic per-DSO guest versions.

#![cfg(target_os = "linux")]

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use android_elf_load::elf::{ElfError, ElfView, SymbolVersion};
use android_elf_load::fixture::{
    build_multiversion_gnu_provider_aarch64_so, build_multiversion_provider_aarch64_so,
    build_sysv_versioned_consumer_aarch64_so, build_versioned_aarch64_so,
    build_versioned_provider_aarch64_so, VersionMode, MULTIVERSION_GNU_V1_VALUE,
    MULTIVERSION_GNU_V2_VALUE, MULTIVERSION_V1_VALUE, MULTIVERSION_V2_VALUE,
};
use android_elf_load::hash::sysv_hash;

static SERIAL: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU32 = AtomicU32::new(0);

const CONSUMER_IMPORT_SLOT: usize = 528;

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap()
}

fn write_fixture(name: &str, image: &[u8]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::AcqRel);
    let path = std::env::temp_dir().join(format!(
        "rudroid-verdef-{}-{name}-{id}.so",
        std::process::id()
    ));
    std::fs::write(&path, image).expect("write fixture");
    path
}

fn load(path: &PathBuf) -> *mut android_elf_load::ExecutableModule {
    let path = CString::new(path.to_str().expect("UTF-8 fixture path")).unwrap();
    // SAFETY: the C string lives for the call and the caller owns the result.
    unsafe { android_elf_load::rudroid_elf_exec_load(path.as_ptr()) }
}

fn last_error() -> String {
    let mut buffer = [0u8; 1024];
    // SAFETY: writable buffer of the supplied capacity.
    let len =
        unsafe { android_elf_load::rudroid_elf_last_error(buffer.as_mut_ptr(), buffer.len()) };
    String::from_utf8_lossy(&buffer[..len.min(buffer.len())]).into_owned()
}

fn file_unmapped(path: &PathBuf) -> bool {
    let maps = std::fs::read_to_string("/proc/self/maps").expect("read maps");
    let needle = path.to_str().expect("UTF-8 fixture path");
    !maps.lines().any(|line| line.contains(needle))
}

fn consumer() -> Vec<u8> {
    build_versioned_aarch64_so(VersionMode::Strong)
}

fn strong_unversioned_consumer() -> Vec<u8> {
    // Baseline deliberately keeps the stock WEAK import. Refusal cases need
    // GLOBAL binding with VERSYM GLOBAL (1), so a missing address fails load.
    let mut image = consumer();
    set_symbol_versym(&mut image, 2, 1);
    image
}

fn consumer_requiring(version: &str, file: &str) -> Vec<u8> {
    assert!(version.len() <= 11, "stock version-string slot is 12 bytes");
    assert!(file.len() <= 9, "stock file-string slot is 10 bytes");
    let mut image = consumer();
    image[576..588].fill(0);
    image[588..598].fill(0);
    image[576..576 + version.len()].copy_from_slice(version.as_bytes());
    image[588..588 + file.len()].copy_from_slice(file.as_bytes());
    image[620..624].copy_from_slice(&sysv_hash(version).to_le_bytes());
    image
}

fn requirement(version: &str) -> SymbolVersion<'_> {
    SymbolVersion {
        name: version,
        file: "libc.so.6",
        hash: sysv_hash(version),
    }
}

fn set_symbol_versym(image: &mut [u8], symbol: usize, version: u16) {
    let view = ElfView::parse(image).expect("fixture parses");
    let at = view
        .vaddr_to_file_offset(view.versym.expect("fixture has DT_VERSYM"))
        .expect("VERSYM is file-backed")
        + symbol * 2;
    drop(view);
    image[at..at + 2].copy_from_slice(&version.to_le_bytes());
}

fn provider_symbol(provider: *mut android_elf_load::ExecutableModule) -> usize {
    unsafe {
        android_elf_load::rudroid_elf_exec_symbol(
            provider,
            b"rudroid_import\0".as_ptr().cast(),
        )
    }
}

fn consumer_bound(consumer: *mut android_elf_load::ExecutableModule) -> usize {
    let base = unsafe { android_elf_load::rudroid_elf_exec_load_base(consumer) };
    unsafe { ((base + CONSUMER_IMPORT_SLOT) as *const usize).read_unaligned() }
}

fn load_provider_and_consumer(
    provider_image: &[u8],
    consumer_image: &[u8],
) -> (
    PathBuf,
    PathBuf,
    *mut android_elf_load::ExecutableModule,
    *mut android_elf_load::ExecutableModule,
) {
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = write_fixture("provider", provider_image);
    let consumer_path = write_fixture("consumer", consumer_image);
    let provider = load(&provider_path);
    assert!(!provider.is_null(), "provider must load: {}", last_error());
    let consumer = load(&consumer_path);
    (provider_path, consumer_path, provider, consumer)
}

fn cleanup(
    provider_path: PathBuf,
    consumer_path: PathBuf,
    provider: *mut android_elf_load::ExecutableModule,
    consumer: *mut android_elf_load::ExecutableModule,
) {
    unsafe {
        if !consumer.is_null() {
            android_elf_load::rudroid_elf_exec_destroy(consumer);
        }
        android_elf_load::rudroid_elf_exec_destroy(provider);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    let _ = std::fs::remove_file(consumer_path);
    let _ = std::fs::remove_file(provider_path);
}

fn write_named_library(name: &str, image: &[u8]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::AcqRel);
    let dir =
        std::env::temp_dir().join(format!("rudroid-verdef-named-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, image).expect("write named fixture");
    path
}

#[test]
fn exact_verdef_node_name_and_hash_bind_the_guest_definition() {
    let _serial = serial();
    // SAFETY: tests in this integration binary are serialized and destroy
    // every handle before the next case; only test-fixtures reaches this API.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = write_fixture(
        "provider",
        &build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6")),
    );
    let consumer_path = write_fixture("consumer", &consumer());

    let provider = load(&provider_path);
    assert!(!provider.is_null(), "provider must load: {}", last_error());
    let consumer = load(&consumer_path);
    assert!(!consumer.is_null(), "consumer must load: {}", last_error());

    let provider_base = unsafe { android_elf_load::rudroid_elf_exec_load_base(provider) };
    let consumer_base = unsafe { android_elf_load::rudroid_elf_exec_load_base(consumer) };
    let provider_import = unsafe {
        android_elf_load::rudroid_elf_exec_symbol(provider, b"rudroid_import\0".as_ptr().cast())
    };
    assert_ne!(provider_import, 0);
    let bound =
        unsafe { ((consumer_base + CONSUMER_IMPORT_SLOT) as *const usize).read_unaligned() };
    assert_eq!(bound, provider_import);
    assert!(provider_import >= provider_base);

    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(consumer);
        android_elf_load::rudroid_elf_exec_destroy(provider);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    assert!(file_unmapped(&consumer_path));
    assert!(file_unmapped(&provider_path));
    std::fs::remove_file(consumer_path).unwrap();
    std::fs::remove_file(provider_path).unwrap();
}

#[test]
fn sysv_duplicate_name_selects_the_later_exact_version() {
    let _serial = serial();
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    // No DT_SONAME; the consumer's vn_file must not scope the provider.
    let provider_path = write_named_library("libmv.so", &build_multiversion_provider_aarch64_so());
    let consumer_path = write_fixture(
        "multiversion-consumer",
        &consumer_requiring("V2", "libmv.so"),
    );

    let provider = load(&provider_path);
    assert!(!provider.is_null(), "provider must load: {}", last_error());
    let provider_base = unsafe { android_elf_load::rudroid_elf_exec_load_base(provider) };
    let first_export = unsafe {
        android_elf_load::rudroid_elf_exec_symbol(provider, b"rudroid_import\0".as_ptr().cast())
    };
    assert_eq!(
        first_export,
        provider_base + MULTIVERSION_V1_VALUE as usize,
        "unversioned lookup still reports the first chain definition"
    );

    let consumer = load(&consumer_path);
    assert!(
        !consumer.is_null(),
        "V2 must be found after the wrong first V1 entry: {}",
        last_error()
    );
    let consumer_base = unsafe { android_elf_load::rudroid_elf_exec_load_base(consumer) };
    let bound =
        unsafe { ((consumer_base + CONSUMER_IMPORT_SLOT) as *const usize).read_unaligned() };
    assert_eq!(bound, provider_base + MULTIVERSION_V2_VALUE as usize);

    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(consumer);
        android_elf_load::rudroid_elf_exec_destroy(provider);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    std::fs::remove_file(consumer_path).unwrap();
    std::fs::remove_dir_all(provider_path.parent().unwrap()).unwrap();
}

#[test]
fn unlisted_requested_version_uses_global_and_rejects_a_different_node() {
    let _serial = serial();
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &build_versioned_provider_aarch64_so(Some("TESTVER_2.0"), Some("libc.so.6")),
        &consumer(),
    );
    assert!(
        consumer.is_null(),
        "a provider defining only a different node must not satisfy TESTVER_1.0"
    );
    let error = last_error();
    assert!(error.contains("requires version `TESTVER_1.0`"), "{error}");
    assert!(error.contains("libc.so.6"), "{error}");
    cleanup(provider_path.clone(), consumer_path.clone(), provider, consumer);
    assert!(file_unmapped(&provider_path));
    assert!(file_unmapped(&consumer_path));
}

#[test]
fn unversioned_provider_satisfies_any_requested_version() {
    let _serial = serial();
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &build_versioned_provider_aarch64_so(None, Some("libc.so.6")),
        &consumer(),
    );
    assert!(
        !consumer.is_null(),
        "a provider without VERSYM must satisfy the import: {}",
        last_error()
    );
    assert_eq!(consumer_bound(consumer), provider_symbol(provider));
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn mismatched_soname_does_not_scope_guest_interposition() {
    let _serial = serial();
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libinterpose.so")),
        &consumer_requiring("TESTVER_1.0", "libc.so.6"),
    );
    assert!(
        !consumer.is_null(),
        "VERNEED vn_file records the dependency, not a provider allowlist: {}",
        last_error()
    );
    assert_eq!(consumer_bound(consumer), provider_symbol(provider));
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn missing_requested_node_falls_back_to_global_version_one() {
    let _serial = serial();
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_2.0"), Some("libother.so"));
    set_symbol_versym(&mut provider_image, 2, 1);
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &provider_image,
        &consumer_requiring("TESTVER_1.0", "libc.so.6"),
    );
    assert!(
        !consumer.is_null(),
        "GLOBAL index 1 must satisfy a version absent from the DSO: {}",
        last_error()
    );
    assert_eq!(consumer_bound(consumer), provider_symbol(provider));
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn existing_requested_node_rejects_a_global_definition() {
    let _serial = serial();
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    set_symbol_versym(&mut provider_image, 2, 1);
    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer());
    assert!(
        consumer.is_null(),
        "when TESTVER_1.0 exists, GLOBAL index 1 must not be selected"
    );
    assert!(
        last_error().contains("requires version `TESTVER_1.0`"),
        "{}",
        last_error()
    );
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn existing_requested_node_rejects_a_definition_at_another_node() {
    let _serial = serial();
    let mut provider_image = build_multiversion_provider_aarch64_so();
    // Both duplicate definitions now point at V2; V1 still exists as node 2.
    set_symbol_versym(&mut provider_image, 2, 3);
    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer_requiring("V1", "libmv.so"));
    assert!(
        consumer.is_null(),
        "an existing requested node must match that node, not a different symbol node"
    );
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn local_version_index_never_matches() {
    let _serial = serial();
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    set_symbol_versym(&mut provider_image, 2, 0);
    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer());
    assert!(consumer.is_null(), "VERSYM LOCAL (0) is never a definition");
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn hidden_bit_is_masked_after_the_requested_node_is_selected() {
    let _serial = serial();
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    set_symbol_versym(&mut provider_image, 2, 0x8002);
    let value = ElfView::parse(&provider_image).unwrap().dynsym(2).unwrap().1 as usize;
    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer());
    assert!(
        !consumer.is_null(),
        "Bionic masks the VERSYM hidden bit for a requested version: {}",
        last_error()
    );
    let base = unsafe { android_elf_load::rudroid_elf_exec_load_base(provider) };
    assert_eq!(consumer_bound(consumer), base + value);
    assert_eq!(provider_symbol(provider), 0, "hidden version is only visible to the explicit version request");
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn unversioned_queries_skip_hidden_predecessors_in_sysv_and_gnu_chains() {
    let _serial = serial();
    for (mut image, expected) in [
        (build_multiversion_provider_aarch64_so(), MULTIVERSION_V2_VALUE),
        (build_multiversion_gnu_provider_aarch64_so(), MULTIVERSION_GNU_V2_VALUE),
    ] {
        set_symbol_versym(&mut image, 2, 0x8002);
        let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
            &image, &strong_unversioned_consumer(),
        );
        assert!(!consumer.is_null(), "unversioned consumer must select default V2: {}", last_error());
        let base = unsafe { android_elf_load::rudroid_elf_exec_load_base(provider) };
        assert_eq!(provider_symbol(provider), base + expected as usize);
        assert_eq!(consumer_bound(consumer), base + expected as usize);
        cleanup(provider_path, consumer_path, provider, consumer);
    }
}

#[test]
fn all_hidden_versions_cannot_supply_an_unversioned_import() {
    let _serial = serial();
    let mut image = build_multiversion_provider_aarch64_so();
    set_symbol_versym(&mut image, 2, 0x8002);
    set_symbol_versym(&mut image, 3, 0x8003);
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &image, &strong_unversioned_consumer(),
    );
    assert!(consumer.is_null(), "bare lookup must not retry a named hidden version");
    assert_eq!(provider_symbol(provider), 0);
    // The same rejected address remains an ordinary unresolved WEAK zero;
    // rejecting a strong binding must not change that legal ELF behavior.
    let weak_path = write_fixture("hidden-weak-consumer", &build_versioned_aarch64_so(VersionMode::Baseline));
    let weak = load(&weak_path);
    assert!(!weak.is_null(), "unresolved weak remains loadable: {}", last_error());
    assert_eq!(consumer_bound(weak), 0);
    unsafe { android_elf_load::rudroid_elf_exec_destroy(weak) };
    std::fs::remove_file(weak_path).unwrap();
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn version_selection_precedes_tls_rejection_for_unversioned_queries() {
    let _serial = serial();
    for visible_tls in [false, true] {
        let mut image = build_multiversion_provider_aarch64_so();
        set_symbol_versym(&mut image, 2, 0x8002);
        let view = ElfView::parse(&image).unwrap();
        let symbol = view.symtab_entry_offset(if visible_tls { 3 } else { 2 }).unwrap();
        drop(view);
        image[symbol + 4] = (image[symbol + 4] & 0xf0) | 6;
        let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
            &image, &strong_unversioned_consumer(),
        );
        if visible_tls {
            assert!(consumer.is_null(), "selected default TLS is not an address");
            assert_eq!(provider_symbol(provider), 0);
            assert!(last_error().contains("TLS"));
        } else {
            assert!(!consumer.is_null(), "hidden TLS must not hide visible V2: {}", last_error());
            let base = unsafe { android_elf_load::rudroid_elf_exec_load_base(provider) };
            assert_eq!(provider_symbol(provider), base + MULTIVERSION_V2_VALUE as usize);
            assert_eq!(consumer_bound(consumer), base + MULTIVERSION_V2_VALUE as usize);
        }
        cleanup(provider_path, consumer_path, provider, consumer);
    }
}

#[test]
fn ordinary_unversioned_provider_and_consumer_still_bind() {
    let _serial = serial();
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &build_versioned_provider_aarch64_so(None, None),
        &strong_unversioned_consumer(),
    );
    assert!(!consumer.is_null(), "no VERSYM keeps ordinary name binding: {}", last_error());
    assert_ne!(provider_symbol(provider), 0);
    assert_eq!(consumer_bound(consumer), provider_symbol(provider));
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn hidden_bit_does_not_match_a_different_requested_node() {
    let _serial = serial();
    let mut provider_image = build_multiversion_provider_aarch64_so();
    set_symbol_versym(&mut provider_image, 2, 0x8003);
    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer_requiring("V1", "libmv.so"));
    assert!(
        consumer.is_null(),
        "masking hidden must not widen an exact V1 request to V2"
    );
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn unresolved_nonlocal_version_index_never_binds() {
    let _serial = serial();
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    set_symbol_versym(&mut provider_image, 2, 0x7fff);
    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer());
    assert!(
        consumer.is_null(),
        "a nonlocal VERSYM index with no matching definition must not bind"
    );
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn forged_verdef_hash_is_rejected_during_guest_lookup() {
    let _serial = serial();
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    let view = ElfView::parse(&provider_image).expect("provider parses");
    let (verdef, count) = view.verdef.expect("provider has DT_VERDEF");
    assert_eq!(count, 1);
    let at = view
        .vaddr_to_file_offset(verdef)
        .expect("VERDEF is file-backed");
    drop(view);
    let hash = u32::from_le_bytes(provider_image[at + 8..at + 12].try_into().unwrap());
    provider_image[at + 8..at + 12].copy_from_slice(&(hash ^ 1).to_le_bytes());

    let (provider_path, consumer_path, provider, consumer) =
        load_provider_and_consumer(&provider_image, &consumer());
    assert!(
        consumer.is_null(),
        "a forged VERDEF hash must not fall back to GLOBAL or bind"
    );
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn malformed_versioned_provider_does_not_fall_through_to_an_older_one() {
    let _serial = serial();
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let valid_path = write_fixture(
        "older-valid-provider",
        &build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6")),
    );
    let mut malformed = build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    let view = ElfView::parse(&malformed).expect("provider parses");
    let (verdef, _) = view.verdef.expect("provider has DT_VERDEF");
    let at = view
        .vaddr_to_file_offset(verdef)
        .expect("VERDEF is file-backed");
    drop(view);
    let hash = u32::from_le_bytes(malformed[at + 8..at + 12].try_into().unwrap());
    malformed[at + 8..at + 12].copy_from_slice(&(hash ^ 1).to_le_bytes());
    let malformed_path = write_fixture("newer-malformed-provider", &malformed);
    let consumer_path = write_fixture("consumer", &consumer());

    let valid = load(&valid_path);
    assert!(!valid.is_null(), "older provider must load: {}", last_error());
    let malformed = load(&malformed_path);
    assert!(
        !malformed.is_null(),
        "malformed provider shape must still map: {}",
        last_error()
    );
    let rejected = load(&consumer_path);
    assert!(
        rejected.is_null(),
        "malformed newest provider must be a hard miss, not interposition fallback"
    );
    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(malformed);
        android_elf_load::rudroid_elf_exec_destroy(valid);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    std::fs::remove_file(valid_path).unwrap();
    std::fs::remove_file(malformed_path).unwrap();
    std::fs::remove_file(consumer_path).unwrap();
}

#[test]
fn sysv_undefined_import_in_newest_dso_defers_to_the_older_provider() {
    let _serial = serial();
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = write_fixture(
        "older-valid-provider",
        &build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6")),
    );
    // The intermediate is both previously loaded and import-bearing: its
    // SYSV chain exposes the undefined import under the same bare name, but
    // it has no VERDEF and is therefore not a malformed definition.
    let intermediate_path = write_fixture(
        "newer-importing-dso",
        &build_sysv_versioned_consumer_aarch64_so(),
    );
    let consumer_path = write_fixture("final-consumer", &consumer());

    let provider = load(&provider_path);
    assert!(!provider.is_null(), "older provider must load: {}", last_error());
    let intermediate = load(&intermediate_path);
    assert!(
        !intermediate.is_null(),
        "newer import-bearing DSO must bind to the older provider: {}",
        last_error()
    );
    let consumer = load(&consumer_path);
    assert!(
        !consumer.is_null(),
        "final consumer must skip the newer import and bind the older provider: {}",
        last_error()
    );
    assert_eq!(consumer_bound(consumer), provider_symbol(provider));

    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(consumer);
        android_elf_load::rudroid_elf_exec_destroy(intermediate);
        android_elf_load::rudroid_elf_exec_destroy(provider);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    std::fs::remove_file(consumer_path).unwrap();
    std::fs::remove_file(intermediate_path).unwrap();
    std::fs::remove_file(provider_path).unwrap();
}

#[test]
fn gnu_duplicate_name_selects_the_later_exact_version() {
    let _serial = serial();
    let provider_image = build_multiversion_gnu_provider_aarch64_so();
    let (provider_path, consumer_path, provider, consumer) = load_provider_and_consumer(
        &provider_image,
        &consumer_requiring("V2", "libmv.so"),
    );
    assert!(
        !consumer.is_null(),
        "GNU-hash duplicate lookup must find V2: {}",
        last_error()
    );
    let base = unsafe { android_elf_load::rudroid_elf_exec_load_base(provider) };
    assert_eq!(
        provider_symbol(provider),
        base + MULTIVERSION_GNU_V1_VALUE as usize
    );
    assert_eq!(
        consumer_bound(consumer),
        base + MULTIVERSION_GNU_V2_VALUE as usize
    );
    cleanup(provider_path, consumer_path, provider, consumer);
}

#[test]
fn forged_verneed_hash_is_rejected_before_any_name_fallback() {
    let _serial = serial();
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = write_fixture(
        "hash-provider",
        &build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6")),
    );
    let mut consumer_image = consumer();
    // Stock fixture: VERNAUX starts at 620; its first field is vna_hash.
    let expected = u32::from_le_bytes(consumer_image[620..624].try_into().unwrap());
    consumer_image[620..624].copy_from_slice(&(expected ^ 1).to_le_bytes());
    let consumer_path = write_fixture("hash-consumer", &consumer_image);

    let provider = load(&provider_path);
    assert!(!provider.is_null(), "provider must load: {}", last_error());
    let rejected = load(&consumer_path);
    assert!(rejected.is_null(), "a forged requirement hash must fail");
    let error = last_error();
    assert!(error.contains("Vernaux hash does not match"), "{error}");
    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(provider);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    std::fs::remove_file(provider_path).unwrap();
    std::fs::remove_file(consumer_path).unwrap();
}

#[test]
fn versioned_tls_definition_is_never_mixed_into_an_address_binding() {
    let _serial = serial();
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let mut provider_image =
        build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    let view = ElfView::parse(&provider_image).expect("provider parses");
    let sym = view.symtab_entry_offset(2).expect("import dynsym entry");
    drop(view);
    // Keep the low nibble (type) as TLS while preserving GLOBAL binding.
    provider_image[sym + 4] = (provider_image[sym + 4] & 0xf0) | 6;
    let provider_path = write_fixture("tls-provider", &provider_image);
    let consumer_path = write_fixture("tls-consumer", &consumer());

    let provider = load(&provider_path);
    assert!(
        !provider.is_null(),
        "provider shape must still map: {}",
        last_error()
    );
    let rejected = load(&consumer_path);
    assert!(
        rejected.is_null(),
        "a versioned TLS definition is not an address export"
    );
    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(provider);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    std::fs::remove_file(provider_path).unwrap();
    std::fs::remove_file(consumer_path).unwrap();
}

#[test]
fn newest_versioned_tls_provider_does_not_fall_through_to_an_older_one() {
    let _serial = serial();
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let valid_path = write_fixture(
        "older-valid-provider",
        &build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6")),
    );
    let mut tls = build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    let view = ElfView::parse(&tls).expect("provider parses");
    let sym = view.symtab_entry_offset(2).expect("import dynsym entry");
    drop(view);
    tls[sym + 4] = (tls[sym + 4] & 0xf0) | 6;
    let tls_path = write_fixture("newer-tls-provider", &tls);
    let consumer_path = write_fixture("consumer", &consumer());

    let valid = load(&valid_path);
    assert!(!valid.is_null(), "older provider must load: {}", last_error());
    let tls = load(&tls_path);
    assert!(!tls.is_null(), "TLS provider shape must map: {}", last_error());
    let rejected = load(&consumer_path);
    assert!(
        rejected.is_null(),
        "a matching newest TLS export must be a hard miss, not older interposition"
    );
    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(tls);
        android_elf_load::rudroid_elf_exec_destroy(valid);
        android_elf_load::rudroid_elf_image_chain_reset();
    }
    std::fs::remove_file(valid_path).unwrap();
    std::fs::remove_file(tls_path).unwrap();
    std::fs::remove_file(consumer_path).unwrap();
}

#[test]
fn unversioned_chain_rejections_block_older_addresses_but_real_misses_continue() {
    let _serial = serial();
    for shape in ["tls", "bad-versym", "undefined-miss"] {
        unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
        let older_path = write_fixture("bare-older", &build_versioned_provider_aarch64_so(None, None));
        let mut image = if shape == "undefined-miss" {
            build_sysv_versioned_consumer_aarch64_so()
        } else {
            build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), None)
        };
        let view = ElfView::parse(&image).unwrap();
        let symbol = view.symtab_entry_offset(2).unwrap();
        let dynamic = view.vaddr_to_file_offset(view.dynamic.0).unwrap();
        let dynamic_size = view.dynamic.1 as usize;
        drop(view);
        if shape == "tls" {
            image[symbol + 4] = (image[symbol + 4] & 0xf0) | 6;
        } else if shape == "bad-versym" {
            let mut changed = false;
            for entry in image[dynamic..dynamic + dynamic_size].chunks_exact_mut(16) {
                if u64::from_le_bytes(entry[..8].try_into().unwrap()) == android_elf_load::elf::DT_VERSYM {
                    entry[8..].copy_from_slice(&u64::MAX.to_le_bytes());changed = true;
                }
            }
            assert!(changed);
        }
        let newer_path = write_fixture(shape, &image);
        let consumer_path = write_fixture("bare-consumer", &strong_unversioned_consumer());
        let older = load(&older_path);assert!(!older.is_null(), "{}", last_error());
        let expected = provider_symbol(older);assert_ne!(expected, 0);
        let newer = load(&newer_path);assert!(!newer.is_null(), "provider {shape}: {}", last_error());
        let consumer = load(&consumer_path);
        if shape == "undefined-miss" {
            assert!(!consumer.is_null(), "an undefined import is a miss, not a rejected definition: {}", last_error());
            assert_eq!(consumer_bound(consumer), expected);
        } else {
            assert!(consumer.is_null(), "{shape} must not bind an older plain address");
        }
        unsafe {
            if !consumer.is_null() { android_elf_load::rudroid_elf_exec_destroy(consumer); }
            android_elf_load::rudroid_elf_exec_destroy(newer);
            android_elf_load::rudroid_elf_exec_destroy(older);
            android_elf_load::rudroid_elf_image_chain_reset();
        }
        for path in [consumer_path, newer_path, older_path] { std::fs::remove_file(path).unwrap(); }
    }
}

#[test]
fn malformed_verdef_aux_and_next_are_bounded_errors() {
    let original = build_versioned_provider_aarch64_so(Some("TESTVER_1.0"), Some("libc.so.6"));
    let view = ElfView::parse(&original).expect("provider parses");
    let (verdef_at, count) = view.verdef.expect("provider has DT_VERDEF");
    assert_eq!(count, 1);
    let verdef_at = verdef_at as usize;
    drop(view);

    let mut aux_outside = original.clone();
    aux_outside[verdef_at + 12..verdef_at + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    let view = ElfView::parse(&aux_outside).expect("dynamic table itself remains valid");
    assert!(matches!(
        view.symbol_definition(2),
        Err(ElfError::InvalidVerdef(_))
    ));
    assert!(matches!(
        view.verdef_version_index(requirement("TESTVER_9.9")),
        Err(ElfError::InvalidVerdef(_))
    ));

    let mut next_overlaps = original.clone();
    next_overlaps[verdef_at + 4..verdef_at + 6].copy_from_slice(&3u16.to_le_bytes());
    next_overlaps[verdef_at + 16..verdef_at + 20].copy_from_slice(&1u32.to_le_bytes());
    let view = ElfView::parse(&next_overlaps).expect("dynamic table itself remains valid");
    assert!(matches!(
        view.symbol_definition(2),
        Err(ElfError::InvalidVerdef(_))
    ));
    assert!(matches!(
        view.verdef_version_index(requirement("TESTVER_9.9")),
        Err(ElfError::InvalidVerdef(_))
    ));

    let mut forged_hash = original;
    let hash = u32::from_le_bytes(
        forged_hash[verdef_at + 8..verdef_at + 12]
            .try_into()
            .unwrap(),
    );
    forged_hash[verdef_at + 8..verdef_at + 12].copy_from_slice(&(hash ^ 1).to_le_bytes());
    let view = ElfView::parse(&forged_hash).expect("dynamic table itself remains valid");
    assert!(matches!(
        view.symbol_definition(2),
        Err(ElfError::InvalidVerdef(_))
    ));
    assert!(matches!(
        view.verdef_version_index(requirement("TESTVER_1.0")),
        Err(ElfError::InvalidVerdef(_))
    ));
}
