//! Ticket 86 acceptance: the production exec C ABI resolves guest→guest and
//! host/thunk imports through the reference `so_relocate_rel` /
//! `so_resolve_link` order: a symbol DEFINED in the module binds to its own
//! definition; an undefined import tries the `glad_`-rewritten name first,
//! then registered sources, then the guest image chain newest-loaded first,
//! then weak → 0.
//!
//! Fixtures are compiled from `tests/fixtures/rudroid86_*.c` by the test itself with
//! the canonical container's `cc` (same pattern as the unity_loader
//! `jni_on_load_fixture` probe); a missing compiler fails the test loudly
//! instead of skipping.
//!
//! The image chain and sources are process-global: every test takes the
//! SERIAL lock and unregisters what it registered.

#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

mod cc_fixture;

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use cc_fixture::fixture;

static SERIAL: Mutex<()> = Mutex::new(());

use android_elf_load::ExecutableModule;

fn load(path: &str) -> *mut ExecutableModule {
    // SAFETY: NUL-free path from CString; handle contract per C API.
    let handle = unsafe {
        android_elf_load::rudroid_elf_exec_load(
            CString::new(path.to_owned()).unwrap().as_ptr(),
        )
    };
    assert!(!handle.is_null(), "exec load must succeed for {path}: {}", last_error());
    handle
}

fn symbol(handle: *const ExecutableModule, name: &str) -> usize {
    // SAFETY: live handle; NUL-free name.
    unsafe {
        android_elf_load::rudroid_elf_exec_symbol(
            handle,
            CString::new(name).unwrap().as_ptr(),
        )
    }
}

fn call(handle: *const ExecutableModule, name: &str) -> i32 {
    let address = symbol(handle, name);
    assert_ne!(address, 0, "{name} must be exported");
    // SAFETY: the fixture exports `int name(void)` at this address.
    let function: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { function() }
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

#[allow(non_snake_case)]
unsafe extern "C" fn glad_glRudroid() -> i32 {
    311
}

unsafe extern "C" fn gl_rudroid_exact() -> i32 {
    299
}

unsafe extern "C" fn rudroid86_override_shadow() -> i32 {
    777
}

static RUDROID86_VALUE_SHADOW: i32 = 777;

/// The thunk-layer callback: `glad_glRudroid` always resolves; the override
/// and data shadows resolve only while armed.
unsafe extern "C" fn host_source_callback(name: *const c_char, user: *mut c_void) -> usize {
    // SAFETY: NUL-terminated name from the layered lookup.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
    let armed = unsafe { &*(user as *const AtomicBool) };
    match name.as_ref() {
        "glad_glRudroid" => glad_glRudroid as usize,
        "rudroid86_override" if armed.load(Ordering::Acquire) => rudroid86_override_shadow as usize,
        "rudroid86_value" if armed.load(Ordering::Acquire) => {
            &RUDROID86_VALUE_SHADOW as *const i32 as usize
        }
        _ => 0,
    }
}

/// A source answering BOTH the exact GL name and its `glad_` twin.
unsafe extern "C" fn gl_both_callback(name: *const c_char, _user: *mut c_void) -> usize {
    // SAFETY: NUL-terminated name from the layered lookup.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
    match name.as_ref() {
        "glRudroid" => gl_rudroid_exact as usize,
        "glad_glRudroid" => glad_glRudroid as usize,
        _ => 0,
    }
}

struct HostSource {
    id: c_int,
}

impl HostSource {
    /// # Safety
    /// `user` must outlive the registration (test static).
    unsafe fn register(
        callback: unsafe extern "C" fn(*const c_char, *mut c_void) -> usize,
        user: *mut c_void,
    ) -> HostSource {
        // SAFETY: 'static label and contract-conforming callback.
        let id = unsafe {
            android_elf_load::rudroid_elf_symbol_source_register(
                c"rudroid86-host".as_ptr(),
                callback,
                user,
            )
        };
        assert!(id >= 0, "source registration must succeed");
        HostSource { id }
    }

    fn unregister(&self) {
        // SAFETY: id from register.
        let rc = unsafe { android_elf_load::rudroid_elf_symbol_source_unregister(self.id) };
        assert_eq!(rc, 0, "source unregistration must succeed");
    }
}

fn armed_ptr(flag: &'static AtomicBool) -> *mut c_void {
    flag as *const AtomicBool as *mut c_void
}

/// Guest→guest and guest-data resolution through the PRODUCTION exec C ABI:
/// the consumer loads only after the provider; its data import reads the
/// provider's variable and its function import calls into the provider.
#[test]
fn guest_to_guest_imports_resolve_in_load_order() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = fixture("rudroid86_provider");
    let consumer_path = fixture("rudroid86_consumer");

    // Load order matters: without the provider, the consumer's strong
    // import cannot resolve and the load fails with the symbol named.
    // SAFETY: NUL-free path; null handle IS the expected outcome.
    let early = unsafe {
        android_elf_load::rudroid_elf_exec_load(
            CString::new(consumer_path.clone()).unwrap().as_ptr(),
        )
    };
    assert!(early.is_null(), "consumer must fail without its provider");
    let reason = last_error();
    assert!(reason.contains("rudroid86_value"), "the data import is the first unresolved strong symbol: {reason}");

    let provider = load(&provider_path);
    let consumer = load(&consumer_path);

    assert_eq!(call(consumer, "rudroid86_read"), 42, "data import binds to the provider");
    assert_eq!(call(consumer, "rudroid86_read_override"), 42, "function import binds");

    let provider_override = symbol(provider, "rudroid86_override");
    assert_ne!(provider_override, 0);

    // SAFETY: live handles.
    unsafe {
        android_elf_load::rudroid_elf_exec_destroy(consumer);
        android_elf_load::rudroid_elf_exec_destroy(provider);
    }
}

