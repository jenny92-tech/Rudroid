//! INPUT:  private items of `src/events.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/events.rs`; declared there with `#[path]`
use super::*;
use std::alloc::{GlobalAlloc, System};
use std::cell::Cell;
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static DENY: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;
// Test-binary-only allocator: thread-local failure injection and counts;
// production still uses its normal allocator and exports no test controls.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if DENY.try_with(Cell::get).unwrap_or(false) { return ptr::null_mut(); }
        if TRACK.try_with(Cell::get).unwrap_or(false) { let _ = ALLOCS.try_with(|count| count.set(count.get() + 1)); }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) { System.dealloc(pointer, layout); }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if DENY.try_with(Cell::get).unwrap_or(false) { return ptr::null_mut(); }
        if TRACK.try_with(Cell::get).unwrap_or(false) { let _ = ALLOCS.try_with(|count| count.set(count.get() + 1)); }
        System.realloc(pointer, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
fn register(registry: &mut Registry, fd: c_int) -> u64 {
    let (index, fresh, generation) = registry.reserve(fd).unwrap();
    let slot = Slot { generation, fd, ident: fd, next_free: NONE, callback: None, data: ptr::null_mut() };
    let token = slot.token(index); registry.publish(index, fresh, slot); token
}
#[test]
fn token_replacement_free_slot_reuse_and_empty_release() {
    let mut registry = Registry::default();
    let first = register(&mut registry, 7);
    assert_eq!(registry.slots.capacity(), 1);
    assert_eq!(size_of::<Slot>(), 32);
    assert_eq!(registry.snapshot(first).unwrap().fd, 7);
    let second = register(&mut registry, 7);
    assert!(registry.snapshot(first).is_none());
    let third = register(&mut registry, 8);
    registry.retire(0);
    let fourth = register(&mut registry, 9);
    assert_eq!(fourth as u32, second as u32);
    assert!(registry.snapshot(second).is_none());
    assert_eq!(registry.snapshot(third).unwrap().fd, 8);
    assert!(registry.snapshot(0).is_none());
    assert!(registry.snapshot(u64::MAX).is_none());
    registry.retire(0); registry.retire(1);
    assert_eq!(registry.slots.capacity(), 0);
    let next = register(&mut registry, 7);
    assert_ne!(next, first);
    assert!(registry.snapshot(first).is_none());
    registry.generation = u32::MAX;
    assert_eq!(registry.reserve(7), Err(libc::EOVERFLOW));
    assert!(registry.snapshot(next).is_some());
}
#[test]
fn reference_count_rejects_zero_and_overflow_without_wrapping() {
    for count in [0, isize::MAX as usize, usize::MAX] {
        let refs = AtomicUsize::new(count);
        assert!(!increment_reference(&refs)); assert_eq!(refs.load(Ordering::Relaxed), count);
    }
    let refs = AtomicUsize::new(1); assert!(increment_reference(&refs));
    assert_eq!(refs.load(Ordering::Relaxed), 2);
}
#[test]
fn registry_growth_amortizes_allocations_and_drops_empty_capacity() {
    let mut registry = Registry::default();
    ALLOCS.with(|count| count.set(0)); TRACK.with(|track| track.set(true));
    for fd in 0..256 { register(&mut registry, fd); }
    TRACK.with(|track| track.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 9);
    assert_eq!(registry.slots.capacity() * size_of::<Slot>(), 8192);
    for index in 0..256 { registry.retire(index); }
    assert_eq!(registry.slots.capacity(), 0);
    println!("NATIVE_EVENTS_MEMORY registrations=256 growth_allocations=9 slot_bytes=8192 after_remove_bytes=0");
}
#[test]
fn shrinking_vacant_suffix_preserves_active_tokens_and_is_fallible() {
    let mut registry = Registry::default();
    let first = register(&mut registry, 0);
    for fd in 1..256 { register(&mut registry, fd); }
    DENY.with(|deny| deny.set(true));
    for index in (1..256).rev() { registry.retire(index); }
    DENY.with(|deny| deny.set(false));
    assert_eq!(registry.slots.len(), 1);
    assert_eq!(registry.snapshot(first).unwrap().fd, 0);
    register(&mut registry, 1); registry.retire(1);
    assert_eq!(registry.slots.capacity(), 1);
    assert_eq!(registry.snapshot(first).unwrap().fd, 0);
    registry.retire(0); assert_eq!(registry.slots.capacity(), 0);
}
#[test]
fn allocation_failures_recover_internal_fds_and_ready_poll_does_not_allocate() {
    let _fds=crate::TEST_FDS.lock().unwrap_or_else(|error| error.into_inner());
    // Fresh native TLS; worker exit also tests cleanup after successful retry.
    std::thread::spawn(|| unsafe {
        let fd_count = || std::fs::read_dir("/proc/self/fd").unwrap().count();
        let before = fd_count();
        DENY.with(|deny| deny.set(true));
        let failed = prepare(1); let error = errno();
        DENY.with(|deny| deny.set(false));
        assert!(failed.is_null()); assert_eq!(error, libc::ENOMEM);
        assert_eq!(fd_count(), before, "OOM after epoll/eventfd creation must close both");
        let looper = prepare(1); assert!(!looper.is_null());
        let mut pipes = [0; 2]; assert_eq!(libc::pipe(pipes.as_mut_ptr()), 0);
        let mut calls = 0usize;
        unsafe extern "C" fn ready(_: c_int, _: c_int, data: *mut c_void) -> c_int {
            *data.cast::<usize>() += 1; 1
        }
        let data = (&mut calls as *mut usize).cast();
        DENY.with(|deny| deny.set(true));
        let failed = add_fd(looper, pipes[0], -2, 1, Some(ready), data); let error = errno();
        DENY.with(|deny| deny.set(false));
        assert_eq!(failed, -1); assert_eq!(error, libc::ENOMEM);
        assert_eq!((*looper).registry().live, 0);
        assert_eq!(poll_once(0, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()), TIMEOUT);
        assert_eq!(add_fd(looper, pipes[0], -2, 1, Some(ready), data), 1);
        assert_eq!(libc::write(pipes[1], c"x".as_ptr().cast(), 1), 1);
        ALLOCS.with(|count| count.set(0)); TRACK.with(|track| track.set(true));
        for _ in 0..10000 { assert_eq!(poll_once(0, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()), CALLBACK); }
        TRACK.with(|track| track.set(false));
        assert_eq!(ALLOCS.with(Cell::get), 0); assert_eq!(calls, 10000);
        assert_eq!(remove_fd(looper, pipes[0]), 1);
        assert_eq!((*looper).registry().slots.capacity(), 0);
        assert_eq!(libc::close(pipes[0]), 0); assert_eq!(libc::close(pipes[1]), 0);
        println!("NATIVE_EVENTS_ALLOC ready_polls=10000 allocations=0 prepare_oom=ENOMEM add_oom=ENOMEM");
    }).join().unwrap();
}
