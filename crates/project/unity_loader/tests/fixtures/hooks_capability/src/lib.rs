//! Loader test fixture (ticket 53, role adjusted by ticket 77): a
//! platform-kind capability whose register-JNI phase registers the reference
//! Android runtime hooks (`RudroidAndroidHooks { flush_preferences,
//! signal_vsync }`) through the Host API v0 table — the ticket-45 dynamic
//! registration model, exactly like the C++ `android_base` plugin that
//! registers `bd_flush_prefs_impl` / `bd_signal_vsync`.
//!
//! Ticket 77 contract: the PRODUCTION android_base framework registers the
//! hooks first — android_base is the first RegisterJni phase in every valid
//! kit, so the registry's first-write-wins slot is already taken when this
//! fixture registers. Deployed alongside the real framework, the fixture's
//! registration is therefore REJECTED and the framework's hooks are the
//! effective ones: the fixture treats `REGISTRY_REJECTED` as the expected
//! "framework owns the hooks" outcome (logs it, the phase stays green) and
//! must never assert its own hooks fire in that kit. The fixture still wins
//! — and its logging hooks are the observable ones — only in diagnostic kits
//! whose android_base member registers no hooks (the ticket-44 probe as
//! `libandroid_base.so`, e.g. the ticket-65 ELF-diagnostic kit); such kits
//! are explicitly fixture-marked (the ticket-56 diagnostic-kit convention).
//!
//! This is a test fixture, not the framework: the hooks only LOG their
//! invocation through the host log so the integration tests can observe the
//! call sequence; the vsync signal itself is issued guest-side by the
//! ticket-53 spy (`rudroid_spy_signal_vsync`), and the loader invokes
//! `flush_preferences` on the clean profile-exit path.

use std::ffi::c_int;
use std::sync::OnceLock;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::RudroidAndroidHooks;

rudroid_common::declare_descriptor!("android_hooks_fixture", rudroid_common::Kind::Platform);

/// The table copy the hook callbacks log through (installed by the
/// register-JNI phase, before any hook can fire; the loader pins this
/// library for the process life).
static HOST: OnceLock<HostApiV0> = OnceLock::new();

fn log(message: &str) {
    if let Some(host) = HOST.get() {
        host.log_line(c"android_hooks_fixture", message);
    }
}

/// Table-bound hook; fires only after the register-JNI phase installed the
/// host table.
extern "C" fn flush_preferences() {
    log("flush_preferences invoked (profile exit path)");
}

/// Table-bound hook; fires only after the register-JNI phase installed the
/// host table.
extern "C" fn signal_vsync() {
    log("signal_vsync invoked");
}

#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    let _ = HOST.set(*api);
    let hooks = RudroidAndroidHooks {
        flush_preferences: Some(flush_preferences),
        signal_vsync: Some(signal_vsync),
    };
    // The host copies the POD during registration (ticket 45).
    let registered = (api.register_android_hooks)(Some(&hooks));
    if registered == 1 {
        // REGISTRY_REJECTED (ticket 45 first-write-wins): the production
        // android_base framework registered first (ticket 77 contract) and
        // ITS hooks are the effective ones. Not fatal — the fixture only
        // must not claim the hooks; the framework's events are the ones the
        // tests assert.
        api.log_line(
            c"android_hooks_fixture",
            "registration rejected: the production android_base \
             framework owns the runtime hooks (first-write-wins, ticket 77 \
             contract) — this fixture stays a passive member",
        );
        return 0;
    }
    if registered != 0 {
        api.log_line(
            c"android_hooks_fixture",
            &format!("register_android_hooks rejected the registration (code {registered})"),
        );
        return -4;
    }
    api.log_line(
        c"android_hooks_fixture",
        "Android runtime hooks registered (flush_preferences + signal_vsync)",
    );
    0
}
