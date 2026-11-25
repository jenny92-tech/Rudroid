//! INPUT: verified inline render-sample boundaries and an optional thread name
//! OUTPUT: bounded `/proc` counter pairs, identity and monotonic read brackets
//! POS: read-only Linux diagnostics, with no scheduling changes or worker
#![forbid(unsafe_code)]

use rudroid_common::clock::monotonic_nanos;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

const PEER: &str = "RUDROID_DIAGNOSTIC_FRAME_SCHED_THREAD";
const SEARCH_ATTEMPTS: u8 = 8;
const SEARCH_TASKS: usize = 512;

#[derive(Clone, Copy, Default)]
pub(super) struct Read {
    tid: u32,
    handle: u8,
    start_ticks: u64,
    before: i64,
    after: i64,
    counters: Option<[u64; 3]>,
    status: &'static str,
}

impl Read {
    fn missing(status: &'static str) -> Self { Self { status, ..Self::default() } }

    fn delta(self, end: Self) -> (Option<[u64; 3]>, &'static str) {
        let (Some(a), Some(b)) = (self.counters, end.counters) else { return (None, "missing"); };
        if self.tid != end.tid || self.handle != end.handle || self.start_ticks != end.start_ticks { return (None, "identity"); }
        let Some(delta) = subtract(a, b) else { return (None, "counter-regressed"); };
        if self.before <= 0 || self.after < self.before || end.before < self.after || end.after < end.before {
            return (None, "clock");
        }
        // All-zero lifetime run-delay fields do not establish accounting.
        // Keep the raw values and executable/slice deltas for the analyst.
        (Some(delta), if b[1] == 0 { "run-delay-unobserved" } else { "ok" })
    }
}

fn subtract(start: [u64; 3], end: [u64; 3]) -> Option<[u64; 3]> {
    Some([end[0].checked_sub(start[0])?, end[1].checked_sub(start[1])?, end[2].checked_sub(start[2])?])
}

fn counters(bytes: &[u8]) -> Option<[u64; 3]> {
    if !bytes.ends_with(b"\n") { return None; }
    let mut parts = std::str::from_utf8(bytes).ok()?.split_ascii_whitespace();
    let values = [parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?];
    parts.next().is_none().then_some(values)
}

struct Task { tid: u32, handle: u8, start_ticks: u64, fd: File }

impl Task {
    fn open(tid: u32, handle: u8, path: &Path) -> Option<Self> {
        let identity = || -> Option<u64> {
            fs::read_to_string(path.join("stat")).ok()?.rsplit_once(')')?.1
                .split_ascii_whitespace().nth(19)?.parse().ok()
        };
        let start_ticks = identity()?;
        let fd = File::open(path.join("schedstat")).ok()?;
        (identity()? == start_ticks).then_some(Self { tid, handle, start_ticks, fd })
    }

