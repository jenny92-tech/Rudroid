//! INPUT:  shared HostApi, player lifecycle, physical/swap pressure and Unity metadata
//! OUTPUT:  optional memory notification/reclamation or bounded read-only developer snapshots
//! POS:    preserves engine ownership and kernel swap policy on the selected player thread

mod engine;
mod policy;
mod pressure;

use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::il2cpp::Il2cpp;
use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::ModuleKind;
use engine::Engine;
use policy::{Action, ReclaimPolicy};
use pressure::{MemorySource, PressureGate};
use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant};

rudroid_common::declare_descriptor!(
    "unity_memory_policy",
    rudroid_common::Kind::Runtime,
    package = None,
    requires = Some("il2cpp_patch")
);

static HOST: OnceLock<HostApiV0> = OnceLock::new();
static CONFIG: OnceLock<Config> = OnceLock::new();
static API: Mutex<Option<Il2cpp>> = Mutex::new(None);
static STOPPED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static OWNER: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

fn log(message: &str) {
    if let Some(host) = HOST.get() {
        host.log_line(c"UNITY_MEMORY", message);
    }
}

#[derive(Clone, Copy)]
struct Config {
    reserve_kib: u64,
    recovery_kib: u64,
    cooldown: Duration,
    warmup: Duration,
    collect_managed: bool,
    notify: bool,
    unload: bool,
    notification_grace: Duration,
    diagnostic_only: bool,
    diagnostic_interval: Duration,
    diagnostic_samples: u32,
}

impl Config {
    fn read(host: &HostApiV0) -> Result<Self, &'static str> {
        let read = |key: &std::ffi::CStr, fallback| host.config_i64(key, fallback);
        let reserve = read(c"unity_memory.reserve_mib", 64);
        let recovery = read(
            c"unity_memory.recovery_mib",
            reserve.saturating_mul(2).max(128),
        );
        let cooldown = read(c"unity_memory.cooldown_ms", 30000);
        let warmup = read(c"unity_memory.warmup_ms", 5000);
        let grace = read(c"unity_memory.notification_grace_ms", 1000);
        let diagnostic_only = host.config_bool(c"unity_memory.diagnostic_only", false);
        let diagnostic_interval = read(c"unity_memory.diagnostic_interval_ms", 5000);
        let diagnostic_samples = read(c"unity_memory.diagnostic_samples", 32);
        if diagnostic_only && (!(1000..=60000).contains(&diagnostic_interval) || !(1..=120).contains(&diagnostic_samples)) {
            return Err("unity_memory diagnostic interval must be 1000..60000ms and samples 1..120");
        }
        if !(0..=1_048_576).contains(&reserve)
            || !(1..=1_048_576).contains(&recovery)
            || recovery <= reserve
        {
            return Err("unity_memory requires 0 <= reserve_mib < recovery_mib <= 1048576");
        }
        if !(1000..=3_600_000).contains(&cooldown) || !(0..=600_000).contains(&warmup) {
            return Err("unity_memory cooldown_ms must be 1000..3600000 and warmup_ms 0..600000");
        }
        if !(0..=10_000).contains(&grace) {
            return Err("unity_memory notification_grace_ms must be 0..10000");
        }
        Ok(Self {
            reserve_kib: reserve as u64 * 1024,
            recovery_kib: recovery as u64 * 1024,
            cooldown: Duration::from_millis(cooldown as u64),
            warmup: Duration::from_millis(warmup as u64),
            collect_managed: !diagnostic_only && host.config_bool(c"unity_memory.collect_managed", false),
            notify: !diagnostic_only && host.config_bool(c"unity_memory.notify_low_memory", true),
            // Extra engine scans/unloads may stall or reload assets. Require
            // explicit per-title acceptance instead of enabling by installation.
            unload: !diagnostic_only && host.config_bool(c"unity_memory.unload_unused_assets", false),
            notification_grace: Duration::from_millis(grace as u64),
            diagnostic_only,
            diagnostic_interval: Duration::from_millis(diagnostic_interval.max(0) as u64),
            diagnostic_samples: diagnostic_samples.max(0) as u32,
        })
    }
}

#[derive(Debug)]
struct Measurement {
    residency: pressure::Residency,
    managed_used_bytes: Option<i64>,
    managed_heap_bytes: Option<i64>,
}

