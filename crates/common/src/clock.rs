//! INPUT: host libc CLOCK_MONOTONIC and its native timespec layout
//! OUTPUT: monotonic_nanos in the host uptime domain, with no private library epoch
//! POS: one scalar clock boundary shared by input timestamps and Java frame time

/// Native uptime nanoseconds. Returns zero if the host cannot provide this clock.
/// All linked copies read the same OS clock; no allocation, lock or epoch cache.
#[cfg(unix)]
pub fn monotonic_nanos() -> i64 {
    clock_nanos(libc::CLOCK_MONOTONIC)
}

/// CPU consumed by the calling thread, in nanoseconds.
#[cfg(target_os = "linux")]
pub fn thread_cpu_nanos() -> i64 {
    clock_nanos(libc::CLOCK_THREAD_CPUTIME_ID)
}

#[cfg(unix)]
fn clock_nanos(clock: libc::clockid_t) -> i64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: native timespec is initialized and exclusively borrowed for this
    // synchronous libc call; neither supported clock borrows a Rust owner.
    if unsafe { libc::clock_gettime(clock, &mut time) } != 0 {
        return 0;
    }
    (time.tv_sec as i64)
        .saturating_mul(1_000_000_000)
        .saturating_add(time.tv_nsec as i64)
}

#[cfg(not(unix))]
pub fn monotonic_nanos() -> i64 {
    0
}
