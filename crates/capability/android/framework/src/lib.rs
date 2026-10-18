//! INPUT: shared JNI/Host API and safe Android service facades
//! OUTPUT: android_base registration, runtime hooks and JNI framework behavior
//! POS: keeps platform-facing Android mock business logic outside native pointer boundaries
//! rudroid-android-framework: registers the guest `java.*` / `android.*`
//! surface (ticket 14), the Android runtime hooks (ticket 16) and — since
//! ticket 52 — joins the shared JNI session during the register-JNI phase.
//!
//! Rust counterpart of the read-only `android_base` plugin role. The surface
//! registry binds explicit fixed replies for a documented minimal class set;
//! anything unbound resolves through the injected catalog fallback
//! (`CatalogFallback`), and a miss stays visible via the resolver's
//! `STUB-MISS` callback. The fallback is a seam installed by the runtime
//! wiring — typically the `android_jni_vm` resolver over the dynamically
//! loaded `reply_catalog` C API (cross-library registration, ticket 45);
//! this capability never statically links other capabilities (ticket 43).
//! The class set grows iteratively toward full Parity with `javastubs/`.
//!
//! Ticket 52 session join: `rudroid_plugin_register_jni` resolves the
//! registry's shared `JavaVM*`, walks GetEnv → FindClass → GetStaticMethodID
//! → CallStaticIntMethodA on the guest-visible JNI tables and holds a
//! global class reference as this library's session anchor — the same
//! session and reference semantics the guest sees. Without a session the
//! join degrades to a logged skip; a broken session fails the phase.
//!
//! Ticket 16: `AndroidHooks { flush_preferences, signal_vsync }` mirrors the
//! C++ `RudroidAndroidHooks` (android_base registers `flush_preferences` from
//! `jnivm::android::content::SharedPreferences::flush_all` and
//! `signal_vsync` from `Choreographer::signalAllVSync`); here the hooks are
//! registry slots the Unity loop wiring (ticket 31+) can invoke.
//!
//! Ticket 77: this library now performs the reference registration itself —
//! `rudroid_plugin_register_jni` registers both hooks through the existing
//! `register_android_hooks` C ABI (android_base.cpp registers them inside
//! its register-JNI phase too), so the production kit satisfies the
//! profile-prepare gate ("flush_preferences and signal_vsync must be
//! registered") with NO fixture member in the deploy dir. `flush_preferences`
//! persists dirty shared preference stores at the reference flush point; `signal_vsync` bumps an atomic frame
//! counter without logging each frame by default (`unity.log_vsync_frames`
//! enables diagnostics). The `render_sleep_us` render-loop pacing stays in
//! the profile capabilities — no Host API surface exists for a
//! cross-capability wake handoff, so this remains the counter half.

use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
mod choreographer;
mod collections;
mod input;
mod java;
mod keycodes;
mod lifecycle;
mod preferences;
mod services;
mod statfs;
mod threads;
mod trust_store;
mod uri;

use rudroid_common::jni_env::{Arg, JNI_VERSION_1_6};
use rudroid_common::registry::RudroidAndroidHooks;

/// The Host API table copy the production hooks log through (ticket 77).
/// Installed by the register-JNI phase before any hook can fire — the loader
/// pins this library for the process life (ticket 46), and the table itself
/// is a process static on the loader side.
static HOST: OnceLock<rudroid_common::host_api::HostApiV0> = OnceLock::new();

/// Choreographer frame-signal counter (ticket 77): every `signal_vsync`
/// invocation bumps it atomically. Optional diagnostic logs expose the
/// count; the default hot path never formats, allocates or calls the logger.
static VSYNC_FRAMES: AtomicU64 = AtomicU64::new(0);
static LOG_VSYNC_FRAMES: AtomicBool = AtomicBool::new(false);

/// Table-bound hook; fires only after the register-JNI phase installed the
/// host table (the registration contract keeps the registered pointers
/// process-lifetime, ticket 45).
extern "C" fn flush_preferences_hook() {
    preferences::flush();
    if let Some(host) = HOST.get() {
        host.log_line(
            c"android_base",
            "flush_preferences: dirty shared preference stores flushed",
        );
    }
}

/// Table-bound hook; fires only after the register-JNI phase installed the
/// host table (the registration contract keeps the registered pointers
/// process-lifetime, ticket 45).
extern "C" fn signal_vsync_hook() {
    let frame = VSYNC_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if !LOG_VSYNC_FRAMES.load(Ordering::Relaxed) {
        return;
    }
    if let Some(host) = HOST.get() {
        host.log_line(
            c"android_base",
            &format!(
                "signal_vsync: frame {frame} signalled (Choreographer::signalAllVSync \
                 analogue, atomic frame counter — ticket 77)"
            ),
        );
    }
}

