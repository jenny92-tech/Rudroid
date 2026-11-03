//! rudroid-android-reply-catalog: fixed JNI replies as a same-build binary
//! blob (class/method/signature → value), never TOML, no encryption.
//!
//! - Source entries: `const` tables in `source` (single in-repo source of
//!   truth, compiled into the blob at build time).
//! - Binary blob: little-endian v1 format from `rudroid-reply-catalog-format`,
//!   rebuilt on every source change (rebuild-on-change, no compat matrix).
//! - Query model: `Catalog` lives in `rudroid-reply-catalog-format` (shared
//!   plain library, ticket 43); this crate re-exports it. It does **not**
//!   statically link other capabilities, and other capabilities must not
//!   statically link this one — the blob is consumed at runtime through the
//!   dynamic C API below.
//!
//! Runtime role (ticket 45): the cdylib's `early_init` phase registers a
//! `CatalogProviderV0` with the host registry, so every other capability in
//! the process queries the same catalog through the Host API
//! (`catalog_query_int` / `catalog_query_bool`), never by linking this crate.
//!
//! Runtime role (ticket 52): the `init` phase joins the shared JNI session
//! as the addon representative — its own fixed reply (100) comes back
//! through the JNI function table via the registry's shared-VM slot,
//! proving the session and the catalog ecosystem interoperate end to end.
//! Without a session the join degrades to a logged skip.
//!
//! The cdylib also exposes the direct C API documented in
//! `common::symbols::reply_catalog` (`rudroid_reply_catalog_*`, POD
//! in/out-params only).

pub mod ffi;

use std::ffi::{c_int, c_void};
use std::sync::{Arc, OnceLock};

use rudroid_common::handles::HandleTable;
use rudroid_common::host_api::HostApiV0;
use rudroid_common::jni_env::{Arg, JNI_VERSION_1_6};
use rudroid_common::registry::{CatalogProviderV0, HostStr};

rudroid_common::declare_descriptor!("reply_catalog", rudroid_common::Kind::Runtime);

pub use rudroid_reply_catalog_format::{
    decode, encode, Catalog, CatalogLoadError, DecodeError, Entry, ReplyValue,
};
pub use rudroid_reply_catalog_format::source::{SourceEntry, SOURCE_TABLE};

/// Decode the same-build blob once. Provider generations own a strong
/// reference to this SAME immutable data, never a cloned Catalog graph.
fn provider_catalog() -> &'static Arc<Catalog> {
    static CATALOG: OnceLock<Arc<Catalog>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/reply_catalog.bin"));
        Arc::new(Catalog::from_blob(bytes).expect("same-build catalog blob is valid"))
    })
}

/// One strong reference to the shared catalog per accepted provider
/// generation; the provider `user` is its handle.
static PROVIDERS: HandleTable<Catalog> = HandleTable::new();

/// Borrow the three query keys for one call; `None` for null or non-UTF-8.
fn keys<'a>(class: HostStr<'a>, method: HostStr<'a>, sig: HostStr<'a>) -> Option<(&'a str, &'a str, &'a str)> {
    let read = |key: HostStr<'a>| key.to_cstr()?.to_str().ok();
    Some((read(class)?, read(method)?, read(sig)?))
}

extern "C" fn provider_query_int(
    user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut i64>,
) -> c_int {
    let Some(catalog) = PROVIDERS.get(user) else {
        return 0;
    };
    let Some((class, method, sig)) = keys(class, method, sig) else {
        return 0;
    };
    match catalog.query(class, method, sig) {
        Some(ReplyValue::Int(value)) => {
            if let Some(out) = out {
                *out = *value;
            }
            1
        }
        _ => 0,
    }
}

extern "C" fn provider_query_bool(
    user: *mut c_void,
    class: HostStr<'_>,
    method: HostStr<'_>,
    sig: HostStr<'_>,
    out: Option<&mut c_int>,
) -> c_int {
    let Some(catalog) = PROVIDERS.get(user) else {
        return 0;
    };
    let Some((class, method, sig)) = keys(class, method, sig) else {
        return 0;
    };
    match catalog.query(class, method, sig) {
        Some(ReplyValue::Bool(value)) => {
            if let Some(out) = out {
                *out = *value as c_int;
            }
            1
        }
        _ => 0,
    }
}

/// Registry shutdown releases the generation's one reference, after all query
/// leases finished.
extern "C" fn provider_destroy(user: *mut c_void) {
    PROVIDERS.remove(user);
}

#[no_mangle]
extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    let user = PROVIDERS.insert_arc(Arc::clone(provider_catalog()));
    let provider = CatalogProviderV0 {
        user,
        query_int: Some(provider_query_int),
        query_bool: Some(provider_query_bool),
        destroy: Some(provider_destroy),
    };
    // `user` ownership transfers only on acceptance.
    if !api.register_catalog(&provider) {
        // Registration rejected this one ownership transfer (the registry's
        // only refusal for a non-null provider, REGISTRY_REJECTED = 1).
        // Release its Arc reference without touching any accepted owner.
        PROVIDERS.remove(user);
        return 1;
    }
    0
}

/// Ticket 52: as the addon representative, the catalog library joins the
/// SAME shared JNI session the framework and the guest use — its own fixed
/// reply (100) comes back through the JNI function table, having crossed the
/// session into the registry and back into this very library. A kit without
/// the session capability degrades to a logged skip; a session that fails
/// mid-join fails the phase.
#[no_mangle]
extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    let Some(vm) = api.java_vm() else {
        api.log_line(
            c"reply_catalog",
            "no JNI session registered — session join skipped \
             (deploy the android_jni_vm capability, ticket 52)",
        );
        return 0;
    };
    let log = |message: String| -> bool {
        api.log_line(c"reply_catalog", &message);
        true
    };
    let fail = |message: String| -> c_int {
        log(format!("session join FAILED: {message} (ticket 52)"));
        -3
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return fail("GetEnv on the shared VM did not yield a JNIEnv".to_owned());
    };
    let class = env.find_class(c"android/media/AudioManager");
    if class.is_null() {
        return fail("FindClass(android/media/AudioManager) returned null".to_owned());
    }
    let Some(method) = env.static_method(class, c"getStreamVolume", c"(I)I") else {
        return fail("GetStaticMethodID(getStreamVolume) returned null".to_owned());
    };
    let volume = env.call_static_int(class, method, &[Arg::int(3)]);
    if volume != 100 {
        return fail(format!(
            "CallStaticIntMethodA(getStreamVolume) returned {volume}, want \
             this library's own fixed reply 100"
        ));
    }
    log(format!(
        "session join ok volume={volume} (catalog value round-tripped \
         through the shared JNI session back into this library — ticket 52)"
    ));
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
