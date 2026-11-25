//! INPUT: Unity's exact frame-rate icall registrations and the player frame event
//! OUTPUT: opt-in desktop engine target and diagnostic requested/applied snapshots
//! POS: writes Unity's target from the player thread, after nativeRender unwinds
//!
//! Desktop policy and the registration seam follow PortMaster-Unity
//! runtime/host/framerate.c. See THIRD-PARTY-NOTICES.

use crate::{library_symbol, Library};
use rudroid_common::{
    host_api::{HostApiV0, HOST_API_VERSION},
    registry::{HostStr, RudroidModuleEvent},
    ModuleKind,
};
use rudroid_unity_common::mono_abi::{
    MonoStartup, FRAME_PACING_ANDROID, FRAME_PACING_DESKTOP, FRAME_PACING_OBSERVE,
    MAX_DISPLAY_REFRESH_HZ, MIN_DISPLAY_REFRESH_HZ,
};
use std::{
    ffi::{c_int, c_void},
    sync::{
        atomic::{
            AtomicBool, AtomicI64, AtomicUsize,
            Ordering::{Acquire, Relaxed, Release},
        },
        OnceLock,
    },
};

type Register = extern "C" fn(HostStr<'_>, *const c_void);
type Setter = extern "C" fn(c_int);
type Getter = extern "C" fn() -> c_int;

const UNSET: i64 = i64::MIN;
static PACING: OnceLock<Pacing> = OnceLock::new();

#[derive(Clone, Copy)]
enum Mode {
    Observe,
    Desktop(c_int),
}

struct Pacing {
    mode: Mode,
    register: Register,
    setter: OnceLock<Setter>,
    getter: OnceLock<Getter>,
    vsync: OnceLock<Getter>,
    requested: AtomicI64,
    selected: AtomicI64,
    engine_readback: AtomicI64,
    last_vsync: AtomicI64,
    managed_readback: AtomicI64,
    managed_sets: AtomicUsize,
    managed_gets: AtomicUsize,
    sampled: AtomicBool,
    stopped: AtomicBool,
}

impl Pacing {
    fn new(mode: Mode, register: Register) -> Self {
        Self {
            mode,
            register,
            setter: OnceLock::new(),
            getter: OnceLock::new(),
            vsync: OnceLock::new(),
            requested: AtomicI64::new(UNSET),
            selected: AtomicI64::new(UNSET),
            engine_readback: AtomicI64::new(UNSET),
            last_vsync: AtomicI64::new(UNSET),
            managed_readback: AtomicI64::new(UNSET),
            managed_sets: AtomicUsize::new(0),
            managed_gets: AtomicUsize::new(0),
            sampled: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }
    }

    fn set_requested(&self, value: c_int) {
        self.requested.store(i64::from(value), Relaxed);
        self.managed_sets.fetch_add(1, Relaxed);
        if matches!(self.mode, Mode::Observe) || !self.sampled.load(Acquire) {
            if let Some(setter) = self.setter.get() {
                setter(value);
            }
        }
    }

    fn get_requested(&self) -> c_int {
        let value = match self.mode {
            Mode::Desktop(_) if self.sampled.load(Acquire) => {
                snapshot(&self.requested)
                    .unwrap_or_else(|| self.getter.get().map_or(-1, |getter| getter()))
            }
            _ => self.getter.get().map_or(-1, |getter| getter()),
        };
        self.managed_readback.store(i64::from(value), Relaxed);
        self.managed_gets.fetch_add(1, Relaxed);
        value
    }

    fn frame(&self) {
        if self.stopped.load(Acquire) {
            return;
        }
        let (Some(set), Some(get), Some(get_vsync)) =
            (self.setter.get(), self.getter.get(), self.vsync.get())
        else {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Relaxed) {
                rudroid_common::diagnostic!(
                    "[MONO-PACING] waiting for Unity icalls: setter={} getter={} vsync={}",
                    self.setter.get().is_some(),
                    self.getter.get().is_some(),
                    self.vsync.get().is_some(),
                );
            }
            return;
        };
        let before = get();
        let _ = self
            .requested
            .compare_exchange(UNSET, i64::from(before), Relaxed, Relaxed);
        let requested = snapshot(&self.requested).unwrap_or(before);
        let vsync = get_vsync();
        let selected = match self.mode {
            Mode::Observe => before,
            Mode::Desktop(hz) => effective(hz, requested, vsync),
        };
        let after = if selected != before {
            set(selected);
            get()
        } else {
            before
        };
        self.selected.store(i64::from(selected), Relaxed);
        self.engine_readback.store(i64::from(after), Relaxed);
        self.last_vsync.store(i64::from(vsync), Relaxed);
        if !self.sampled.swap(true, Release) {
            self.log("first_frame");
        }
    }

    fn log(&self, phase: &str) {
        let mode = if matches!(self.mode, Mode::Observe) {
            "observe"
        } else {
            "desktop"
        };
        rudroid_common::diagnostic!(
            "[MONO-PACING] phase={phase} mode={mode} requested={:?} vsync={:?} selected={:?} engine_readback={:?} managed_sets={} managed_gets={} managed_readback={:?}",
            snapshot(&self.requested),
            snapshot(&self.last_vsync),
            snapshot(&self.selected),
            snapshot(&self.engine_readback),
            self.managed_sets.load(Relaxed),
            self.managed_gets.load(Relaxed),
            snapshot(&self.managed_readback),
        );
    }
}

