//! INPUT: host libc CLOCK_MONOTONIC and its native timespec layout
//! OUTPUT: monotonic_nanos in the host uptime domain, with no private library epoch
//! POS: one scalar clock boundary shared by input timestamps and Java frame time

/// Native uptime nanoseconds. Returns zero if the host cannot provide this clock.
/// All linked copies read the same OS clock; no allocation, lock or epoch cache.
#[cfg(unix)]
pub fn monotonic_nanos() -> i64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: native timespec is initialized and exclusively borrowed for this
    // synchronous libc call; CLOCK_MONOTONIC retains no pointer or Rust owner.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return 0;
    }
    (time.tv_sec as i64).saturating_mul(1_000_000_000).saturating_add(time.tv_nsec as i64)
}

#[cfg(not(unix))]
pub fn monotonic_nanos() -> i64 { 0 }
