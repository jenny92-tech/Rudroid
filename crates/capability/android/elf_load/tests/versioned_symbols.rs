//! INPUT:  synthetic ELF version requirements and both symbol-source C ABIs
//! OUTPUT:  exact version binding, rejection, weak-zero and revocation regressions
//! POS:    a matching name alone never satisfies a strong or weak ABI requirement
//!
//! Linux-only: the executable backend is Linux (ticket 49/85/86).

#![cfg(target_os = "linux")]

use std::ffi::{c_char, c_void, CStr, CString};
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// Source registration is process-global state (ticket 86 ABI): these
/// suites must not race each other on it.
static SERIAL: Mutex<()> = Mutex::new(());

use android_elf_load::fixture::{
    build_minimal_aarch64_so, build_versioned_aarch64_so, VersionMode,
};

static COUNTER: AtomicU32 = AtomicU32::new(0);
static FINALIZED: AtomicU32 = AtomicU32::new(0);
static FINALIZED_VALUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "C" fn finalize_live_range(start: usize, length: usize) {
    assert!(length >= 536);
    FINALIZED_VALUE.store(
        ((start + 528) as *const usize).read_unaligned(),
        Ordering::Relaxed,
    );
    FINALIZED.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn explicitly_registered_finalizer_survives_source_revoke_and_runs_before_unmap() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    FINALIZED.store(0, Ordering::Relaxed);
    unsafe {
        assert_eq!(
            android_elf_load::rudroid_elf_symbol_source_register_finalized(
                c"null-finalizer".as_ptr(),
                Some(supply_versioned),
                std::ptr::null_mut(),
                None
            ),
            -1
        );
        assert_eq!(
            android_elf_load::rudroid_elf_symbol_source_register_finalized(
                c"null-source".as_ptr(),
                None,
                std::ptr::null_mut(),
                Some(finalize_live_range)
            ),
            -1
        );
    }
    let source = SourceGuard(unsafe {
        android_elf_load::rudroid_elf_symbol_source_register_finalized(
            c"scoped-finalizer".as_ptr(),
            Some(supply_versioned),
            std::ptr::null_mut(),
            Some(finalize_live_range),
        )
    });
    assert!(source.0 > 0);
    let path = write_fixture(
        "finalized",
        &build_versioned_aarch64_so(VersionMode::Strong),
    );
    let handle = load(&path);
    assert!(!handle.is_null(), "{}", last_error());
    drop(source); // revokes the lookup source; the mapped image keeps the shared cleanup snapshot
    assert!(load(&path).is_null());
    assert_eq!(FINALIZED.load(Ordering::Relaxed), 0);
    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(handle);
    }
    assert_eq!(FINALIZED.load(Ordering::Relaxed), 1);
    assert_eq!(FINALIZED_VALUE.load(Ordering::Relaxed), 0x0000_7000_B060);
    assert!(file_unmapped(&path));
    std::fs::remove_file(path).unwrap();
}

fn write_fixture(name: &str, image: &[u8]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::AcqRel);
    let path = std::env::temp_dir().join(format!("rudroid-t86-{}-{name}-{id}.so", std::process::id()));
    std::fs::write(&path, image).expect("write fixture");
    path
}

fn load(path: &PathBuf) -> *mut android_elf_load::ExecutableModule {
    let c = CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: NUL-terminated path owned for the call.
    unsafe { android_elf_load::rudroid_elf_exec_load(c.as_ptr()) }
}

fn last_error() -> String {
    let mut buffer = [0u8; 512];
    // SAFETY: valid buffer for the call.
    let len =
        unsafe { android_elf_load::rudroid_elf_last_error(buffer.as_mut_ptr(), buffer.len()) };
    let len = len.min(buffer.len());
    String::from_utf8_lossy(&buffer[..len]).into_owned()
}

