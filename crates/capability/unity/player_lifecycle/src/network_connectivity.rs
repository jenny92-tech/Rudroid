//! INPUT:  Unity's protected connectivity query and Linux default routes/link state
//! OUTPUT:  JNI reachability value 0 (absent) or 2 (local network) with bounded polling
//! POS:    safe host-link query matching the observed APK enum; native Curl still owns connections
#![forbid(unsafe_code)]
use rudroid_common::jni_env::{JObject, JniEnv};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant};
static CACHE: OnceLock<Mutex<Option<(Instant, i32)>>> = OnceLock::new();
static LOGGED: AtomicU8 = AtomicU8::new(0);

fn default_interfaces(text: &str, ipv6: bool) -> Vec<&str> {
    text.lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split_whitespace().collect();
            let (destination, prefix, flags, interface) = if ipv6 {
                (
                    *columns.first()?,
                    *columns.get(1)?,
                    *columns.get(8)?,
                    *columns.get(9)?,
                )
            } else {
                (
                    *columns.get(1)?,
                    *columns.get(7)?,
                    *columns.get(3)?,
                    *columns.first()?,
                )
            };
            let default = if ipv6 {
                destination == "00000000000000000000000000000000" && prefix == "00"
            } else {
                destination == "00000000" && prefix == "00000000"
            };
            let flags = u32::from_str_radix(flags, 16).ok()?;
            let safe_name = interface != "lo"
                && interface != "."
                && interface != ".."
                && interface.len() <= 15
                && interface
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
            (default && flags & 1 != 0 && flags & 0x200 == 0 && safe_name).then_some(interface)
        })
        .collect()
}
fn detected() -> i32 {
    for (file, ipv6) in [("/proc/net/route", false), ("/proc/net/ipv6_route", true)] {
        let Ok(routes) = std::fs::read_to_string(file) else {
            continue;
        };
        for interface in default_interfaces(&routes, ipv6) {
            if std::fs::read_to_string(format!("/sys/class/net/{interface}/operstate"))
                .is_ok_and(|state| matches!(state.trim(), "up" | "unknown"))
            {
                return 2;
            }
        }
    }
    0
}
fn current() -> i32 {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let now = Instant::now();
    if let Some((checked, value)) = *cache.lock().unwrap_or_else(|e| e.into_inner()) {
        if now.saturating_duration_since(checked) < Duration::from_secs(1) {
            return value;
        }
    }
    // Never hold the cache lock over filesystem I/O; there is no worker thread.
    let value = detected();
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, value));
    value
}
pub(super) extern "C" fn query(_env: JniEnv, _player: JObject) -> i32 {
    let value = current();
    if LOGGED
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < 3).then_some(n.saturating_add(1))
        })
        .is_ok()
    {
        eprintln!("[UNITY] network connectivity query: host link={value}");
    }
    value
}

#[cfg(test)]
#[path = "../tests/unit/network_connectivity.rs"]
mod tests;
