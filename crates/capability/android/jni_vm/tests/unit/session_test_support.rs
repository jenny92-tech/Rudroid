//! INPUT:  private items of `src/session.rs` via `super`
//! OUTPUT:  `test_support` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/session.rs`; declared there with `#[path]`
//! Test-only Host API table: the catalog query entries answer from a
//! process-global stub table, so in-process sessions (local or global)
//! exercise the same `catalog_query_*` seam the loader installs across
//! the registry (the real dynamic path is covered by the ticket-52
//! integration test with the deployed `reply_catalog` .so).

use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void};
use std::sync::Mutex;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::HostStr;

use super::MethodValue;

/// Serializes every test in this crate that touches a process-global
/// (the stub catalog table, the global session or the shared-VM slot
/// mirror). Lives here so both `session::tests` and the lib-level
/// `early_init` test share ONE lock — never take it twice on one path.
pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

pub type StubKey = (String, String, String);

fn stub_table() -> &'static Mutex<BTreeMap<StubKey, MethodValue>> {
    static STUB: Mutex<BTreeMap<StubKey, MethodValue>> = Mutex::new(BTreeMap::new());
    &STUB
}

pub fn stub_set(class: &str, method: &str, sig: &str, value: MethodValue) {
    stub_table().lock().unwrap().insert(
        (class.to_owned(), method.to_owned(), sig.to_owned()),
        value,
    );
}

pub fn stub_clear() {
    stub_table().lock().unwrap().clear();
}

fn read(text: HostStr<'_>) -> Option<String> {
    Some(text.to_cstr()?.to_string_lossy().into_owned())
}

extern "C" fn stub_query_bool(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut c_int>,
) -> c_int {
    let (Some(c), Some(m), Some(s)) = (read(class), read(method), read(sig)) else {
        return 0;
    };
    match stub_table()
        .lock()
        .unwrap()
        .get(&(c, m, s))
    {
        Some(MethodValue::Bool(b)) => {
            if let Some(out) = out {
                *out = c_int::from(*b);
            }
            1
        }
        _ => 0,
    }
}

extern "C" fn stub_query_int(
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int {
    let (Some(c), Some(m), Some(s)) = (read(class), read(method), read(sig)) else {
        return 0;
    };
    match stub_table()
        .lock()
        .unwrap()
        .get(&(c, m, s))
    {
        Some(MethodValue::Int(i)) => {
            if let Some(out) = out {
                *out = *i;
            }
            1
        }
        _ => 0,
    }
}

extern "C" fn stub_log(_tag: HostStr<'_>, _message: HostStr<'_>) {}
extern "C" fn stub_get_bool(_k: HostStr<'_>, fallback: c_int) -> c_int {
    fallback
}
extern "C" fn stub_get_i64(_k: HostStr<'_>, fallback: i64) -> i64 {
    fallback
}
extern "C" fn stub_get_string(_k: HostStr<'_>, _fallback: HostStr<'_>, _out: *mut c_char, _out_size: u32) -> c_int {
    0
}
extern "C" fn stub_register_shared_vm(vm: Option<rudroid_common::jni_env::JavaVm>) -> c_int {
    *SHARED_VM_SLOT.lock().unwrap() = vm.map_or(0, |vm| vm.as_raw() as usize);
    0
}

extern "C" fn stub_shared_vm() -> Option<rudroid_common::jni_env::JavaVm> {
    // SAFETY: stub_register_shared_vm receives the process-pinned session VM.
    unsafe { rudroid_common::jni_env::JavaVm::from_raw(*SHARED_VM_SLOT.lock().unwrap() as *mut c_void) }
}

// Test-only mirror of the registry's shared-VM slot. `usize` keeps the
// static `Send` (raw pointers are not) and the registry itself is just
// as opaque to the session — a bare pointer value.
pub static SHARED_VM_SLOT: Mutex<usize> = Mutex::new(0);

pub fn reset_shared_vm_slot() {
    *SHARED_VM_SLOT.lock().unwrap() = 0;
}

/// A HostApiV0 whose catalog entries answer from the stub table and
/// whose shared-VM slot behaves like the loader registry.

// Ordinary fixture paths have no extracted-pack overlay. This is the Host API
// v4 unhandled reply, allowing the stream tests to exercise their actual files.
unsafe extern "C" fn stub_asset_query(
    _:Option<&rudroid_common::android_assets::AssetRequest<'_>>, _: *mut u8, _:u32
)->i32 {0}

pub fn stub_host() -> HostApiV0 {
    HostApiV0 {
        config_get_bool: stub_get_bool,
        config_get_i64: stub_get_i64,
        // SAFETY: the static stub returns a miss without retaining or touching buffers.
        config_get_string: unsafe { rudroid_common::host_api::ConfigGetStringFn::new(stub_get_string) },
        log: stub_log,
        // SAFETY: pinned fixture callback returns unhandled and writes/retains nothing.
        asset_query: unsafe {rudroid_common::host_api::AssetQueryFn::new(stub_asset_query)},
        catalog_query_int: stub_query_int,
        catalog_query_bool: stub_query_bool,
        register_shared_vm: stub_register_shared_vm,
        shared_vm: stub_shared_vm,
        ..HostApiV0::stubbed()
    }
}
