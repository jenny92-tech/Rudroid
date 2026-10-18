//! INPUT:  SDL session C API, Xvfb/Mesa, thread-local allocation counter
//! OUTPUT:  Acceptance of zero allocations and stable string pointers across repeated report queries
//! POS:    Verifies the render session reuses one copy of metadata instead of copying or leaking it per query

#![cfg(feature = "sdl")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_void;

use platform_sdl_runtime::c_api::{
    rudroid_sdl_session_create, rudroid_sdl_session_destroy, rudroid_sdl_session_report,
    CRudroidSdlReport,
};

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

fn record(size: usize) {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        ALLOCS.with(|value| value.set(value.get() + 1));
        BYTES.with(|value| value.set(value.get() + size));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Session(*mut c_void);

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { rudroid_sdl_session_destroy(self.0) }; // No guest entry or report borrow survives this guard.
    }
}

fn empty_report() -> CRudroidSdlReport {
    CRudroidSdlReport {
        window_w: 0,
        window_h: 0,
        major: 0,
        minor: 0,
        profile_mask: 0,
        swap_count: 0,
        readback_non_background: 0,
        video_driver: std::ptr::null(),
        gl_vendor: std::ptr::null(),
        gl_renderer: std::ptr::null(),
        gl_version: std::ptr::null(),
    }
}

#[test]
fn repeated_reports_borrow_the_same_session_strings_without_allocating() {
    let session = Session(rudroid_sdl_session_create(64, 64, 3, 2));
    assert!(!session.0.is_null(), "requires the real Xvfb/Mesa SDL kit");
    let mut first = empty_report();
    assert_eq!(rudroid_sdl_session_report(session.0, Some(&mut first)), 0);
    // A missing out pointer and a foreign handle take the old null path.
    assert_eq!(rudroid_sdl_session_report(session.0, None), -1);
    assert_eq!(rudroid_sdl_session_report(0xdead as *const c_void, Some(&mut first)), -1);
    let mut stable = true;
    let mut next = empty_report();
    TRACK.with(|track| track.set(true));
    for _ in 0..10_000 {
        let result = rudroid_sdl_session_report(session.0, Some(&mut next));
        if result != 0 {
            TRACK.with(|track| track.set(false));
            panic!("live session report failed: {result}");
        }
        stable &= first.video_driver == next.video_driver
            && first.gl_vendor == next.gl_vendor
            && first.gl_renderer == next.gl_renderer
            && first.gl_version == next.gl_version;
    }
    TRACK.with(|track| track.set(false));
    let allocations = ALLOCS.with(Cell::get);
    let bytes = BYTES.with(Cell::get);
    eprintln!("[GPU COST] 10000 reports: allocations={allocations} requested_bytes={bytes} stable_pointers={stable}");
    assert_eq!(
        allocations, 0,
        "reports must not allocate or leak copied strings"
    );
    assert!(
        stable,
        "all C facades must borrow the session's cached strings"
    );
}
