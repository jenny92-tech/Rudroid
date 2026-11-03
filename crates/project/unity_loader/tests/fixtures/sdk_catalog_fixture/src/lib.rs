//! Loader/kit fixture (ticket 56): a Runtime-kind capability whose early
//! init registers a CatalogProviderV0 answering the FIXED replies the addon
//! shared-session registrations consume (GMS `Task.isSuccessful == true`,
//! `AuthenticationResult.isAuthenticated == false` — oracle google_gms.cpp
//! values; play-core `Task.addOnCompleteListener` presence marker — the v1
//! session value model cannot carry the oracle's object return, so the
//! fixture carries a non-null Int marker and the join never calls it; FMOD
//! `checkInit == true`). The standard android_base entries are included so
//! the framework join answers inside the same kit.
//!
//! The production catalog (`reply_catalog`) stays untouched: its source
//! table is oracle-derived android_base data. This fixture exists so the
//! ticket-56 integration kits can prove the addon registrations through the
//! REAL session + registry + provider chain (ticket 70 threading contract
//! included: queries are immutable table lookups, safe for concurrent calls).
//! Fixture only — it registers no guest-visible Java classes of its own.

use std::ffi::{c_int, c_void};

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::HostStr;

rudroid_common::declare_descriptor!("sdk_fixture_catalog", rudroid_common::Kind::Runtime);

/// One fixed reply: class/method/signature → value.
struct Entry {
    class: &'static str,
    method: &'static str,
    sig: &'static str,
    kind: Kind,
    int: i64,
}

#[derive(PartialEq)]
enum Kind {
    Bool(bool),
    Int,
}

const ENTRIES: &[Entry] = &[
    // -- android_base standard set (oracle-derived, ticket 15) -------------
    Entry { class: "android/media/AudioManager", method: "isBluetoothA2dpOn", sig: "()Z", kind: Kind::Bool(false), int: 0 },
    Entry { class: "android/media/AudioManager", method: "getStreamVolume", sig: "(I)I", kind: Kind::Int, int: 100 },
    Entry { class: "android/media/AudioDeviceInfo", method: "getType", sig: "()I", kind: Kind::Int, int: 4 },
    // -- GMS (oracle google_gms.cpp fixed replies) --------------------------
    Entry { class: "com/google/android/gms/tasks/Task", method: "isSuccessful", sig: "()Z", kind: Kind::Bool(true), int: 0 },
    Entry { class: "com/google/android/gms/games/AuthenticationResult", method: "isAuthenticated", sig: "()Z", kind: Kind::Bool(false), int: 0 },
    // -- Play Core (presence marker; see the crate docs for the boundary) ---
    Entry {
        class: "com/google/android/play/core/tasks/Task",
        method: "addOnCompleteListener",
        sig: "(Lcom/google/android/play/core/tasks/OnCompleteListener;)Lcom/google/android/play/core/tasks/Task;",
        kind: Kind::Int,
        int: 1,
    },
    // -- FMOD (oracle fakefmod.cpp checkInit) -------------------------------
    Entry { class: "org/fmod/FMOD", method: "checkInit", sig: "()Z", kind: Kind::Bool(true), int: 0 },
];

fn lookup(class: HostStr<'_>, method: HostStr<'_>, sig: HostStr<'_>) -> Option<&'static Entry> {
    let read = |text: HostStr<'_>| Some(text.to_cstr()?.to_string_lossy().into_owned());
    let (Some(class), Some(method), Some(sig)) = (read(class), read(method), read(sig)) else {
        return None;
    };
    ENTRIES.iter().find(|entry| {
        entry.class == class && entry.method == method && entry.sig == sig
    })
}

extern "C" fn query_int(
    _user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int {
    match lookup(class, method, sig) {
        Some(entry) if matches!(entry.kind, Kind::Int) => {
            if let Some(out) = out {
                *out = entry.int;
            }
            1
        }
        _ => 0,
    }
}

extern "C" fn query_bool(
    _user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut c_int>,
) -> c_int {
    match lookup(class, method, sig) {
        Some(entry) => {
            let Kind::Bool(value) = entry.kind else { return 0 };
            if let Some(out) = out {
                *out = c_int::from(value);
            }
            1
        }
        _ => 0,
    }
}

extern "C" fn destroy(_user: *mut c_void) {}

#[no_mangle]
pub extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    let provider = rudroid_common::registry::CatalogProviderV0 {
        user: std::ptr::null_mut(),
        query_int: Some(query_int),
        query_bool: Some(query_bool),
        destroy: Some(destroy),
    };
    let registered = (api.register_catalog_provider)(Some(&provider));
    let tag = c"sdk_fixture_catalog";
    if registered != 0 {
        api.log_line(
            tag,
            &format!(
                "catalog provider registration rejected with code {registered} \
                 (a provider is already registered)"
            ),
        );
        return -3;
    }
    api.log_line(tag, &format!("fixture catalog registered: {} entries", ENTRIES.len()));
    0
}
