//! INPUT:  Linux meminfo/cgroup/PSI/proc residency files, reserve/recovery/cooldown
//! OUTPUT:  MemorySource, MemorySample, Residency and PressureGate
//! POS:    keeps RAM, swap and stalls distinct; samples read-only kernel interfaces
#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct MemorySample {
    pub available_kib: u64,
    pub host_available_kib: u64,
    pub cgroup_headroom_kib: Option<u64>,
    pub host_swap_total_kib: Option<u64>,
    pub host_swap_free_kib: Option<u64>,
    pub cgroup_swap_headroom_kib: Option<u64>,
    pub cgroup_memsw_headroom_kib: Option<u64>,
    pub psi: Option<Psi>,
}

#[derive(Clone, Copy, Debug)]
pub struct Psi {
    pub scope: &'static str,
    pub some_avg10: Option<f64>,
    pub full_avg10: Option<f64>,
}

fn kib_field(text: &str, wanted: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == wanted).then_some(())?;
        let value = words.next()?.parse::<u64>().ok()?;
        (words.next()? == "kB" && words.next().is_none()).then_some(value)
    })
}

fn psi(text: &str, scope: &'static str) -> Option<Psi> {
    let avg10 = |kind| {
        text.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == kind).then_some(())?;
            let value = words
                .find_map(|word| word.strip_prefix("avg10="))?
                .parse::<f64>()
                .ok()?;
            (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
        })
    };
    let sample = Psi {
        scope,
        some_avg10: avg10("some"),
        full_avg10: avg10("full"),
    };
    (sample.some_avg10.is_some() || sample.full_avg10.is_some()).then_some(sample)
}

/// Fixed small files only; no /proc mappings dump or process scan.
fn read_small(path: &Path) -> io::Result<String> {
    let mut bytes = [0; 16384];
    let mut file = fs::File::open(path)?;
    let mut len = 0;
    while len < bytes.len() {
        let n = file.read(&mut bytes[len..])?;
        if n == 0 {
            break;
        }
        len += n;
    }
    if len == bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "memory file too large",
        ));
    }
    String::from_utf8(bytes[..len].to_vec()).map_err(|_| io::ErrorKind::InvalidData.into())
}

fn meminfo(text: &str) -> Option<u64> {
    // Older kernels: conservative free-only fallback, not fabricated zero.
    kib_field(text, "MemAvailable:").or_else(|| kib_field(text, "MemFree:"))
}

struct CgroupLimit {
    memory: (PathBuf, PathBuf),
    // v2: independent swap limit. v1: combined RAM+swap accounting.
    swap: Option<(PathBuf, PathBuf)>,
    memsw: Option<(PathBuf, PathBuf)>,
}