    fn sample(&self) -> Read {
        let mut bytes = [0; 96];
        let before = monotonic_nanos();
        let result = self.fd.read_at(&mut bytes, 0);
        let after = monotonic_nanos();
        let (counters, status) = match result {
            Ok(n) => match counters(&bytes[..n]) {
                Some(values) => (Some(values), "ok"),
                None => (None, "malformed"),
            },
            Err(_) => (None, "unreadable"),
        };
        Read { tid: self.tid, handle: self.handle, start_ticks: self.start_ticks, before, after, counters, status }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Boundary {
    wall_start: i64,
    wall_end: i64,
    cpu_ns: i64,
    reads: [Read; 2],
}

struct Row {
    iteration: u64,
    render_start: i64,
    render_end: i64,
    start: Boundary,
    end: Boundary,
}

pub(super) struct Probe {
    path: PathBuf,
    main: Option<Task>,
    peer: Option<Task>,
    peer_name: String,
    attempts: u8,
    peer_status: &'static str,
    control: &'static str,
    rows: Vec<Row>,
}

impl Probe {
    pub(super) fn configured(phase_path: &Path, capacity: usize) -> Option<Self> {
        let value = std::env::var_os(PEER)?;
        let peer_name = value.to_str().filter(|name| !name.is_empty() && name.len() <= 15
            && name.bytes().all(|b| b == b' ' || b.is_ascii_graphic() && b != b','));
        let Some(peer_name) = peer_name else {
            rudroid_common::diagnostic!("[UNITY-RENDER-SCHED] {PEER} needs one printable Linux comm (1–15 bytes)");
            return None;
        };
        let main_path = Path::new("/proc/thread-self");
        let main = fs::read_link(main_path).ok().and_then(|path| {
            Task::open(path.file_name()?.to_str()?.parse().ok()?, 0, main_path)
        });
        let control = match fs::read_to_string("/proc/sys/kernel/sched_schedstats").ok().as_deref().map(str::trim) {
            Some("0") => "0",
            Some("1") => "1",
            _ => "unknown",
        };
        Some(Self { path: phase_path.with_extension("sched.csv"), main, peer: None,
            peer_name: peer_name.to_owned(), attempts: 0, peer_status: "not-seen", control,
            rows: Vec::with_capacity(capacity) })
    }

    fn discover(&self) -> Result<Task, &'static str> {
        let mut matched = None;
        let entries = fs::read_dir("/proc/self/task").map_err(|_| "task-dir-unreadable")?;
        for (index, entry) in entries.enumerate() {
            if index == SEARCH_TASKS { return Err("search-limit"); }
            let Ok(entry) = entry else { return Err("task-dir-unreadable"); };
            let Some(tid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue; };
            if self.main.as_ref().is_some_and(|task| task.tid == tid) { continue; }
            if fs::read_to_string(entry.path().join("comm")).ok().as_deref().and_then(|name| name.strip_suffix('\n')) != Some(&*self.peer_name) {
                continue;
            }
            if matched.is_some() { return Err("ambiguous"); }
            matched = Some((tid, entry.path()));
        }
        let (tid, path) = matched.ok_or("not-seen")?;
        let task = Task::open(tid, self.attempts, &path).ok_or("schedstat-unavailable")?;
        (fs::read_to_string(path.join("comm")).ok().as_deref().and_then(|name| name.strip_suffix('\n')) == Some(&*self.peer_name))
            .then_some(task).ok_or("identity-changed")
    }

    pub(super) fn boundary(&mut self, start: bool) -> Boundary {
        let wall_start = monotonic_nanos();
        let cpu_start = cpu_now();
        if start && self.peer.is_none() && self.attempts < SEARCH_ATTEMPTS {
            self.attempts += 1;
            match self.discover() {
                Ok(task) => self.peer = Some(task),
                Err(status) => self.peer_status = status,
            }
        }
        let main = self.main.as_ref().map_or(Read::missing("schedstat-unavailable"), Task::sample);
        let peer = self.peer.as_ref().map_or(Read::missing(self.peer_status), Task::sample);
        if self.peer.is_some() && peer.counters.is_none() {
            // A dead proc fd stays bound to the old identity. Rediscover only
            // before a later render, never complete a pair with another fd.
            self.peer = None;
            self.peer_status = "lost";
        }
        let cpu_ns = cpu_now().checked_sub(cpu_start).filter(|delta| cpu_start > 0 && *delta >= 0).unwrap_or(-1);
        Boundary { wall_start, wall_end: monotonic_nanos(), cpu_ns, reads: [main, peer] }
    }

    pub(super) fn append(&mut self, iteration: u64, render_start: i64, render_end: i64, start: Boundary, end: Boundary) {
        self.rows.push(Row { iteration, render_start, render_end, start, end });
    }

    pub(super) fn write(&self) -> io::Result<()> {
        let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&self.path)?);
        writeln!(out, "# unity-render-sched-v1 pid={} samples={} discoveries={} sched_schedstats={} peer={}",
            std::process::id(), self.rows.len(), self.attempts, self.control, self.peer_name)?;
        writeln!(out, "iteration,role,start_tid,start_handle,start_ticks,end_tid,end_handle,end_ticks,render_start_ns,render_end_ns,observer_start_ns,observer_start_end_ns,observer_end_start_ns,observer_end_ns,observer_start_cpu_ns,observer_end_cpu_ns,read_start_before_ns,read_start_after_ns,read_end_before_ns,read_end_after_ns,start_status,end_status,pair_status,runtime_delta_ns,run_delay_delta_ns,slices_delta")?;
        for row in &self.rows {
            for (index, role) in ["main", "peer"].into_iter().enumerate() {
                let a = row.start.reads[index];
                let b = row.end.reads[index];
                let (delta, status) = a.delta(b);
                write!(out, "{},{role},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{status},", row.iteration,
                    a.tid, a.handle, a.start_ticks, b.tid, b.handle, b.start_ticks, row.render_start, row.render_end,
                    row.start.wall_start, row.start.wall_end, row.end.wall_start, row.end.wall_end,
                    row.start.cpu_ns, row.end.cpu_ns,
                    a.before, a.after, b.before, b.after, a.status, b.status)?;
                if let Some([runtime, delay, slices]) = delta { writeln!(out, "{runtime},{delay},{slices}")?; }
                else { writeln!(out, ",,")?; }
            }
        }
        out.flush()
    }
}

fn cpu_now() -> i64 {
    #[cfg(target_os = "linux")]
    { rudroid_common::clock::thread_cpu_nanos() }
    #[cfg(not(target_os = "linux"))]
    { 0 }
}

#[cfg(test)]
#[path = "../tests/unit/render_schedstat.rs"]
mod tests;
