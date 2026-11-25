//! INPUT: private phase trace path and the inline UnityMain loop
//! OUTPUT: bounded wall/thread-CPU samples with monotonic clock alignment
//! POS: one owner, clocks only on sampled frames, output on loop exit
#![forbid(unsafe_code)]

use rudroid_common::{clock::monotonic_nanos, logging::diagnostics_enabled};
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use super::render_schedstat::{Boundary, Probe};

const OUTPUT: &str = "RUDROID_DIAGNOSTIC_FRAME_PHASES";
const STRIDE: u64 = 64;
const MAX_SAMPLES: usize = 1024;

#[derive(Clone, Copy)]
struct Clock {
    mono: i64,
    cpu: i64,
}

impl Clock {
    fn now() -> Self {
        #[cfg(target_os = "linux")]
        let cpu = rudroid_common::clock::thread_cpu_nanos();
        #[cfg(not(target_os = "linux"))]
        let cpu = 0;
        Self { mono: monotonic_nanos(), cpu }
    }

    fn since(self, start: Self) -> Option<(i64, i64)> {
        if start.mono <= 0 || start.cpu <= 0 || self.mono < start.mono || self.cpu < start.cpu {
            return None;
        }
        Some((self.mono - start.mono, self.cpu - start.cpu))
    }
}

struct Anchor {
    before: i64,
    unix: u128,
    after: i64,
}

impl Anchor {
    fn now() -> Self {
        let before = monotonic_nanos();
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
        Self { before, unix, after: monotonic_nanos() }
    }

    fn write(&self, label: &str, out: &mut impl Write) -> io::Result<()> {
        writeln!(out, "# anchor,{label},{},{},{}", self.before, self.unix, self.after)
    }
}

pub(super) struct Sample {
    iteration: u64,
    start: Clock,
    input_end: Clock,
    render_end: Clock,
    scheduler_start: Option<Boundary>,
    scheduler_end: Option<Boundary>,
}

struct Row {
    iteration: u64,
    start: i64,
    input: (i64, i64),
    render: (i64, i64),
    post: (i64, i64),
    sleep_request: u128,
    sleep_wall: i64,
}

impl Row {
    fn complete(sample: Sample, end: Clock, sleep: Duration, sleep_start: Option<i64>) -> Option<Self> {
        let input = sample.input_end.since(sample.start)?;
        let render = sample.render_end.since(sample.input_end)?;
        let post = end.since(sample.render_end)?;
        let sleep_wall = match sleep_start {
            Some(start) if start >= sample.render_end.mono && start <= end.mono => end.mono - start,
            None if sleep.is_zero() => 0,
            _ => return None,
        };
        Some(Self {
            iteration: sample.iteration, start: sample.start.mono,
            input, render, post, sleep_request: sleep.as_nanos(), sleep_wall,
        })
    }

    fn write(&self, out: &mut impl Write) -> io::Result<()> {
        writeln!(out, "{},{},{},{},{},{},{},{},{},{}", self.iteration, self.start,
            self.input.0, self.input.1, self.render.0, self.render.1, self.post.0,
            self.post.1, self.sleep_request, self.sleep_wall)
    }
}

pub(super) struct Trace {
    path: PathBuf,
    anchor: Anchor,
    rows: Vec<Row>,
    dropped: u64,
    invalid: u64,
    scheduler: Option<Probe>,
}

impl Trace {
    pub(super) fn configured() -> Option<Self> {
        if !cfg!(target_os = "linux") || !diagnostics_enabled() {
            return None;
        }
        let path = PathBuf::from(std::env::var_os(OUTPUT)?);
        if !path.is_absolute() {
            rudroid_common::diagnostic!("[UNITY-MAIN-PHASES] {OUTPUT} requires an absolute output path");
            return None;
        }
        let scheduler = Probe::configured(&path, MAX_SAMPLES);
        Some(Self {
            path, anchor: Anchor::now(), rows: Vec::with_capacity(MAX_SAMPLES),
            dropped: 0, invalid: 0, scheduler,
        })
    }

    fn due(&mut self, iteration: u64) -> bool {
        if iteration == 0 || iteration % STRIDE != 0 {
            return false;
        }
        if self.rows.len() == MAX_SAMPLES {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        true
    }

    pub(super) fn begin(&mut self, iteration: u64) -> Option<Sample> {
        self.due(iteration).then(|| {
            let start = Clock::now();
            Sample { iteration, start, input_end: start, render_end: start, scheduler_start: None, scheduler_end: None }
        })
    }

    pub(super) fn before_render(&mut self, sample: &mut Sample) {
        sample.scheduler_start = self.scheduler.as_mut().map(|probe| probe.boundary(true));
        sample.input_end = Clock::now();
    }

    pub(super) fn after_render(&mut self, sample: &mut Sample) {
        sample.render_end = Clock::now();
        sample.scheduler_end = self.scheduler.as_mut().map(|probe| probe.boundary(false));
    }

    pub(super) fn finish(&mut self, sample: Sample, sleep: Duration, sleep_start: Option<i64>) {
        let sched = sample.scheduler_start.zip(sample.scheduler_end);
        let render_start = sample.input_end.mono;
        let render_end = sample.render_end.mono;
        if let Some(row) = Row::complete(sample, Clock::now(), sleep, sleep_start) {
            if let (Some(probe), Some((start, end))) = (&mut self.scheduler, sched) {
                probe.append(row.iteration, render_start, render_end, start, end);
            }
            self.rows.push(row);
        } else {
            self.invalid = self.invalid.saturating_add(1);
        }
    }

    fn write(&self, path: &Path, stop: Anchor) -> io::Result<()> {
        let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
        writeln!(out, "# unity-main-phases-v1 stride={STRIDE} cap={MAX_SAMPLES} samples={} dropped={} invalid={}",
            self.rows.len(), self.dropped, self.invalid)?;
        writeln!(out, "# anchor,name,mono_before_ns,unix_ns,mono_after_ns")?;
        self.anchor.write("start", &mut out)?;
        stop.write("stop", &mut out)?;
        writeln!(out, "iteration,mono_start_ns,input_wall_ns,input_cpu_ns,render_wall_ns,render_cpu_ns,post_wall_ns,post_cpu_ns,sleep_request_ns,sleep_wall_ns")?;
        for row in &self.rows {
            row.write(&mut out)?;
        }
        out.flush()
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        let stop = Anchor::now();
        if let Err(error) = self.write(&self.path, stop) {
            rudroid_common::diagnostic!("[UNITY-MAIN-PHASES] output failed: {error}");
        } else {
            rudroid_common::diagnostic!("[UNITY-MAIN-PHASES] samples={} dropped={} invalid={}",
                self.rows.len(), self.dropped, self.invalid);
        }
        if let Some(probe) = &self.scheduler {
            match probe.write() {
                Ok(()) => rudroid_common::diagnostic!("[UNITY-RENDER-SCHED] samples={}", self.rows.len()),
                Err(error) => rudroid_common::diagnostic!("[UNITY-RENDER-SCHED] output failed: {error}"),
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/phase_timing.rs"]
mod tests;