impl Measurement {
    fn read(engine: &Engine) -> Self {
        let residency = pressure::residency();
        let (managed_used_bytes, managed_heap_bytes) = engine.heap_bytes();
        Self {
            residency,
            managed_used_bytes: managed_used_bytes.filter(|bytes| *bytes >= 0),
            managed_heap_bytes: managed_heap_bytes.filter(|bytes| *bytes >= 0),
        }
    }
    fn description(&self) -> String {
        format!(
            "rss_kib={:?} pss_kib={:?} managed_used_bytes={} managed_heap_bytes={} swap_kib={:?} swap_pss_kib={:?} pss_plus_swap_pss_kib={:?}",
            self.residency.rss_kib, self.residency.pss_kib,
            self.managed_used_bytes.map(|v| v.to_string()).unwrap_or_else(|| "unavailable".into()),
            self.managed_heap_bytes.map(|v| v.to_string()).unwrap_or_else(|| "unavailable".into()),
            self.residency.swap_kib, self.residency.swap_pss_kib,
            self.residency.pss_kib.zip(self.residency.swap_pss_kib).and_then(|(ram, swap)| ram.checked_add(swap))
        )
    }
}

struct Controller {
    source: MemorySource,
    engine: Engine,
    policy: ReclaimPolicy,
    config: Config,
    start: Instant,
    pending_since: Option<Instant>,
    before: Option<Measurement>,
    disabled: bool,
    sample_error: bool,
    pending_warning: bool,
    requests: u64,
    completed: u64,
    notifications: u64,
    recovered: u64,
    gc_runs: u64,
    next_diagnostic: Duration,
    diagnostic_remaining: u32,
}

impl Controller {
    fn new(api: Il2cpp, config: Config) -> Result<Self, String> {
        let engine = Engine::discover(api)?;
        let notify = config.notify && engine.supports_notification();
        let unload = config.unload && engine.supports_cleanup();
        if !notify && !unload && !config.diagnostic_only {
            return Err("no enabled, validated memory mechanism".into());
        }
        if let Some(reason) = engine.cleanup_unavailable() {
            log(&format!(
                "engine cleanup unavailable: {reason}; validated notification remains independent"
            ));
        }
        if config.collect_managed && !engine.supports_gc() {
            log("extra managed GC unavailable: optional export missing; standard notification/unload preserved");
        }
        let surface = if engine.supports_cleanup() {
            "UnloadUnusedAssets/isDone"
        } else {
            "low-memory notification"
        };
        log(&format!("ready: metadata-validated {surface}; unity_version={} low_memory_notification={} reserve_kib={} recovery_kib={} cooldown_ms={} collect_managed={} unload_enabled={} notification_grace_ms={} diagnostic_only={}",
            engine.unity_version, notify, config.reserve_kib, config.recovery_kib, config.cooldown.as_millis(), config.collect_managed, unload, config.notification_grace.as_millis(), config.diagnostic_only));
        Ok(Self {
            source: MemorySource::discover(),
            engine,
            policy: ReclaimPolicy::new(
                PressureGate::new(
                    config.reserve_kib,
                    config.recovery_kib,
                    config.cooldown,
                    config.warmup,
                ),
                config.recovery_kib,
                config.notification_grace,
                notify,
                unload,
            ),
            config,
            start: Instant::now(),
            pending_since: None,
            before: None,
            disabled: false,
            sample_error: false,
            pending_warning: false,
            requests: 0,
            completed: 0,
            notifications: 0,
            recovered: 0,
            gc_runs: 0,
            next_diagnostic: config.warmup,
            diagnostic_remaining: config.diagnostic_samples,
        })
    }

