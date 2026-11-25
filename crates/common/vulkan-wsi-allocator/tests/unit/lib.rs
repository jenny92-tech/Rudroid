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
static CREATE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static CREATE_ERRNO: AtomicUsize = AtomicUsize::new(0);
static EXPORT_FAILURES: AtomicUsize = AtomicUsize::new(0);
static EXPORT_ERRNO: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn new_device(_: i32) -> Handle { 1usize as Handle }
unsafe extern "C" fn destroy_device(_: Handle) { DEV_DESTROYED.fetch_add(1, Ordering::Relaxed); }
unsafe extern "C" fn create(_: Handle, _: u32, _: u32, _: u32, _: *const u64, _: u32) -> Handle {
    let call = CREATED.fetch_add(1, Ordering::Relaxed);
    if call < CREATE_FAILURES.load(Ordering::Relaxed) {
        let error = CREATE_ERRNO.load(Ordering::Relaxed);
        if error != 0 {
            // SAFETY: native GNU errno is writable only by this test thread.
            unsafe { *__errno_location() = error as i32 };
        }
        std::ptr::null_mut()
    } else { 2usize as Handle }
}
unsafe extern "C" fn destroy(_: Handle) {
    DESTROYED.fetch_add(1, Ordering::Relaxed);
    // SAFETY: this thread owns errno. Model cleanup clobbering the failure
    // errno so classification must capture it before the BO is dropped.
    unsafe { *__errno_location() = 5 };
}
unsafe extern "C" fn modifier(_: Handle) -> u64 { BAD_LAYOUT.load(Ordering::Relaxed) as u64 }
unsafe extern "C" fn format(_: Handle) -> u32 { 0x34324241 }
unsafe extern "C" fn planes(_: Handle) -> i32 { 1 }
unsafe extern "C" fn stride(_: Handle) -> u32 { STRIDE.load(Ordering::Relaxed) as u32 }
unsafe extern "C" fn offset(_: Handle, _: i32) -> u32 { 0 }
unsafe extern "C" fn export(_: Handle) -> i32 {
    let call = EXPORTED.fetch_add(1, Ordering::Relaxed);
    if call < EXPORT_FAILURES.load(Ordering::Relaxed) {
        let error = EXPORT_ERRNO.load(Ordering::Relaxed);
        if error != 0 {
            // SAFETY: native GNU errno is writable only by this test thread.
            unsafe { *__errno_location() = error as i32 };
        }
        return -1;
    }
    File::open("/dev/null").unwrap().into_raw_fd()
}
fn owner() -> Box<Allocator> {
    for counter in [&CREATED, &DESTROYED, &DEV_DESTROYED, &EXPORTED, &BAD_LAYOUT,
        &CREATE_FAILURES, &CREATE_ERRNO, &EXPORT_FAILURES, &EXPORT_ERRNO] { counter.store(0, Ordering::Relaxed); }
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

#[test]
fn create_and_export_failures_keep_real_resource_evidence_and_fd_ownership() {
    let _serial = SERIAL.lock().unwrap();
    for export_failure in [false, true] {
        for error in [0, 1, 5, 11, 12, 22, 23, 24, 28, 95] {
            let owner = Box::into_raw(owner());
            if export_failure {
                EXPORT_FAILURES.store(usize::MAX, Ordering::Relaxed);
                EXPORT_ERRNO.store(error, Ordering::Relaxed);
            } else {
                CREATE_FAILURES.store(usize::MAX, Ordering::Relaxed);
                CREATE_ERRNO.store(error, Ordering::Relaxed);
            }
            let format = Format { fourcc: 0x34324241, ..Format::default() };
            let info = AllocateInfo { formats: &format, format_count: 1, width: 640, height: 480, flags: 0 };
            let mut result = AllocateResult::default();
            // SAFETY: unique fake owner and guarded native inputs; failed calls
            // transfer no fd. Seed stale ENOMEM to detect an uncleared errno.
            unsafe {
                *__errno_location() = 12;
                let expected = if [11, 12, 23, 24, 28].contains(&error) { -3 } else { -2 };
                assert_eq!(wsialloc_alloc(owner, &info, &mut result), expected, "export={export_failure} errno={error}");
                assert_eq!(result.buffer_fds, [-1; 4]);
                assert_eq!(DESTROYED.load(Ordering::Relaxed), usize::from(export_failure));
                assert!((*owner).gate.lock().unwrap().is_none());
                wsialloc_delete(owner);
            }
        }
    }
}

#[test]
fn later_candidates_can_succeed_after_resource_failure() {
    let _serial = SERIAL.lock().unwrap();
    for export_failure in [false, true] {
        let owner = Box::into_raw(owner());
        if export_failure {
            EXPORT_FAILURES.store(1, Ordering::Relaxed);
            EXPORT_ERRNO.store(24, Ordering::Relaxed);
        } else {
            CREATE_FAILURES.store(1, Ordering::Relaxed);
            CREATE_ERRNO.store(12, Ordering::Relaxed);
        }
        let formats = [Format { fourcc: 0x34324241, ..Format::default() }; 2];
        let info = AllocateInfo { formats: formats.as_ptr(), format_count: 2, width: 640, height: 480, flags: 0 };
        let mut result = AllocateResult::default();
        // SAFETY: one native lease per attempt; only the successful fd is owned
        // by this caller, and all successful BOs must already be destroyed.
        unsafe {
            assert_eq!(wsialloc_alloc(owner, &info, &mut result), 0);
            assert!(result.buffer_fds[0] >= 0);
            assert_eq!(CREATED.load(Ordering::Relaxed), 2);
            assert_eq!(DESTROYED.load(Ordering::Relaxed), if export_failure { 2 } else { 1 });
            drop(OwnedFd::from_raw_fd(result.buffer_fds[0]));
            wsialloc_delete(owner);
        }
    }
}

#[test]
fn resource_evidence_survives_a_later_layout_rejection() {
    let _serial = SERIAL.lock().unwrap();
    let owner = Box::into_raw(owner());
    CREATE_FAILURES.store(1, Ordering::Relaxed);
    CREATE_ERRNO.store(12, Ordering::Relaxed);
    let formats = [Format { fourcc: 0x34324241, ..Format::default() },
        Format { fourcc: 0x34325241, ..Format::default() }];
    let info = AllocateInfo { formats: formats.as_ptr(), format_count: 2, width: 640, height: 480, flags: 0 };
    let mut result = AllocateResult::default();
    // SAFETY: unique fake owner; the second BO rejects a returned format
    // mismatch, and neither failed candidate transfers a descriptor.
    unsafe {
        assert_eq!(wsialloc_alloc(owner, &info, &mut result), -3);
        assert_eq!(CREATED.load(Ordering::Relaxed), 2);
        assert_eq!(DESTROYED.load(Ordering::Relaxed), 1);
        assert_eq!(EXPORTED.load(Ordering::Relaxed), 0);
        assert_eq!(result.buffer_fds, [-1; 4]);
        wsialloc_delete(owner);
    }
}

#[test]
fn missing_render_node_uses_the_existing_primary_card_for_allocation() {
    let mut attempted = Vec::new();
    let file = open_drm_with(None, |path| {
        attempted.push(path.to_owned());
        if path.ends_with("renderD128") { Err(std::io::ErrorKind::NotFound.into()) }
        else { File::open("/dev/null") }
    }).unwrap();
    assert_eq!(attempted, ["/dev/dri/renderD128", "/dev/dri/card0"]);
    assert!(file.metadata().is_ok());
}

#[test]
fn existing_render_node_and_permission_errors_never_fall_through_to_a_card() {
    for denied in [false, true] {
        let mut attempted = Vec::new();
        let result = open_drm_with(None, |path| {
            attempted.push(path.to_owned());
            if denied { Err(std::io::ErrorKind::PermissionDenied.into()) } else { File::open("/dev/null") }
        });
        assert_eq!(attempted, ["/dev/dri/renderD128"]);
        assert_eq!(result.is_err(), denied);
    }
}

#[test]
fn explicit_drm_node_is_respected_and_unrelated_paths_never_open() {
    for path in ["/dev/dri/card0", "/dev/dri/card15", "/dev/dri/renderD128", "/dev/dri/renderD255"] {
        let mut attempted = Vec::new();
        let error = open_drm_with(Some(path), |selected| {
            attempted.push(selected.to_owned());
            Err(std::io::ErrorKind::NotFound.into())
        }).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(attempted, [path]);
    }
    for path in ["/tmp/card0", "/dev/dri/card0/../card1", "/dev/dri/card16", "/dev/dri/renderD127", "/dev/dri/renderD256"] {
        let error = open_drm_with(Some(path), |_| panic!("invalid device must not reach filesystem access")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
