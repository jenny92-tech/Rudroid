//! INPUT: explicit private diagnostic mode and native-Wayland primary swaps
//! OUTPUT: bounded swap-wall/CPU and interval lifecycle records, written at stopping
//! POS: samples around SDL without additional GL commands or timed file writes
#![forbid(unsafe_code)]

use rudroid_common::{
    clock::{monotonic_nanos, thread_cpu_nanos},
    host_api::HostApiV0,
    registry::RudroidModuleEvent,
    ModuleKind,
};
use std::{
    collections::VecDeque,
    ffi::c_void,
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const SWAP_LIMIT: usize = 8192;
const INTERVAL_LIMIT: usize = 64;
const MODE: &str = "RUDROID_DIAG_WAYLAND_INTERVAL";
const OUTPUT: &str = "RUDROID_DIAG_WAYLAND_PROFILE";

#[derive(Clone, Copy)]
enum Route {
    Raw,
    Sdl,
}

struct Settings {
    route: Route,
    output: PathBuf,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

pub(super) fn register(api: &HostApiV0) -> Result<(), String> {
    if !rudroid_common::logging::diagnostics_enabled() { return Ok(()); }
    let Some(route) = std::env::var_os(MODE) else {
        return Ok(());
    };
    let route = match route.to_str() {
        Some("raw") => Route::Raw,
        Some("sdl") => Route::Sdl,
        _ => return Err(format!("{MODE} must be raw or sdl")),
    };
    let output = std::env::var_os(OUTPUT)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| format!("{OUTPUT} must be an absolute output file"))?;
    if !api.on_module_loaded(ModuleKind::UnityStopping, report, std::ptr::null_mut()) {
        return Err("could not subscribe Wayland interval diagnostics to UnityStopping".into());
    }
    SETTINGS
        .set(Settings { route, output })
        .map_err(|_| "Wayland interval diagnostics already registered".into())
}

extern "C" fn report(_: Option<&RudroidModuleEvent>, _: *mut c_void) {
    if let Err(error) = super::guest_graphics::report_wayland_interval() {
        rudroid_common::error!("[WAYLAND-INTERVAL] report failed: {error}");
    }
}

pub(super) struct Trace {
    settings: &'static Settings,
    samples: Mutex<Option<Samples>>,
}

struct Samples {
    swaps: VecDeque<Swap>,
    intervals: Vec<Interval>,
    overwritten: u64,
    intervals_total: u64,
}

struct Swap {
    wall_start_ns: u128,
    wall_ns: i64,
    cpu_ns: i64,
}

struct Interval {
    wall_start_ns: u128,
    requested: i32,
    before: i32,
    raw_ok: bool,
    sdl_status: Option<i32>,
    after: i32,
}

fn epoch_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

impl Trace {
    pub(super) fn for_native_wayland(native_wayland: bool) -> Option<Self> {
        let settings = SETTINGS.get().filter(|_| native_wayland)?;
        Some(Self::new(settings))
    }

    fn new(settings: &'static Settings) -> Self {
        Self {
            settings,
            samples: Mutex::new(Some(Samples {
                swaps: VecDeque::with_capacity(SWAP_LIMIT),
                intervals: Vec::with_capacity(INTERVAL_LIMIT),
                overwritten: 0,
                intervals_total: 0,
            })),
        }
    }

    pub(super) fn routes_to_sdl(&self) -> bool {
        matches!(self.settings.route, Route::Sdl)
    }

    pub(super) fn measure_swap(&self, call: impl FnOnce()) {
        let wall_start_ns = epoch_ns();
        let start_cpu = thread_cpu_nanos();
        let start_wall = monotonic_nanos();
        call();
        let wall_ns = monotonic_nanos().saturating_sub(start_wall);
        let cpu_ns = thread_cpu_nanos().saturating_sub(start_cpu);
        let mut samples = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(samples) = &mut *samples {
            if samples.swaps.len() == SWAP_LIMIT {
                samples.swaps.pop_front();
                samples.overwritten += 1;
            }
            samples.swaps.push_back(Swap {
                wall_start_ns,
                wall_ns,
                cpu_ns,
            });
        }
    }

    pub(super) fn interval(
        &self,
        requested: i32,
        before: i32,
        raw_ok: bool,
        sdl_status: Option<i32>,
        after: i32,
    ) {
        let mut samples = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(samples) = &mut *samples {
            samples.intervals_total += 1;
            if samples.intervals.len() < INTERVAL_LIMIT {
                samples.intervals.push(Interval {
                    wall_start_ns: epoch_ns(),
                    requested,
                    before,
                    raw_ok,
                    sdl_status,
                    after,
                });
            }
        }
    }

    pub(super) fn report(&self) -> io::Result<()> {
        let samples = self
            .samples
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some(samples) = samples else {
            return Ok(());
        };
        let path = &self.settings.output;
        let mut output =
            BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
        writeln!(
            output,
            "mode,{}",
            if self.routes_to_sdl() { "sdl" } else { "raw" }
        )?;
        writeln!(
            output,
            "swap_samples,{},overwritten,{},interval_requests,{}",
            samples.swaps.len(),
            samples.overwritten,
            samples.intervals_total
        )?;
        writeln!(
            output,
            "interval,epoch_ns,requested,sdl_before,raw_ok,sdl_status,sdl_after"
        )?;
        for item in samples.intervals {
            let status = item
                .sdl_status
                .map(|value| value.to_string())
                .unwrap_or_default();
            writeln!(
                output,
                "interval,{},{},{},{},{},{}",
                item.wall_start_ns,
                item.requested,
                item.before,
                u8::from(item.raw_ok),
                status,
                item.after
            )?;
        }
        writeln!(output, "swap,epoch_ns,wall_ns,thread_cpu_ns")?;
        for item in samples.swaps {
            writeln!(
                output,
                "swap,{},{},{}",
                item.wall_start_ns, item.wall_ns, item.cpu_ns
            )?;
        }
        output.flush()
    }
}

#[cfg(test)]
#[path = "../tests/unit/wayland_interval.rs"]
mod tests;
