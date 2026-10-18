//! INPUT: shared Host API, JNI requests and catalog/native capability registrations
//! OUTPUT: one process-wide VM with checked Java objects, references and dispatch
//! POS: authoritative JNI service; Java primitives use safe owned-data helpers
//! rudroid-android-jni-vm: the minimal shared JNI session (ticket 52)
//! plus the catalog hit/miss path for unresolved guest JNI calls (ticket
//! 13).
//!
//! Ticket 52 adds the dynamic closed loop the C++ kit gets from
//! `libjnivm`: a real `JavaVM*`/`JNIEnv` C function-table pair (see
//! [`abi`] for the layout and the implemented/explicit-failure slot split)
//! backing one process-wide session ([`session`]). The early-init phase
//! creates the session with the loader's Host API table and registers its
//! `JavaVM*` in the runtime registry's shared-VM slot (ticket 45), so a
//! guest `JNI_OnLoad` — called by the loader from the mapped exec image —
//! obtains a valid session whose method calls resolve through the
//! registered catalog provider (the `reply_catalog` .so). The framework
//! (`android_base`) and the catalog library join the SAME session, sharing
//! its reference semantics.
//!
//! Scope: [`abi::ENV_IMPLEMENTED`] lists the implemented env entries; every
//! other slot is an explicit named failure. The pre-52 [`Resolver`]
//! model (method key -> catalog fixed reply or caller default) stays for
//! in-process consumers; the session layer adds the strict JNI-visible
//! semantics on top.
//!
//! Catalog access: the session consults the registry through the Host API
//! table it is created with. The `Catalog` value for the [`Resolver`] is
//! injected by the caller (`Resolver::with_catalog`). This capability never
//! statically links or embeds another capability (ticket 43).
//!
//! Miss behavior (Resolver): a miss is normal (most guest JNI calls are
//! served by registered stubs elsewhere); when configured, the resolver
//! emits one `STUB-MISS` log line per miss so bring-up gaps stay visible.
//! This crate only logs; it never edits the C++ libjnivm.

pub mod abi;
pub mod session;
mod integers;

use rudroid_reply_catalog_format::{Catalog, ReplyValue};

rudroid_common::declare_descriptor!("android_jni_vm", rudroid_common::Kind::Runtime);

/// Creates the process session and registers its `JavaVM*` in the
/// registry's shared-VM slot. Idempotent across repeated bring-ups in one
/// process: when the slot already holds THIS session's VM pointer, the phase
/// succeeds without re-registering (registry first-write-wins).
#[no_mangle]
extern "C" fn rudroid_plugin_early_init(
    api: Option<&'static rudroid_common::host_api::HostApiV0>,
) -> std::ffi::c_int {
    /// The loader's `register_shared_vm` code for an occupied slot (the only
    /// failure for a non-null VM).
    const REGISTRY_REJECTED: std::ffi::c_int = 1;
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    // HostApiV0 is a POD C table (fn pointers, no ownership) — a bitwise
    // copy is what the session keeps, the same copy the registry makes for
    // provider registrations.
    session::init_with_host(*api);
    // SAFETY: the session's own `JavaVM*`: a static invoke table, live for the process.
    let vm = unsafe { rudroid_common::jni_env::JavaVm::from_raw(abi::java_vm_ptr()) }
        .expect("the session JavaVM* is a static table");
    let registered = api.register_vm(vm);
    let already = api.java_vm();
    if !registered {
        if already != Some(vm) {
            // A foreign pointer owns the slot: a different session won the
            // race — same-build kits cannot produce this; fail the phase.
            return REGISTRY_REJECTED;
        }
        // Our own earlier registration (repeat bring-up): idempotent OK.
        return 0;
    }
    api.log_line(
        c"android_jni_vm",
        "session created: JavaVM/JNIEnv tables live, shared-VM slot \
         registered (ticket 52 minimal JNI session)",
    );
    0
}

/// A guest JNI method identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodKey {
    /// JVM internal name, e.g. `android/media/AudioManager`.
    pub class: String,
    pub method: String,
    /// JNI descriptor, e.g. `()Z`.
    pub sig: String,
}

impl MethodKey {
    pub fn new(class: &str, method: &str, sig: &str) -> MethodKey {
        MethodKey {
            class: class.to_owned(),
            method: method.to_owned(),
            sig: sig.to_owned(),
        }
    }
}

/// Fixed-reply resolution over the Reply Catalog.
pub struct Resolver {
    catalog: Catalog,
    /// Called with the missing key on every catalog miss when installed.
    pub on_miss: Option<Box<dyn Fn(&MethodKey) + Send + Sync>>,
}

impl Resolver {
    /// Resolver over a caller-supplied catalog (decoded from a blob, loaded
    /// from a path, or obtained across the dynamic catalog C API).
    pub fn with_catalog(catalog: Catalog) -> Resolver {
        Resolver {
            catalog,
            on_miss: None,
        }
    }

    fn record_miss(&self, key: &MethodKey) {
        if let Some(on_miss) = &self.on_miss {
            on_miss(key);
        }
    }

    /// Exact catalog hit, `None` on miss (caller decides the default).
    pub fn hit(&self, key: &MethodKey) -> Option<&ReplyValue> {
        self.catalog.query(&key.class, &key.method, &key.sig)
    }

    pub fn resolve_bool(&self, key: &MethodKey, default: bool) -> bool {
        match self.hit(key) {
            Some(ReplyValue::Bool(value)) => *value,
            _ => {
                self.record_miss(key);
                default
            }
        }
    }

    pub fn resolve_int(&self, key: &MethodKey, default: i64) -> i64 {
        match self.hit(key) {
            Some(ReplyValue::Int(value)) => *value,
            _ => {
                self.record_miss(key);
                default
            }
        }
    }

    pub fn resolve_str(&self, key: &MethodKey, default: &str) -> String {
        match self.hit(key) {
            Some(ReplyValue::Str(value)) => value.clone(),
            _ => {
                self.record_miss(key);
                default.to_owned()
            }
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
