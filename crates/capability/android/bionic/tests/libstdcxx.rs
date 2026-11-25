//! INPUT:  exact libstdc++.so@LIBC_O provider entries and the Linux ARM64 runtime
//! OUTPUT:  LP64 allocation, guard concurrency/retry and fatal-failure regressions
//! POS:    verifies the legacy Unity C++ imports without a C++ exception runtime
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {
    use std::ffi::{c_int, c_void, CStr};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{mpsc, Arc, Barrier};
    use std::time::Duration;

    unsafe fn query(name: &CStr, library: &CStr, version: &CStr) -> usize {
        // SAFETY: immutable C names live for the duration of the query.
        unsafe {
            android_bionic::rudroid_bionic_symbol(
                name.as_ptr(), library.as_ptr(), version.as_ptr(), std::ptr::null_mut(),
            )
        }
    }

    unsafe fn symbol(name: &CStr) -> usize {
        let address = unsafe { query(name, c"libstdc++.so", c"LIBC_O") };
        assert_ne!(address, 0, "missing {}", name.to_string_lossy());
        address
    }

    #[repr(C, align(8))]
    struct Guard {
        state: AtomicU32,
        _padding: u32,
    }
    const _: () = assert!(std::mem::size_of::<Guard>() == 8);
    const _: () = assert!(std::mem::align_of::<Guard>() == 8);

    impl Guard {
        fn new() -> Self {
            Self { state: AtomicU32::new(0), _padding: 0 }
        }
    }

    #[test]
    fn imports_resolve_only_through_libstdcxx_so_libc_o() {
        for name in [
            c"_Znwm", c"_Znam", c"_ZdlPv", c"_ZdaPv",
            c"__cxa_guard_acquire", c"__cxa_guard_release",
            c"__cxa_guard_abort", c"__cxa_pure_virtual",
        ] {
            // SAFETY: immutable literal names and exact production gates.
            unsafe {
                assert_ne!(query(name, c"libstdc++.so", c"LIBC_O"), 0, "{name:?}");
                assert_eq!(query(name, c"libstdc++.so", c"LIBC"), 0, "{name:?}");
                assert_eq!(query(name, c"libstdc++.so.6", c"LIBC_O"), 0, "{name:?}");
                assert_eq!(query(name, c"libc.so", c"LIBC_O"), 0, "{name:?}");
                assert_eq!(query(name, c"libm.so", c"LIBC_O"), 0, "{name:?}");
                assert_eq!(android_bionic::rudroid_bionic_symbol(
                    name.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut(),
                ), 0);
            }
        }
    }

    #[test]
    fn allocation_entries_share_the_native_aligned_heap_and_deleters() {
        type New = unsafe extern "C" fn(usize) -> *mut c_void;
        type Delete = unsafe extern "C" fn(*mut c_void);
        // SAFETY: exact scalar/pointer LP64 mangled operator prototypes. The
        // allocated spans stay live until their matching delete call.
        unsafe {
            let new: New = std::mem::transmute(symbol(c"_Znwm"));
            let new_array: New = std::mem::transmute(symbol(c"_Znam"));
            let delete: Delete = std::mem::transmute(symbol(c"_ZdlPv"));
            let delete_array: Delete = std::mem::transmute(symbol(c"_ZdaPv"));

            for size in [1_usize, 7, 64, 4096] {
                let allocation = new(size);
                assert!(!allocation.is_null());
                assert_eq!(allocation as usize % std::mem::align_of::<u128>(), 0);
                assert!(libc::malloc_usable_size(allocation) >= size);
                std::ptr::write_bytes(allocation.cast::<u8>(), 0xa5, size);
                delete(allocation);

                let array = new_array(size);
                assert!(!array.is_null());
                assert_eq!(array as usize % std::mem::align_of::<u128>(), 0);
                std::ptr::write_bytes(array.cast::<u8>(), 0x5a, size);
                delete_array(array);

                let native = libc::malloc(size);
                assert!(!native.is_null());
                delete(native);
            }
            delete(std::ptr::null_mut());
            delete_array(std::ptr::null_mut());
        }
    }

    #[test]
    fn allocation_and_pure_virtual_failures_abort_without_exceptions() {
        type New = unsafe extern "C" fn(usize) -> *mut c_void;
        type PureVirtual = unsafe extern "C" fn();
        // SAFETY: exact ABI prototypes. Each rejected operation runs in an
        // isolated child with core dumps disabled and must end in SIGABRT.
        unsafe {
            let new: New = std::mem::transmute(symbol(c"_Znwm"));
            let new_array: New = std::mem::transmute(symbol(c"_Znam"));
            let pure_virtual: PureVirtual = std::mem::transmute(symbol(c"__cxa_pure_virtual"));
            for mode in 0..3 {
                let pid = libc::fork();
                assert!(pid >= 0);
                if pid == 0 {
                    libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong);
                    match mode {
                        0 => { let _ = new(usize::MAX); }
                        1 => { let _ = new_array(usize::MAX); }
                        _ => pure_virtual(),
                    }
                    libc::_exit(77);
                }
                let mut status = 0;
                assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
                assert!(libc::WIFSIGNALED(status), "mode={mode}");
                assert_eq!(libc::WTERMSIG(status), libc::SIGABRT, "mode={mode}");
            }
        }
    }

    #[test]
    fn guard_initializes_once_publishes_the_low_byte_and_retries_after_abort() {
        type Acquire = unsafe extern "C" fn(*mut Guard) -> c_int;
        type Release = unsafe extern "C" fn(*mut Guard);
        // SAFETY: the compile-time-asserted eight-byte guard shape and exact
        // Bionic guard prototypes; each guard remains live through teardown.
        unsafe {
            let acquire: Acquire = std::mem::transmute(symbol(c"__cxa_guard_acquire"));
            let release: Release = std::mem::transmute(symbol(c"__cxa_guard_release"));
            let abort: Release = std::mem::transmute(symbol(c"__cxa_guard_abort"));

            let guard = Box::new(Guard::new());
            let pointer = Box::as_ref(&guard) as *const Guard as *mut Guard;
            assert_eq!((*pointer).state.load(Ordering::Acquire), 0);
            assert_eq!(acquire(pointer), 1);
            assert_eq!((*pointer).state.load(Ordering::Acquire), 0x100);
            assert_eq!((*pointer).state.load(Ordering::Acquire) & 0xff, 0);
            release(pointer);
            assert_eq!((*pointer).state.load(Ordering::Acquire), 1);
            assert_eq!((*pointer).state.load(Ordering::Acquire) & 0xff, 1);
            assert_eq!(acquire(pointer), 0);

            let retry = Box::new(Guard::new());
            let pointer = Box::as_ref(&retry) as *const Guard as *mut Guard;
            assert_eq!(acquire(pointer), 1);
            abort(pointer);
            assert_eq!((*pointer).state.load(Ordering::Acquire), 0);
            assert_eq!(acquire(pointer), 1);
            release(pointer);
        }
    }

    #[test]
    fn guard_waiters_observe_release_and_do_not_reinitialize() {
        type Acquire = unsafe extern "C" fn(*mut Guard) -> c_int;
        type Release = unsafe extern "C" fn(*mut Guard);
        // SAFETY: both workers receive a stable Arc-owned guard address; the
        // guard persists until after both joins, and the acquire sequence is
        // synchronized by the ABI itself plus test barriers.
        unsafe {
            let acquire: Acquire = std::mem::transmute(symbol(c"__cxa_guard_acquire"));
            let release: Release = std::mem::transmute(symbol(c"__cxa_guard_release"));
            let guard = Arc::new(Guard::new());
            let start = Arc::new(Barrier::new(3));
            let finish = Arc::new(Barrier::new(2));
            let (sender, receiver) = mpsc::channel();
            let mut workers = Vec::new();
            for _ in 0..2 {
                let guard = Arc::clone(&guard);
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                let sender = sender.clone();
                workers.push(std::thread::spawn(move || {
                    start.wait();
                    let pointer = Arc::as_ptr(&guard) as *mut Guard;
                    let acquired = acquire(pointer);
                    sender.send(acquired).unwrap();
                    if acquired == 1 {
                        finish.wait();
                        release(pointer);
                    }
                }));
            }
            start.wait();
            assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
            let mut waiting = false;
            for _ in 0..2000 {
                if guard.state.load(Ordering::Acquire) == 0x200 {
                    waiting = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(waiting, "second guard caller did not enter the wait state");
            assert!(matches!(
                receiver.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            finish.wait();
            assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
            for worker in workers { worker.join().unwrap(); }
            assert_eq!(guard.state.load(Ordering::Acquire), 1);
            assert_eq!(guard.state.load(Ordering::Acquire) & 0xff, 1);
        }
    }
}
