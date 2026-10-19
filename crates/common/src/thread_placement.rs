//! INPUT:  Linux CPU capacity, the initial allowed mask and current-thread status
//! OUTPUT:  optional one-shot placement on other initially permitted online CPUs
//! POS:    owns the current-thread-only affinity boundary; never targets another TID
use std::{fs::File, io::{self, Read}, path::Path};

#[cfg(target_os = "linux")]
const CPU_LIMIT: usize = libc::CPU_SETSIZE as usize;
#[cfg(not(target_os = "linux"))]
const CPU_LIMIT: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadPlacement {
    primary: usize,
    workers: Vec<usize>,
}

impl ThreadPlacement {
    /// Capture before the guest narrows its affinity. Only the measured shape
    /// of one higher-capacity and two equal lower-capacity CPUs is eligible.
    /// Missing capacity data and every other topology retain engine placement.
    pub fn capture() -> io::Result<Option<Self>> {
        let allowed = current_allowed_cpus()?;
        if allowed.len() != 3 { return Ok(None); }
        let mut topology = Vec::new();
        for cpu in allowed {
            let value = bounded_text(&Path::new("/sys/devices/system/cpu")
                .join(format!("cpu{cpu}/cpu_capacity")))?;
            let capacity = value.trim().parse().map_err(|_| invalid("invalid CPU capacity"))?;
            topology.push((cpu, capacity));
        }
        Ok(Self::from_topology(&topology))
    }

    fn from_topology(topology: &[(usize, u32)]) -> Option<Self> {
        if topology.len() != 3 || topology.iter().any(|(cpu, cap)| *cpu >= CPU_LIMIT || *cap == 0) {
            return None;
        }
        let mut sorted = topology.to_vec();
        sorted.sort_unstable_by_key(|&(cpu, capacity)| (capacity, cpu));
        let [(a, low), (b, same), (primary, high)] = sorted.as_slice() else { return None; };
        if low != same || low >= high || a == b || a == primary || b == primary { return None; }
        let mut workers = vec![*a, *b];
        workers.sort_unstable();
        Some(Self { primary: *primary, workers })
    }

    pub fn primary_cpu(&self) -> usize { self.primary }
    pub fn worker_cpus(&self) -> &[usize] { &self.workers }

    /// Invoke on the presenting worker itself, after its first successful
    /// present. Both it and the process leader must still share the singleton
    /// high-capacity mask. No stored thread ID can be reused by another process.
    pub fn apply_current(&self, expected_name: &str) -> io::Result<bool> {
        let worker = bounded_text(Path::new("/proc/thread-self/status"))?;
        let leader = bounded_text(Path::new("/proc/self/status"))?;
        let allowed = current_allowed_cpus()?;
        if !self.matches(&worker, &leader, &allowed, expected_name) { return Ok(false); }
        set_current_allowed_cpus(&self.workers)?;
        Ok(true)
    }

    fn matches(&self, worker: &str, leader: &str, allowed: &[usize], name: &str) -> bool {
        let field = |text: &str, key: &str| text.lines().find_map(|line|
            line.strip_prefix(key).map(str::trim)).map(str::to_owned);
        let tid = field(worker, "Pid:").and_then(|v| v.parse::<u32>().ok());
        let tgid = field(worker, "Tgid:").and_then(|v| v.parse::<u32>().ok());
        tid.is_some() && tgid.is_some() && tid != tgid
            && field(leader, "Pid:").and_then(|v| v.parse::<u32>().ok()) == tgid
            && field(worker, "Name:").as_deref() == Some(name)
            && allowed == [self.primary]
            && field(leader, "Cpus_allowed_list:").as_deref() == Some(self.primary.to_string().as_str())
    }
}

fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, message) }
fn bounded_text(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?.take(8193).read_to_string(&mut text)?;
    if text.len() > 8192 { return Err(invalid("CPU status exceeds 8 KiB")); }
    Ok(text)
}

/// Read only the calling thread's effective affinity; no guest pointer or TID.
pub fn current_allowed_cpus() -> io::Result<Vec<usize>> {
    #[cfg(target_os = "linux")]
    // SAFETY: zero is valid for cpu_set_t; the kernel writes only this owned
    // stack object. CPU_ISSET indices stay below its native CPU_SETSIZE.
    unsafe {
        let saved_errno = *libc::__errno_location();
        let mut mask: libc::cpu_set_t = std::mem::zeroed();
        let result = if libc::sched_getaffinity(0, std::mem::size_of_val(&mask), &mut mask) != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok((0..libc::CPU_SETSIZE as usize).filter(|&cpu| libc::CPU_ISSET(cpu, &mask)).collect())
        };
        *libc::__errno_location() = saved_errno;
        result
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::new(io::ErrorKind::Unsupported, "thread placement requires Linux"))
}

fn set_current_allowed_cpus(cpus: &[usize]) -> io::Result<()> {
    if cpus.is_empty() || cpus.iter().any(|&cpu| cpu >= CPU_LIMIT) {
        return Err(invalid("empty or out-of-range CPU mask"));
    }
    #[cfg(target_os = "linux")]
    // SAFETY: checked indices fit cpu_set_t, initialized on this stack. PID 0
    // changes only this executing thread, which cannot disappear mid-call;
    // no pointer escapes, external task ID, CPU hotplug or frequency write.
    unsafe {
        let saved_errno = *libc::__errno_location();
        let mut mask: libc::cpu_set_t = std::mem::zeroed();
        for &cpu in cpus { libc::CPU_SET(cpu, &mut mask); }
        let result = if libc::sched_setaffinity(0, std::mem::size_of_val(&mask), &mask) != 0 {
            Err(io::Error::last_os_error())
        } else { Ok(()) };
        *libc::__errno_location() = saved_errno;
        result
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::new(io::ErrorKind::Unsupported, "thread placement requires Linux"))
}

#[cfg(test)]
#[path = "../tests/unit/thread_placement.rs"]
mod tests;