    fn tick(&mut self) {
        if self.disabled || STOPPED.load(Ordering::Acquire) {
            return;
        }
        if self.pending_since.is_none() && self.start.elapsed() < self.config.warmup {
            return;
        }
        if self.config.diagnostic_only {
            let elapsed = self.start.elapsed();
            if self.diagnostic_remaining == 0 {
                self.disabled = true;
                return;
            }
            if elapsed >= self.next_diagnostic {
                let sample = Measurement::read(&self.engine);
                self.diagnostic_remaining -= 1;
                self.next_diagnostic = elapsed.saturating_add(self.config.diagnostic_interval);
                log(&format!("diagnostic-only elapsed_ms={} remaining={} [{}]; no notification/unload/GC", elapsed.as_millis(), self.diagnostic_remaining, sample.description()));
                if self.diagnostic_remaining == 0 {
                    self.disabled = true;
                }
            }
            return;
        }
        if let Some(pending_since) = self.pending_since {
            match self.engine.poll() {
                Ok(false) => {
                    if !self.pending_warning && pending_since.elapsed() > Duration::from_secs(30) {
                        log("unload still pending after 30s; frames continue, no overlapping request");
                        self.pending_warning = true;
                    }
                }
                Ok(true) => {
                    let async_elapsed_ms = pending_since.elapsed().as_millis();
                    let gc = Instant::now();
                    let gc_performed = self.config.collect_managed && self.engine.collect();
                    self.gc_runs += u64::from(gc_performed);
                    let gc_ms = gc.elapsed().as_millis();
                    let after = Measurement::read(&self.engine);
                    self.completed += 1;
                    log(&format!(
                        "completed request={} async_elapsed_ms={} gc_ms={} before=[{}] after=[{}] gc_performed={}",
                        self.requests,
                        async_elapsed_ms,
                        gc_ms,
                        self.before
                            .as_ref()
                            .map(Measurement::description)
                            .unwrap_or_default(),
                        after.description(), gc_performed
                    ));
                    self.before = None;
                    self.pending_since = None;
                    self.policy.completed(self.start.elapsed());
                }
                Err(reason) => {
                    // Retain any outstanding root until UnityStopping. Do not
                    // keep retrying a broken API, or claim the operation ended.
                    log(&format!("collector disabled: {reason}; pending operation root retained until player stop"));
                    self.disabled = true;
                }
            }
            return;
        }
        let mut sample = match self.source.sample() {
            Ok(sample) => {
                self.sample_error = false;
                sample
            }
            Err(error) => {
                if !self.sample_error {
                    log(&format!(
                        "memory sample unavailable: {error}; no fabricated pressure"
                    ));
                }
                self.sample_error = true;
                return;
            }
        };
        // At most notify + immediate fallback in one maintenance. A successful
        // notification normally yields to gameplay before a fresh pressure read.
        for _ in 0..2 {
            if STOPPED.load(Ordering::Acquire) {
                return;
            }
            let action = self.policy.next(sample.available_kib, self.start.elapsed());
            if matches!(action, Action::Notify | Action::Unload) && self.before.is_none() {
                // Swap/PSI are evidence, not allocation headroom. Read them
                // only for a real pressure action, never for every idle tick.
                self.source.fill_diagnostics(&mut sample);
                let before = Measurement::read(&self.engine);
                let psi = sample.psi.map(|v| {
                    format!(
                        "scope={} some_avg10={:?} full_avg10={:?}",
                        v.scope, v.some_avg10, v.full_avg10
                    )
                });
                log(&format!("pressure: available_kib={} host_available_kib={} cgroup_headroom_kib={:?} host_swap_total_kib={:?} host_swap_free_kib={:?} cgroup_swap_headroom_kib={:?} cgroup_memsw_headroom_kib={:?} psi={:?} before=[{}]",
                    sample.available_kib, sample.host_available_kib, sample.cgroup_headroom_kib,
                    sample.host_swap_total_kib, sample.host_swap_free_kib, sample.cgroup_swap_headroom_kib,
                    sample.cgroup_memsw_headroom_kib, psi, before.description()));
                self.before = Some(before);
            }
            if STOPPED.load(Ordering::Acquire) {
                return;
            }
            match action {
                Action::Wait => return,
                Action::Notify => {
                    let succeeded = match self.engine.notify() {
                        Ok(delivered) => {
                            self.notifications += u64::from(delivered);
                            log(&format!("notified low-memory notification={} grace_ms={}; rendering continues before pressure recheck", self.notifications, self.config.notification_grace.as_millis()));
                            delivered
                        }
                        Err(reason) => {
                            log(&format!("low-memory notification failed: {reason}; using available engine cleanup"));
                            false
                        }
                    };
                    self.policy.notified(self.start.elapsed(), succeeded);
                }
                Action::Recovered | Action::NotificationOnly => {
                    if action == Action::Recovered {
                        self.recovered += 1;
                    }
                    let after = Measurement::read(&self.engine);
                    log(&format!("notification outcome={:?} available_kib={} extra_unload=false before=[{}] after=[{}]", action, sample.available_kib,
                        self.before.as_ref().map(Measurement::description).unwrap_or_default(), after.description()));
                    self.before = None;
                    return;
                }
                Action::Unload => {
                    let request = Instant::now();
                    match self.engine.request() {
                        Ok(()) => {
                            self.requests += 1;
                            self.pending_since = Some(request);
                            self.pending_warning = false;
                            log(&format!("requested UnloadUnusedAssets request={} request_ms={}; operation rooted, rendering continues",
                                self.requests, request.elapsed().as_millis()));
                        }
                        Err(reason) => {
                            self.disabled = true;
                            log(&format!("collector disabled: {reason}"));
                        }
                    }
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/config.rs"]
mod config_tests;

enum Slot {
    New,
    Active(Controller),
    Disabled,
    Stopped,
}
thread_local! {
    static SLOT: RefCell<Slot> = const { RefCell::new(Slot::New) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.with(|busy| busy.set(false));
    }
}

fn stop_controller(slot: Slot) -> Slot {
    if let Slot::Active(controller) = &slot {
        log(&format!(
            "player stop: requests={} completed={} pending={} notifications={} recovered={} gc_runs={} notification_waiting={}",
            controller.requests, controller.completed, controller.engine.pending(),
            controller.notifications, controller.recovered, controller.gc_runs,
            controller.policy.awaiting_developer()
        ));
    }
    // The host waits for in-flight player callbacks to return before nativeDone.
    // Release any root on its owning thread, before that teardown can begin.
    drop(slot);
    Slot::Stopped
}

/// No Rust owner pointers cross the ABI. Only the selected player's thread
/// may create/use the controller. Drop its managed root before nativeDone.
extern "C" fn lifecycle(event: Option<&RudroidModuleEvent>, _user: *mut c_void) {
    let Some(event) = event else {
        return;
    };
    let Some(host) = HOST.get() else {
        return;
    };
    match ModuleKind::from_u32(event.kind) {
        // SAFETY: an Il2cpp module event reports the load base of the mapped,
        // never-unmapped guest libil2cpp; `so_symbol` is the loader's resolver.
        Some(ModuleKind::Il2cpp) => {
            match unsafe { Il2cpp::bind(event.base, host.so_symbol, engine::REQUIRED_EXPORTS) } {
                Ok(api) => *API.lock().unwrap_or_else(|p| p.into_inner()) = Some(api),
                Err(missing) => log(&format!(
                    "collector unavailable: missing IL2CPP exports: {}",
                    missing.join(",")
                )),
            }
        }
        Some(ModuleKind::Il2cppReady) => READY.store(true, Ordering::Release),
        Some(ModuleKind::UnityMaintenance | ModuleKind::UnityStopping) => {
            let current = std::thread::current().id();
            {
                let mut owner = OWNER.lock().unwrap_or_else(|p| p.into_inner());
                if owner.is_some_and(|id| id != current) {
                    return;
                }
                if owner.is_none() {
                    *owner = Some(current);
                }
            }
            if event.kind == ModuleKind::UnityStopping as u32 {
                // Stop is never discarded by the recursion guard. An in-flight
                // callback sees cancellation after its current guest call.
                STOPPED.store(true, Ordering::Release);
            }
            BUSY.with(|busy| {
                if busy.replace(true) {
                    return;
                }
                let _busy = BusyGuard;
                // Release the RefCell borrow BEFORE guest calls/finalizers.
                let mut slot = SLOT.with(|slot| slot.replace(Slot::Disabled));
                if STOPPED.load(Ordering::Acquire) {
                    slot = stop_controller(slot);
                } else {
                    if matches!(slot, Slot::New) && READY.load(Ordering::Acquire) {
                        let api = *API.lock().unwrap_or_else(|p| p.into_inner());
                        slot = match api {
                            Some(api) => match Controller::new(
                                api,
                                *CONFIG.get().expect("config set before registration"),
                            ) {
                                Ok(controller) => Slot::Active(controller),
                                Err(reason) => {
                                    log(&format!("collector unavailable: {reason}"));
                                    Slot::Disabled
                                }
                            },
                            None => {
                                log("collector unavailable: IL2CPP binding missing");
                                Slot::Disabled
                            }
                        };
                    }
                    if let Slot::Active(controller) = &mut slot {
                        controller.tick();
                    }
                }
                if STOPPED.load(Ordering::Acquire) && !matches!(slot, Slot::Stopped) {
                    slot = stop_controller(slot);
                }
                SLOT.with(|cell| {
                    cell.replace(slot);
                });
            });
        }
        _ => {}
    }
}

/// Same-build Host API, process-pinned callbacks; absent plugin costs no
/// memory sampling and the profile broadcasts at most once per second.
#[no_mangle]
/// # Safety
/// The host table and callbacks remain mapped for the process; guest symbol
/// lookup and module events obey the same-kit native ABI and quiescent hook
/// installation contract.
pub unsafe extern "C" fn rudroid_plugin_init(host: Option<&'static HostApiV0>) -> c_int {
    let Some(host) = host else {
        return -1;
    };
    if host.abi_version != HOST_API_VERSION {
        return -2;
    }
    if !host.config_bool(c"unity_memory.enabled", true) {
        return 0;
    }
    if !cfg!(target_os = "linux") {
        return 0;
    }
    let config = match Config::read(host) {
        Ok(config) => config,
        Err(reason) => {
            host.log_line(c"UNITY_MEMORY", reason);
            return -3;
        }
    };
    if HOST.set(*host).is_err() || CONFIG.set(config).is_err() {
        return -4;
    }
    for kind in [
        ModuleKind::Il2cpp,
        ModuleKind::Il2cppReady,
        ModuleKind::UnityMaintenance,
        ModuleKind::UnityStopping,
    ] {
        if !host.on_module_loaded(kind, lifecycle, std::ptr::null_mut()) {
            return -5;
        }
    }
    log("armed: pressure-driven player-thread collector; awaiting IL2CPP metadata");
    0
}
