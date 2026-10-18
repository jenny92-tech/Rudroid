//! INPUT:  native allocator boundary with deterministic fake GBM calls and real owned fds
//! OUTPUT:  ABI layout and success/failure ownership assertions without a GPU
//! POS:    distinguishes C ABI/fd correctness from target-device graphics acceptance
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
static SERIAL: Mutex<()> = Mutex::new(());
static CREATED: AtomicUsize = AtomicUsize::new(0);
static DESTROYED: AtomicUsize = AtomicUsize::new(0);
static DEV_DESTROYED: AtomicUsize = AtomicUsize::new(0);
static EXPORTED: AtomicUsize = AtomicUsize::new(0);
static BAD_LAYOUT: AtomicUsize = AtomicUsize::new(0);
static STRIDE: AtomicUsize = AtomicUsize::new(2560);
unsafe extern "C" fn new_device(_: i32) -> Handle { 1usize as Handle }
unsafe extern "C" fn destroy_device(_: Handle) { DEV_DESTROYED.fetch_add(1, Ordering::Relaxed); }
unsafe extern "C" fn create(_: Handle, _: u32, _: u32, _: u32, _: *const u64, _: u32) -> Handle {
    CREATED.fetch_add(1, Ordering::Relaxed); 2usize as Handle
}
unsafe extern "C" fn destroy(_: Handle) { DESTROYED.fetch_add(1, Ordering::Relaxed); }
unsafe extern "C" fn modifier(_: Handle) -> u64 { BAD_LAYOUT.load(Ordering::Relaxed) as u64 }
unsafe extern "C" fn format(_: Handle) -> u32 { 0x34324241 }
unsafe extern "C" fn planes(_: Handle) -> i32 { 1 }
unsafe extern "C" fn stride(_: Handle) -> u32 { STRIDE.load(Ordering::Relaxed) as u32 }
unsafe extern "C" fn offset(_: Handle, _: i32) -> u32 { 0 }
unsafe extern "C" fn export(_: Handle) -> i32 {
    EXPORTED.fetch_add(1, Ordering::Relaxed);
    File::open("/dev/null").unwrap().into_raw_fd()
}
fn owner() -> Box<Allocator> {
    for counter in [&CREATED, &DESTROYED, &DEV_DESTROYED, &EXPORTED, &BAD_LAYOUT] { counter.store(0, Ordering::Relaxed); }
    STRIDE.store(2560, Ordering::Relaxed);
    let gbm = Box::leak(Box::new(Gbm { _library: libloading::os::unix::Library::this().into(),
        create_device: new_device, destroy_device, create, destroy, modifier, format, planes, stride, offset, fd: export }));
    Box::new(Allocator { gbm, device: 1, _drm: File::open("/dev/null").unwrap(), gate: Mutex::new(None) })
}
#[test]
fn wsialloc_v3_matches_the_native_build_gate() {
    assert_eq!(std::mem::size_of::<Format>(), 24);
    assert_eq!(std::mem::size_of::<AllocateInfo>(), 32);
    assert_eq!(std::mem::offset_of!(AllocateInfo, flags), 24);
    assert_eq!(std::mem::size_of::<AllocateResult>(), 80);
    assert_eq!(std::mem::offset_of!(AllocateResult, buffer_fds), 56);
    assert_eq!(std::mem::offset_of!(AllocateResult, is_disjoint), 72);
}
#[test]
fn transfers_a_live_fd_only_after_native_bo_destruction() {
    let _serial = SERIAL.lock().unwrap();
    let owner = Box::into_raw(owner());
    let format = Format { fourcc: 0x34324241, ..Format::default() };
    let info = AllocateInfo { formats: &format, format_count: 1, width: 640, height: 480, flags: 0 };
    let mut result = AllocateResult::default();
    // SAFETY: unique live allocator, readable input and writable result; native
    // fake GBM returns a real owned fd whose lifetime is verified below.
    unsafe {
        assert_eq!(wsialloc_alloc(owner, &info, &mut result), 0);
        assert_eq!(DESTROYED.load(Ordering::Relaxed), 1);
        assert_eq!(EXPORTED.load(Ordering::Relaxed), 1);
        let fd = result.buffer_fds[0];
        assert!(std::path::Path::new(&format!("/proc/self/fd/{fd}")).exists());
        drop(OwnedFd::from_raw_fd(fd));
        wsialloc_delete(owner);
        assert_eq!(DEV_DESTROYED.load(Ordering::Relaxed), 1);
    }
}
#[test]
fn rejects_layout_changes_and_query_flags_without_fd_leaks() {
    let _serial = SERIAL.lock().unwrap();
    let owner = Box::into_raw(owner());
    let format = Format { fourcc: 0x34324241, ..Format::default() };
    let mut info = AllocateInfo { formats: &format, format_count: 1, width: 640, height: 480, flags: 2 };
    let mut result = AllocateResult::default();
    // SAFETY: unique native owner and valid scalar/array leases; deletion ends it once.
    unsafe {
        assert_eq!(wsialloc_alloc(owner, &info, &mut result), -1);
        assert_eq!(CREATED.load(Ordering::Relaxed), 0);
        info.flags = 0;
        BAD_LAYOUT.store(1, Ordering::Relaxed);
        assert_eq!(wsialloc_alloc(owner, &info, &mut result), -2);
        assert_eq!(CREATED.load(Ordering::Relaxed), 1);
        assert_eq!(DESTROYED.load(Ordering::Relaxed), 1);
        assert_eq!(EXPORTED.load(Ordering::Relaxed), 0);
        assert_eq!(result.buffer_fds, [-1; 4]);
        wsialloc_delete(owner);
    }
}

#[test]
fn subsequent_images_must_keep_the_first_images_stride() {
    let _serial = SERIAL.lock().unwrap();
    let owner = Box::into_raw(owner());
    let format = Format { fourcc: 0x34324241, ..Format::default() };
    let info = AllocateInfo { formats: &format, format_count: 1, width: 640, height: 480, flags: 0 };
    let mut first = AllocateResult::default();
    let mut next = AllocateResult::default();
    // SAFETY: unique live owner and valid native leases; each successful fd
    // is reclaimed once. A changed stride must fail before exporting any fd.
    unsafe {
        assert_eq!(wsialloc_alloc(owner, &info, &mut first), 0);
        STRIDE.store(2816, Ordering::Relaxed);
        assert_eq!(wsialloc_alloc(owner, &info, &mut next), -2);
        assert_eq!(EXPORTED.load(Ordering::Relaxed), 1);
        assert_eq!(next.buffer_fds, [-1; 4]);
        STRIDE.store(2560, Ordering::Relaxed);
        assert_eq!(wsialloc_alloc(owner, &info, &mut next), 0);
        assert_eq!(CREATED.load(Ordering::Relaxed), 3);
        assert_eq!(DESTROYED.load(Ordering::Relaxed), 3);
        drop(OwnedFd::from_raw_fd(first.buffer_fds[0]));
        drop(OwnedFd::from_raw_fd(next.buffer_fds[0]));
        wsialloc_delete(owner);
    }
}