/// Register the production Android runtime hooks through the existing
/// `register_android_hooks` C ABI (ticket 77) — the reference android_base
/// registers the same pair inside its register-JNI phase and fails that
/// phase if the registration is refused (android_base.cpp:58-66). The
/// registry keeps its first-write-wins semantics unchanged; a rejection is
/// only reachable on an in-process re-bring-up (android_base is the
/// phase-first registrar in every valid kit, so the prior owner is this same
/// pinned library) and is logged instead of fatal — the C++ host has no
/// in-process re-bring-up, so its fatal arm has no Rust-side counterpart
/// without breaking the runtime-registry suites that bring a kit up twice.
fn register_android_runtime_hooks(api: &rudroid_common::host_api::HostApiV0) {
    // Read configuration once during registration, never in the frame hook.
    let log_frames = api.config_bool(c"unity.log_vsync_frames", false);
    LOG_VSYNC_FRAMES.store(log_frames, Ordering::Relaxed);
    let hooks = RudroidAndroidHooks {
        flush_preferences: Some(flush_preferences_hook),
        signal_vsync: Some(signal_vsync_hook),
    };
    // The host's only rejection code is REGISTRY_REJECTED (1).
    let message = if api.register_hooks(&hooks) {
        "android runtime hooks registered: flush_preferences + signal_vsync \
         (reference android_base parity — ticket 77)"
    } else {
        "android runtime hooks already registered — keeping the first \
         registration (registry first-write-wins, code 1; ticket 77)"
    };
    api.log_line(c"android_base", message);
}

use rudroid_reply_catalog_format::ReplyValue;

rudroid_common::declare_descriptor!("android_base", rudroid_common::Kind::AndroidBase);

/// Deployed-kit behaviour of ticket 45: the register-JNI phase queries the catalog
/// provider through the Host API registry — proving the query happens across
/// the dynamic boundary (the provider lives in `reply_catalog`'s library),
/// not through any static link. Ticket 52: the phase additionally joins the
/// shared JNI session registered by `android_jni_vm` — same `JavaVM*`, same
/// reference lifecycle as the guest — and pulls the catalog's known value
/// through the JNI function table. A kit without the session capability
/// degrades to a logged skip; a session that fails mid-join fails the phase.
#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(
    api: Option<&'static rudroid_common::host_api::HostApiV0>,
) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    // The production hooks log through this table; install it before any
    // hook can fire (the registration below is the earliest fire point).
    let _ = HOST.set(*api);
    let (hit, value) = api
        .catalog_int(c"android/media/AudioManager", c"getStreamVolume", c"(I)I")
        .map_or((0, -1), |value| (1, value));
    api.log_line(c"android_base", &format!("catalog getStreamVolume hit={hit} value={value}"));

    // Ticket 77: the reference android_base registers the Android runtime
    // hooks inside this phase (before any session work); the profile-prepare
    // gate refuses to run without them.
    register_android_runtime_hooks(api);

    join_shared_session(api)
}

/// Join the shared JNI session (ticket 52): resolve the same fixed reply
/// through the JNI tables the guest sees, keep a global class reference as
/// this library's session anchor, and log the provenance. Missing session:
/// explicit logged skip (the registry degrades the same way for the catalog
/// provider); broken session: non-zero phase failure.
fn join_shared_session(api: &rudroid_common::host_api::HostApiV0) -> c_int {
    let Some(vm) = api.java_vm() else {
        api.log_line(
            c"android_base",
            "no JNI session registered — session join skipped \
             (deploy the android_jni_vm capability, ticket 52)",
        );
        return 0;
    };
    let log = |message: String| api.log_line(c"android_base", &message);
    let fail = |message: String| -> c_int {
        log(format!("session join FAILED: {message} (ticket 52)"));
        -3
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return fail("GetEnv on the shared VM did not yield a JNIEnv".to_owned());
    };
    let class_name = c"android/media/AudioManager";
    let local = env.find_class(class_name);
    if local.is_null() {
        return fail("FindClass(android/media/AudioManager) returned null".to_owned());
    }
    let global = env.new_global_ref(local);
    env.delete_local_ref(local);
    if global.is_null() {
        return fail("NewGlobalRef on the framework class returned null".to_owned());
    }
    // `global` is deliberately never deleted: this library's anchor in
    // the shared session's reference tables (process-pinned, ticket 46).
    let Some(method) = env.static_method(global, c"getStreamVolume", c"(I)I") else {
        return fail("GetStaticMethodID(getStreamVolume) returned null".to_owned());
    };
    let volume = env.call_static_int(global, method, &[Arg::int(3)]);
    if volume != 100 {
        return fail(format!(
            "CallStaticIntMethodA(getStreamVolume) returned {volume}, want the \
             catalog fixed reply 100"
        ));
    }
    if java::register(env) != 0 {
        return fail("Android Java method registration failed".to_owned());
    }
    log(format!(
        "session join ok volume={volume} (same JavaVM as the guest, \
         global class ref held — ticket 52)"
    ));
    0
}

/// Test-only fake JNI tables for the session join: a VM table whose GetEnv
/// yields an env table with the join's slots filled by recording shims, so
/// the join logic is exercised without linking the `android_jni_vm`
/// capability (ticket 43; the real pair runs in the ticket-52 integration
/// test).
#[cfg(test)]
#[path = "../tests/unit/lib_jni_fake.rs"]
mod jni_fake;