fn snapshot(value: &AtomicI64) -> Option<c_int> {
    c_int::try_from(value.load(Relaxed)).ok()
}

fn effective(hz: c_int, requested: c_int, vsync: c_int) -> c_int {
    if vsync > 0 {
        return ((hz + vsync / 2) / vsync).max(1);
    }
    if requested > 0 {
        requested
    } else {
        hz
    }
}

pub(super) fn install(
    config: &MonoStartup<'_>,
    bridge: &Library,
    host: Option<&HostApiV0>,
) -> Result<(), String> {
    let mode = match config.frame_pacing {
        FRAME_PACING_ANDROID => return Ok(()),
        FRAME_PACING_OBSERVE => Mode::Observe,
        FRAME_PACING_DESKTOP
            if (MIN_DISPLAY_REFRESH_HZ..=MAX_DISPLAY_REFRESH_HZ)
                .contains(&config.display_refresh_hz) =>
        {
            Mode::Desktop(config.display_refresh_hz as c_int)
        }
        _ => return Err("invalid Mono frame pacing mode or refresh rate".into()),
    };
    let host = host
        .filter(|host| host.abi_version == HOST_API_VERSION)
        .ok_or("Mono frame pacing requires the same-kit Host API")?;
    let register = library_symbol(bridge, c"il2cpp_add_internal_call")
        .ok_or("Mono frame pacing requires the bridge's il2cpp_add_internal_call")?;
    // SAFETY: the trusted pinned PortMaster-Unity bridge documents this
    // registration ABI. The callback address is opaque to the registrar.
    let register: Register = unsafe { std::mem::transmute(register) };
    PACING
        .set(Pacing::new(mode, register))
        .map_err(|_| "Mono pacing already installed")?;
    for kind in [ModuleKind::UnityFrame, ModuleKind::UnityStopping] {
        if !host.on_module_loaded(kind, lifecycle, std::ptr::null_mut()) {
            return Err(format!("Mono frame pacing could not subscribe to {kind:?}"));
        }
    }
    Ok(())
}

extern "C" fn lifecycle(event: Option<&RudroidModuleEvent>, _: *mut c_void) {
    let Some((event, pacing)) = event.zip(PACING.get()) else {
        return;
    };
    if event.kind == ModuleKind::UnityFrame as u32 {
        pacing.frame();
    } else if event.kind == ModuleKind::UnityStopping as u32 {
        // The creator may announce stop after a separate player thread joins.
        // Log cached readback here and do not call the engine from that thread.
        pacing.stopped.store(true, Release);
        pacing.log("stopping");
    }
}

pub(super) fn registration_wrapper() -> Option<usize> {
    PACING.get().map(|_| register as *const () as usize)
}

extern "C" fn register(name: HostStr<'_>, address: *const c_void) {
    let Some(pacing) = PACING.get() else { return };
    let mut forwarded = address;
    if !address.is_null() {
        match name.to_cstr().map(|name| name.to_bytes()) {
            Some(
                b"UnityEngine.Application::set_targetFrameRate"
                | b"UnityEngine.Application::set_targetFrameRate(System.Int32)",
            ) => {
                // SAFETY: this exact scalar Unity icall is void(int32_t).
                let original: Setter = unsafe { std::mem::transmute(address) };
                if *pacing.setter.get_or_init(|| original) as usize == address as usize {
                    forwarded = set_target as *const c_void;
                }
            }
            Some(
                b"UnityEngine.Application::get_targetFrameRate"
                | b"UnityEngine.Application::get_targetFrameRate()",
            ) => {
                // SAFETY: this exact scalar Unity icall is int32_t(void).
                let original: Getter = unsafe { std::mem::transmute(address) };
                if *pacing.getter.get_or_init(|| original) as usize == address as usize {
                    forwarded = get_target as *const c_void;
                }
            }
            Some(
                b"UnityEngine.QualitySettings::get_vSyncCount"
                | b"UnityEngine.QualitySettings::get_vSyncCount()",
            ) => {
                // SAFETY: this exact scalar Unity icall is int32_t(void).
                let original: Getter = unsafe { std::mem::transmute(address) };
                let _ = pacing.vsync.set(original);
            }
            _ => {}
        }
    }
    (pacing.register)(name, forwarded);
}

extern "C" fn set_target(value: c_int) {
    if let Some(pacing) = PACING.get() {
        pacing.set_requested(value);
    }
}

extern "C" fn get_target() -> c_int {
    PACING.get().map_or(-1, Pacing::get_requested)
}

#[cfg(test)]
#[path = "../tests/unit/pacing.rs"]
mod tests;