/// A registered host/thunk source resolves an undefined import BEFORE the
/// guest image chain (override probe: source 777 vs the provider's export
/// 42), the `glad_` rewrite resolves `glRudroid` through `glad_glRudroid`, and
/// revoking the source restores guest binding for later loads.
#[test]
fn host_sources_shadow_guest_imports_rewrite_glad_and_revoke() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = fixture("rudroid86_provider");
    let consumer_path = fixture("rudroid86_consumer");
    let gl_path = fixture("rudroid86_consumer_gl");

    static ARMED: AtomicBool = AtomicBool::new(false);
    // SAFETY: 'static callback and state.
    let source = unsafe { HostSource::register(host_source_callback, armed_ptr(&ARMED)) };

    let gl = load(&gl_path);
    assert_eq!(call(gl, "rudroid86_gl_read"), 311, "glad_glRudroid resolved via rewrite");
    // SAFETY: live handle.
    unsafe { android_elf_load::rudroid_elf_exec_destroy(gl) };

    ARMED.store(true, Ordering::Release);
    let _provider = load(&provider_path);
    let consumer = load(&consumer_path);
    assert_eq!(
        call(consumer, "rudroid86_read_override"),
        777,
        "the registered host source must shadow the earlier guest's export"
    );

    source.unregister();
    // Revocation affects NEW loads: this consumer binds to the guest export
    // again (42). Already-relocated bindings stay, matching the reference:
    // relocations are applied once at load.
    let consumer2 = load(&consumer_path);
    assert_eq!(call(consumer2, "rudroid86_read_override"), 42, "revoked source no longer shadows");
    ARMED.store(false, Ordering::Release);
}