fn file_unmapped(path: &PathBuf) -> bool {
    let maps = std::fs::read_to_string("/proc/self/maps").expect("read maps");
    let needle = path.to_str().expect("utf8 fixture path");
    !maps.lines().any(|line| line.contains(needle))
}

/// The source-layer provider: supplies `rudroid_import` at a recognizable
/// non-null placeholder address (the relocation only writes the value into
/// guest memory — nothing calls through it here).
unsafe extern "C" fn supply_import(_name: *const c_char, _user: *mut c_void) -> usize {
    0x0000_7000_B060
}

/// Register/unregister one `rudroid_import` source and reset the exec chain
/// afterwards so tests stay independent.
struct SourceGuard(c_int);

impl SourceGuard {
    fn versioned() -> SourceGuard {
        let id = unsafe {
            android_elf_load::rudroid_elf_symbol_source_register_versioned(
                c"version-test".as_ptr(),
                Some(supply_versioned),
                std::ptr::null_mut(),
            )
        };
        assert!(id > 0);
        SourceGuard(id)
    }
    fn arm() -> SourceGuard {
        // SAFETY: source ABI — plain callback, null user, id kept for the
        // symmetric unregister.
        let id = unsafe {
            android_elf_load::rudroid_elf_symbol_source_register(
                c"rudroid_import".as_ptr(),
                supply_import,
                std::ptr::null_mut(),
            )
        };
        assert!(id > 0, "source registration must succeed");
        SourceGuard(id)
    }
}

unsafe extern "C" fn supply_versioned(
    name: *const c_char,
    file: *const c_char,
    version: *const c_char,
    _: *mut c_void,
) -> usize {
    if name.is_null() || file.is_null() || version.is_null() {
        return 0;
    }
    if CStr::from_ptr(name) == c"rudroid_import"
        && CStr::from_ptr(file) == c"libc.so.6"
        && CStr::from_ptr(version) == c"TESTVER_1.0"
    {
        0x0000_7000_B060
    } else {
        0
    }
}

#[test]
fn matching_version_and_library_bind_strong_and_weak_imports() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _source = SourceGuard::versioned();
    for mode in [VersionMode::Strong, VersionMode::Weak] {
        let path = write_fixture("ver-accepted", &build_versioned_aarch64_so(mode));
        let handle = load(&path);
        assert!(!handle.is_null(), "{}", last_error());
        unsafe {
            let base = android_elf_load::rudroid_elf_exec_load_base(handle);
            assert_eq!(
                ((base + 528) as *const usize).read_unaligned(),
                0x0000_7000_B060
            );
            android_elf_load::rudroid_elf_exec_destroy(handle);
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn wrong_version_or_library_cannot_fall_back_to_a_name_source() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _bare = SourceGuard::arm();
    let _source = SourceGuard::versioned();
    for (offset, replacement) in [
        (576, b"TESTVER_2.0\0".as_slice()),
        (588, b"libm.so.6\0".as_slice()),
    ] {
        let mut image = build_versioned_aarch64_so(VersionMode::Strong);
        image[offset..offset + replacement.len()].copy_from_slice(replacement);
        let path = write_fixture("ver-mismatch", &image);
        assert!(
            load(&path).is_null(),
            "a name hit must not override a mismatched ABI"
        );
        assert!(last_error().contains("refusing to bind by bare name"));
        assert!(file_unmapped(&path));
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn weak_versioned_import_ignores_a_name_only_hit() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _bare = SourceGuard::arm();
    let path = write_fixture(
        "ver-weak-name-hit",
        &build_versioned_aarch64_so(VersionMode::Weak),
    );
    let handle = load(&path);
    assert!(!handle.is_null(), "{}", last_error());
    unsafe {
        let base = android_elf_load::rudroid_elf_exec_load_base(handle);
        assert_eq!(((base + 528) as *const usize).read_unaligned(), 0);
        android_elf_load::rudroid_elf_exec_destroy(handle);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn versioned_source_revocation_removes_supply_and_null_callback_is_rejected() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        assert_eq!(
            android_elf_load::rudroid_elf_symbol_source_register_versioned(
                c"invalid".as_ptr(),
                None,
                std::ptr::null_mut(),
            ),
            -1
        );
    }
    let source = SourceGuard::versioned();
    drop(source);
    let path = write_fixture(
        "ver-revoked",
        &build_versioned_aarch64_so(VersionMode::Strong),
    );
    assert!(load(&path).is_null());
    assert!(last_error().contains("no version-aware provider"));
    assert!(file_unmapped(&path));
    std::fs::remove_file(path).unwrap();
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        // SAFETY: id from the registration.
        let rc = unsafe { android_elf_load::rudroid_elf_symbol_source_unregister(self.0) };
        assert_eq!(rc, 0, "unregister must succeed");
        // SAFETY: test-owned chain state.
        unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    }
}

/// A versioned STRONG undefined import must be refused even when a
/// registered source supplies the same NAME: the diagnostic names the
/// version and the requiring file, nothing is bound, nothing leaks.
#[test]
fn versioned_strong_import_is_refused_even_with_a_name_provider() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = write_fixture(
        "ver-strong",
        &build_versioned_aarch64_so(VersionMode::Strong),
    );
    let _source = SourceGuard::arm();

    let handle = load(&path);
    assert!(
        handle.is_null(),
        "a versioned strong import must be refused"
    );
    let error = last_error();
    assert!(
        error.contains("requires version `TESTVER_1.0`") && error.contains("libc.so.6"),
        "the refusal must name version and file: {error}"
    );
    assert!(
        error.contains("refusing to bind by bare name"),
        "the refusal must be the precise-reject diagnostic: {error}"
    );
    assert!(
        file_unmapped(&path),
        "the refused load must leave no mapping behind"
    );
    let _ = std::fs::remove_file(&path);
}