fn unescape_mount(path: &str) -> PathBuf {
    PathBuf::from(
        path.replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

/// Discover this process's v1/v2 memory hierarchy, including ancestor limits.
/// Resolve mount roots correctly (containers may mount a subtree at /).
fn cgroup_limits(mounts: &str, groups: &str) -> Vec<CgroupLimit> {
    let mut limits = Vec::new();
    for line in mounts.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = before.split_whitespace().collect();
        let tail: Vec<_> = after.split_whitespace().collect();
        if fields.len() < 5 || tail.len() < 3 {
            continue;
        }
        let v2 = tail[0] == "cgroup2";
        let v1 = tail[0] == "cgroup" && tail[2].split(',').any(|v| v == "memory");
        if !v1 && !v2 {
            continue;
        }
        let root = unescape_mount(fields[3]);
        let mount = unescape_mount(fields[4]);
        for group in groups.lines() {
            let parts: Vec<_> = group.splitn(3, ':').collect();
            if parts.len() != 3 {
                continue;
            }
            if !(v2 && parts[1].is_empty() || v1 && parts[1].split(',').any(|v| v == "memory")) {
                continue;
            }
            let group = Path::new(parts[2]);
            // Namespace-relative paths are rooted at the visible mount.
            let relative = group
                .strip_prefix(&root)
                .or_else(|_| group.strip_prefix("/"));
            let Ok(relative) = relative else {
                continue;
            };
            if relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            let mut current = mount.join(relative);
            loop {
                limits.push(CgroupLimit {
                    memory: (
                        current.join(if v2 {
                            "memory.max"
                        } else {
                            "memory.limit_in_bytes"
                        }),
                        current.join(if v2 {
                            "memory.current"
                        } else {
                            "memory.usage_in_bytes"
                        }),
                    ),
                    swap: v2.then(|| {
                        (
                            current.join("memory.swap.max"),
                            current.join("memory.swap.current"),
                        )
                    }),
                    memsw: (!v2).then(|| {
                        (
                            current.join("memory.memsw.limit_in_bytes"),
                            current.join("memory.memsw.usage_in_bytes"),
                        )
                    }),
                });
                if current == mount || !current.pop() || !current.starts_with(&mount) {
                    break;
                }
            }
        }
    }
    limits
}

pub struct MemorySource {
    limits: Vec<CgroupLimit>,
    cgroup_psi: Option<PathBuf>,
}

fn headroom(limit: &str, usage: &str) -> Option<u64> {
    let limit = limit.trim().parse::<u64>().ok()?; // "max" is not a finite limit.
    let usage = usage.trim().parse::<u64>().ok()?;
    Some(limit.saturating_sub(usage) / 1024)
}

fn read_headroom(paths: &(PathBuf, PathBuf)) -> Option<u64> {
    headroom(&read_small(&paths.0).ok()?, &read_small(&paths.1).ok()?)
}

fn minimum(values: impl Iterator<Item = u64>) -> Option<u64> {
    values.min()
}

fn allocation_headroom(host_ram: u64, cgroup_ram: Option<u64>, cgroup_memsw: Option<u64>) -> u64 {
    // v1 memsw constrains new allocations even when RAM alone has room:
    // swapping existing pages does not reduce the combined charge.
    [cgroup_ram, cgroup_memsw]
        .into_iter()
        .flatten()
        .fold(host_ram, u64::min)
}

impl MemorySource {
    pub fn discover() -> Self {
        // mountinfo can exceed the small memory-file cap; bounded at 1 MiB.
        let mut mounts = String::new();
        if let Ok(file) = fs::File::open("/proc/self/mountinfo") {
            let _ = file.take(1_048_576).read_to_string(&mut mounts);
        }
        let groups = read_small(Path::new("/proc/self/cgroup")).unwrap_or_default();
        let limits = cgroup_limits(&mounts, &groups);
        let cgroup_psi = limits
            .iter()
            .find(|level| level.swap.is_some())
            .map(|level| level.memory.0.with_file_name("memory.pressure"));
        Self { limits, cgroup_psi }
    }

    pub fn sample(&self) -> io::Result<MemorySample> {
        self.sample_with(read_small)
    }

    fn sample_with(
        &self,
        mut read: impl FnMut(&Path) -> io::Result<String>,
    ) -> io::Result<MemorySample> {
        let text = read(Path::new("/proc/meminfo"))?;
        let host_available_kib = meminfo(&text).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "meminfo has no available/free value",
            )
        })?;
        let mut room =
            |paths: &(PathBuf, PathBuf)| headroom(&read(&paths.0).ok()?, &read(&paths.1).ok()?);
        let cgroup_headroom_kib = minimum(self.limits.iter().filter_map(|v| room(&v.memory)));
        let cgroup_memsw_headroom_kib = minimum(
            self.limits
                .iter()
                .filter_map(|v| v.memsw.as_ref().and_then(&mut room)),
        );
        Ok(MemorySample {
            available_kib: allocation_headroom(
                host_available_kib,
                cgroup_headroom_kib,
                cgroup_memsw_headroom_kib,
            ),
            host_available_kib,
            cgroup_headroom_kib,
            host_swap_total_kib: kib_field(&text, "SwapTotal:"),
            host_swap_free_kib: kib_field(&text, "SwapFree:"),
            cgroup_swap_headroom_kib: None,
            cgroup_memsw_headroom_kib,
            psi: None,
        })
    }

    pub fn fill_diagnostics(&self, sample: &mut MemorySample) {
        sample.cgroup_swap_headroom_kib = minimum(
            self.limits
                .iter()
                .filter_map(|v| v.swap.as_ref().and_then(read_headroom)),
        );
        sample.psi = self
            .cgroup_psi
            .as_ref()
            .and_then(|path| read_small(path).ok())
            .and_then(|text| psi(&text, "cgroup"))
            .or_else(|| {
                read_small(Path::new("/proc/pressure/memory"))
                    .ok()
                    .and_then(|text| psi(&text, "host"))
            });
    }
}

#[derive(Debug)]
pub struct Residency {
    pub rss_kib: Option<u64>,
    pub pss_kib: Option<u64>,
    pub swap_kib: Option<u64>,
    pub swap_pss_kib: Option<u64>,
}

fn parse_residency(rollup: &str, status: &str) -> Residency {
    Residency {
        rss_kib: kib_field(rollup, "Rss:").or_else(|| kib_field(status, "VmRSS:")),
        pss_kib: kib_field(rollup, "Pss:"),
        swap_kib: kib_field(rollup, "Swap:").or_else(|| kib_field(status, "VmSwap:")),
        swap_pss_kib: kib_field(rollup, "SwapPss:"),
    }
}

/// RSS reduction accompanied by swap growth is not an allocation release.
pub fn residency() -> Residency {
    let rollup = read_small(Path::new("/proc/self/smaps_rollup")).unwrap_or_default();
    let status = if kib_field(&rollup, "Rss:").is_none() || kib_field(&rollup, "Swap:").is_none() {
        read_small(Path::new("/proc/self/status")).unwrap_or_default()
    } else {
        String::new()
    };
    parse_residency(&rollup, &status)
}

pub struct PressureGate {
    reserve_kib: u64,
    recovery_kib: u64,
    cooldown: Duration,
    next_allowed: Duration,
    critical: bool,
    pending: bool,
}

impl PressureGate {
    pub fn new(reserve_kib: u64, recovery_kib: u64, cooldown: Duration, warmup: Duration) -> Self {
        Self {
            reserve_kib,
            recovery_kib,
            cooldown,
            next_allowed: warmup,
            critical: false,
            pending: false,
        }
    }
    pub fn should_start(&mut self, available_kib: u64, now: Duration) -> bool {
        if available_kib >= self.recovery_kib {
            self.critical = false;
        }
        if available_kib <= self.reserve_kib {
            self.critical = true;
        }
        self.critical && !self.pending && now >= self.next_allowed
    }
    pub fn started(&mut self) {
        self.pending = true;
    }
    pub fn completed(&mut self, now: Duration) {
        self.pending = false;
        self.next_allowed = now.saturating_add(self.cooldown);
    }
}

#[cfg(test)]
#[path = "../tests/unit/pressure.rs"]
mod tests;
