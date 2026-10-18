//! Dev-only shared test facilities (ticket 81). Never a production
//! dependency: consumers take it as a dev-dependency, and the deploy kit
//! never sees it (no descriptor, not a capability).
//!
//! - [`serial_lock`]: process-global test serialization (one mutex per test
//!   binary; guards process-global state like the runtime registry).
//! - [`LogCapture`]: captured-log sink buffer with `contains`/`clear`.
//! - [`SubscriptionFake`]: per-kind module-subscription fake mirroring the
//!   host registry semantics (ticket 81 rework: multi-subscription,
//!   invoked outside the lock, mid-fire additions apply next fire).
//! - [`cargo`]: target-dir/profile assembly rules shared by the dynamic
//!   fixtures (tickets 73/75 conventions; ticket 81 rework + 84 gate): every
//!   nested cargo build (`--locked`, profile, kit features from
//!   `RUDROID_KIT_FEATURES`, variant target subdirs) and guest C fixture
//!   compile goes through it.
#![cfg_attr(not(test), forbid(unsafe_code))]

use std::ffi::{c_int, c_void};
use std::sync::{Mutex, MutexGuard, OnceLock};

use rudroid_common::registry::{RudroidModuleEvent as RudroidEvent, RudroidModuleLoadedCallback as RudroidCallback};

pub mod cargo;

/// Process-global test serialization. Poison-tolerant: a panicked test must
/// not wedge the suite.
pub fn serial_lock() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    let mutex = SERIAL.get_or_init(|| Mutex::new(()));
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Captured log lines ("tag: message"), process-global per test binary.
pub struct LogCapture {
    lines: &'static Mutex<Vec<String>>,
}



impl LogCapture {
    /// One capture per test binary; later calls return the same buffer.
    pub fn shared() -> LogCapture {
        static LINES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        LogCapture {
            lines: LINES.get_or_init(|| Mutex::new(Vec::new())),
        }
    }

    pub fn push(&self, tag: &str, message: &str) {
        self.lines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(format!("{tag}: {message}"));
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.lines().iter().any(|line| line.contains(needle))
    }

    pub fn clear(&self) {
        self.lines
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

/// Per-kind module-subscription fake mirroring the reference host
/// (`plugin_host.cpp::api_register_module_loaded` / `notify_module_loaded`,
/// restated in `common/src/registry.rs`): EVERY valid registration is kept
/// (multiple per kind — a second same-kind subscription is NOT rejected),
/// callbacks are invoked outside the storage lock from a snapshot taken at
/// fire start (a callback may subscribe again or fire again without
/// deadlocking), and subscriptions added during a fire take effect from the
/// next fire on. Null callbacks are rejected (the host returns -1; the fake
/// keeps the distinct `2` so tests can tell rejection from success).
pub struct SubscriptionFake {
    slots: Mutex<Vec<(u32, RudroidCallback, usize)>>,
}


impl Default for SubscriptionFake {
    fn default() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
        }
    }
}

impl SubscriptionFake {
    /// Register a callback for `kind`. Returns 0 and appends a new
    /// subscription (even for an already-subscribed kind — host parity);
    /// 2 for a null callback.
    pub fn register(
        &self,
        kind: u32,
        callback: Option<RudroidCallback>,
        user: *mut c_void,
    ) -> c_int {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        let Some(callback) = callback else {
            return 2;
        };
        slots.push((kind, callback, user as usize));
        0
    }

    /// Fire every subscription registered for `kind` at fire start, in
    /// registration order. Panics when nothing is registered (tests assert
    /// arming explicitly).
    pub fn fire(&self, kind: u32, base: usize, name: &str) {
        // Snapshot then release: callbacks run outside the storage lock
        // (registry contract), so a reentrant register/fire cannot deadlock
        // and mid-fire additions land only in later fires.
        let targets: Vec<(RudroidCallback, usize)> = {
            let slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
            slots
                .iter()
                .filter(|(k, _, _)| *k == kind)
                .map(|(_, c, u)| (*c, *u))
                .collect()
        };
        if targets.is_empty() {
            panic!("no subscription registered for kind {kind}");
        }
        // Event names are process-lifetime, as the loader's are.
        let event = RudroidEvent {
            kind,
            base: base,
            name: rudroid_common::registry::HostStr::new(rudroid_common::descriptor::leak_cstr(name)),
        };
        for (callback, user) in targets {
            callback(Some(&event), user as *mut c_void);
        }
    }

    /// The FIRST (callback, user) registered for `kind`, if any — callers
    /// wrap the callback pointer themselves (the fake never calls it).
    pub fn slot(&self, kind: u32) -> Option<(usize, usize)> {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(k, _, _)| *k == kind)
            .map(|(_, c, u)| (*c as usize, *u))
    }

    /// Every (callback, user) registered for `kind`, in registration order.
    pub fn slots_of(&self, kind: u32) -> Vec<(usize, usize)> {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(k, _, _)| *k == kind)
            .map(|(_, c, u)| (*c as usize, *u))
            .collect()
    }

    pub fn has_kind(&self, kind: u32) -> bool {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|(k, _, _)| *k == kind)
    }

    pub fn reset(&self) {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