/// Unbound-call fallback: given `(class, method, sig)`, answer the configured
/// fixed reply or `None` for a miss. Installed across the dynamic boundary by
/// the runtime wiring (ticket 45); tests inject catalog-backed closures.
pub type CatalogFallback = Box<dyn Fn(&str, &str, &str) -> Option<ReplyValue> + Send + Sync>;

/// A method bound to a fixed reply (registered surface) — takes precedence
/// over the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundMethod {
    pub method: &'static str,
    pub sig: &'static str,
    pub reply: ReplyValue,
}

/// One registered guest class.
#[derive(Debug, Clone, Default)]
pub struct ClassSurface {
    pub name: &'static str,
    pub methods: Vec<BoundMethod>,
}

/// Outcome of a surface call.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Served by an explicitly bound method.
    Bound(ReplyValue),
    /// Served by the injected catalog fallback.
    Catalog(ReplyValue),
    /// No binding and no fallback hit — caller default applies.
    Miss,
}

impl Outcome {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Outcome::Bound(ReplyValue::Bool(b)) | Outcome::Catalog(ReplyValue::Bool(b)) => Some(*b),
            _ => None,
        }
    }
}

/// The registered guest framework surface.
pub struct Framework {
    fallback: Option<CatalogFallback>,
    classes: Vec<ClassSurface>,
}

impl Default for Framework {
    fn default() -> Self {
        Framework::new()
    }
}

impl Framework {
    /// Framework without a catalog fallback: unbound calls miss until the
    /// runtime wiring installs one.
    pub fn new() -> Framework {
        Framework {
            fallback: None,
            classes: Vec::new(),
        }
    }

    /// Framework over a caller-built catalog fallback.
    pub fn with_fallback(fallback: CatalogFallback) -> Framework {
        Framework {
            fallback: Some(fallback),
            classes: Vec::new(),
        }
    }

    /// Install or replace the catalog fallback.
    pub fn set_fallback(&mut self, fallback: CatalogFallback) {
        self.fallback = Some(fallback);
    }

    pub fn fallback(&self) -> Option<&CatalogFallback> {
        self.fallback.as_ref()
    }

    /// Register a class surface. Re-registering the same name replaces it
    /// (registration order is not load order — the surface is a set).
    pub fn register_class(&mut self, surface: ClassSurface) {
        if let Some(slot) = self.classes.iter_mut().find(|c| c.name == surface.name) {
            *slot = surface;
        } else {
            self.classes.push(surface);
        }
    }

    pub fn classes(&self) -> &[ClassSurface] {
        &self.classes
    }

    /// Resolve a method call: bound method first, then the injected catalog
    /// fallback, then miss.
    pub fn call(&self, class: &str, method: &str, sig: &str) -> Outcome {
        if let Some(surface) = self.classes.iter().find(|c| c.name == class) {
            if let Some(bound) = surface
                .methods
                .iter()
                .find(|m| m.method == method && m.sig == sig)
            {
                return Outcome::Bound(bound.reply.clone());
            }
        }
        match self.fallback.as_ref().and_then(|f| f(class, method, sig)) {
            Some(reply) => Outcome::Catalog(reply),
            None => Outcome::Miss,
        }
    }
}

/// Android runtime hooks (ticket 16), analogous to the C++ `RudroidAndroidHooks`.
/// `None` slots mean the corresponding surface is not wired in this build —
/// callers treat invoking them as a programming error and log instead.
#[derive(Default)]
pub struct AndroidHooks {
    /// `SharedPreferences::flush_all` analogue.
    pub flush_preferences: Option<Box<dyn Fn() + Send + Sync>>,
    /// `Choreographer::signalAllVSync` analogue.
    pub signal_vsync: Option<Box<dyn Fn() + Send + Sync>>,
}

impl AndroidHooks {
    /// The documented initial registration from the android_base role: both
    /// hooks wired with process-local no-op-ready implementations. Tests
    /// install counting closures instead.
    pub fn register(
        &mut self,
        flush_preferences: Box<dyn Fn() + Send + Sync>,
        signal_vsync: Box<dyn Fn() + Send + Sync>,
    ) {
        self.flush_preferences = Some(flush_preferences);
        self.signal_vsync = Some(signal_vsync);
    }

    pub fn flush_preferences(&self) {
        if let Some(hook) = &self.flush_preferences {
            hook();
        }
    }

    pub fn signal_vsync(&self) {
        if let Some(hook) = &self.signal_vsync {
            hook();
        }
    }
}

/// The minimal documented class set this ticket registers (names track the
/// javastubs the C++ android_base registers; only AudioManager/
/// AudioDeviceInfo carry bound methods because their fixed replies now live
/// in the catalog — see ticket 15).
pub fn minimal_class_set() -> Vec<ClassSurface> {
    vec![
        ClassSurface {
            name: "android/media/AudioManager",
            methods: vec![],
        },
        ClassSurface {
            name: "android/media/AudioDeviceInfo",
            methods: vec![],
        },
        ClassSurface {
            name: "android/view/Choreographer",
            methods: vec![],
        },
        ClassSurface {
            name: "android/content/SharedPreferences",
            methods: vec![],
        },
    ]
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