/// Same NAME, no version requirement (VERSYM baseline index 1): the source
/// supply BINDS and the load succeeds — proving the gate refuses only the
/// versioned case and the underlying mechanism still works.
#[test]
fn baseline_import_binds_the_name_provider() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = write_fixture(
        "ver-baseline",
        &build_versioned_aarch64_so(VersionMode::Baseline),
    );
    let _source = SourceGuard::arm();

    let handle = load(&path);
    assert!(
        !handle.is_null(),
        "an unversioned (baseline) import binds the source: {}",
        last_error()
    );
    // SAFETY: valid handle.
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
    let _ = std::fs::remove_file(&path);
}

/// A versioned WEAK import with no matching provider binds 0.
#[test]
fn versioned_weak_import_binds_zero_without_a_provider() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = write_fixture("ver-weak", &build_versioned_aarch64_so(VersionMode::Weak));
    let handle = load(&path);
    assert!(
        !handle.is_null(),
        "a versioned weak import binds 0 without a provider: {}",
        last_error()
    );
    // SAFETY: valid handle.
    unsafe { android_elf_load::rudroid_elf_exec_destroy(handle) };
    let _ = std::fs::remove_file(&path);
}

/// No version tables at all (stock fixture with a strong undefined import):
/// the plain missing-provider diagnostic — the gate adds nothing to the
/// unversioned case, regardless of whether the guest is Android or Linux.
#[test]
fn unversioned_missing_import_keeps_the_plain_diagnostic() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let mut image = build_minimal_aarch64_so();
    image[384 + 48 + 4] = 0x12; // dynsym slot 2 binding: WEAK -> GLOBAL
    let path = write_fixture("no-versym-strong", &image);
    let handle = load(&path);
    assert!(
        handle.is_null(),
        "strong undefined must fail without a provider"
    );
    let error = last_error();
    assert!(
        error.contains("no provider module supplies it"),
        "plain undefined diagnostic, not a version refusal: {error}"
    );
    assert!(
        file_unmapped(&path),
        "the failed load leaves no mapping behind"
    );
    let _ = std::fs::remove_file(&path);
}