/// Reference `so_relocate_rel`: a relocation against a symbol DEFINED in
/// the module binds to `base + st_value` directly — registered sources only
/// answer undefined imports. The provider's own GOT/PLT references keep
/// reaching its definitions while a source shadowing both names is armed.
#[test]
fn module_defined_symbols_bind_to_their_own_definition() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let provider_path = fixture("rudroid86_provider");

    static ARMED: AtomicBool = AtomicBool::new(true);
    // SAFETY: 'static callback and state.
    let source = unsafe { HostSource::register(host_source_callback, armed_ptr(&ARMED)) };
    let provider = load(&provider_path);
    let own_call = call(provider, "rudroid86_call_own");
    let own_data = call(provider, "rudroid86_get");
    source.unregister();
    assert_eq!(own_call, 42, "own PLT call binds to the own definition, not the source");
    assert_eq!(own_data, 42, "own GOT data access binds to the own definition");
}

/// Reference `so_resolve_link`: `glXxx` first tries `glad_glXxx`, and only
/// a miss falls back to the exact name.
#[test]
fn glad_rewrite_is_tried_before_the_exact_name() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let gl_path = fixture("rudroid86_consumer_gl");

    // SAFETY: 'static callback, no user state.
    let source = unsafe { HostSource::register(gl_both_callback, std::ptr::null_mut()) };
    let gl = load(&gl_path);
    let value = call(gl, "rudroid86_gl_read");
    source.unregister();
    assert_eq!(value, 311, "glad_glRudroid wins over the exact glRudroid");
}

/// Reference chain order (`mod->next = head; head = mod`): an import
/// resolves against the MOST RECENTLY loaded image exporting the name.
#[test]
fn guest_chain_prefers_the_most_recently_loaded_image() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let _first = load(&fixture("rudroid86_provider"));
    let _second = load(&fixture("rudroid86_provider_shadow"));
    let consumer = load(&fixture("rudroid86_consumer"));
    assert_eq!(call(consumer, "rudroid86_read"), 99, "data import binds to the newest image");
    assert_eq!(call(consumer, "rudroid86_read_override"), 99, "function import binds to the newest image");
}

/// Ticket 86: TLS symbols are not plain addresses — a lookup of an STT_TLS
/// export is rejected with a diagnostic instead of returning base+offset.
#[test]
fn tls_export_lookup_is_rejected_with_a_diagnostic() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let module = load(&fixture("rudroid86_tls"));
    assert_eq!(symbol(module, "rudroid86_tls"), 0, "TLS export must not resolve to an address");
    let reason = last_error();
    assert!(reason.contains("rudroid86_tls") && reason.contains("TLS"), "diagnostic names the TLS symbol: {reason}");
    assert_eq!(call(module, "rudroid86_plain"), 7, "ordinary exports still resolve");
    assert!(last_error().is_empty(), "a successful lookup leaves no error");
}

/// Constructor-time imports: the consumer's `.init_array` constructor calls
/// the provider during exec_load — imports resolve before constructors run,
/// in load order.
#[test]
fn constructor_imports_resolve_in_load_order() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let _provider = load(&fixture("rudroid86_provider"));
    let consumer = load(&fixture("rudroid86_consumer_ctor"));
    assert_eq!(
        call(consumer, "rudroid86_ctor_get"),
        42,
        "the constructor's provider import must have resolved at load time"
    );
}

/// Strong-missing semantics on the exec chain: with no source providing
/// `glRudroid`, the consumer fails the load and the symbol is named — no
/// silent stub, no partial publication.
#[test]
fn strong_missing_import_fails_naming_the_symbol() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no guest code runs in this process between tests.
    unsafe { android_elf_load::rudroid_elf_image_chain_reset() };
    let gl_path = fixture("rudroid86_consumer_gl");
    // SAFETY: NUL-free path; a null handle IS the expected outcome.
    let handle = unsafe {
        android_elf_load::rudroid_elf_exec_load(
            CString::new(gl_path).unwrap().as_ptr(),
        )
    };
    assert!(handle.is_null(), "unresolved strong import must fail the load");
    let text = last_error();
    assert!(!text.is_empty(), "the failure must set the error channel");
    assert!(text.contains("glRudroid"), "the missing symbol must be named: {text}");
}
