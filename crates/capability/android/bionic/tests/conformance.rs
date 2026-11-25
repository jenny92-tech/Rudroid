//! INPUT:  production Bionic queries, independent Android LP64 structures and private native threads
//! OUTPUT:  signal mask, numeric IPv4/IPv6 DNS and pathconf value/error ABI assertions
//! POS:    supplements export coverage with deterministic behavior without game or network dependencies
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {
    use std::ffi::{c_char, c_int, c_void, CStr};

    fn address(name: &CStr) -> usize {
        // SAFETY: immutable names and the attested public library/version query ABI.
        let address = unsafe { android_bionic::rudroid_bionic_symbol(
            name.as_ptr(), c"libc.so".as_ptr(), c"LIBC".as_ptr(), std::ptr::null_mut(),
        ) };
        assert_ne!(address, 0, "missing {name:?}");
        address
    }

    #[test]
    fn every_signal_bit_and_invalid_number_preserve_the_android_eight_byte_extent() {
        #[repr(C)] struct Mask { before: u64, bits: u64, after: u64 }
        let mut mask = Mask { before: 71, bits: 0, after: 82 };
        // SAFETY: exact Android sigset_t scalar-pointer prototypes; every output
        // is eight bytes within guarded storage. No signal is raised or installed.
        unsafe {
            type Change = unsafe extern "C" fn(*mut u64, c_int) -> c_int;
            let empty: unsafe extern "C" fn(*mut u64) -> c_int = std::mem::transmute(address(c"sigemptyset"));
            let fill: unsafe extern "C" fn(*mut u64) -> c_int = std::mem::transmute(address(c"sigfillset"));
            let add: Change = std::mem::transmute(address(c"sigaddset"));
            let delete: Change = std::mem::transmute(address(c"sigdelset"));
            let member: unsafe extern "C" fn(*const u64, c_int) -> c_int = std::mem::transmute(address(c"sigismember"));
            assert_eq!(empty(&mut mask.bits), 0);
            for signal in 1..=64 {
                assert_eq!(add(&mut mask.bits, signal), 0);
                assert_eq!(member(&mask.bits, signal), 1);
                assert_eq!(mask.bits, if signal == 64 { u64::MAX } else { (1_u64 << signal) - 1 });
            }
            assert_eq!(fill(&mut mask.bits), 0);
            assert_eq!(mask.bits, u64::MAX);
            for signal in 1..=64 {
                assert_eq!(delete(&mut mask.bits, signal), 0);
                assert_eq!(member(&mask.bits, signal), 0);
            }
            for signal in [i32::MIN, -1, 0, 65, i32::MAX] {
                mask.bits = 0x1234;
                *libc::__errno_location() = 0;
                assert_eq!(add(&mut mask.bits, signal), -1);
                assert_eq!(*libc::__errno_location(), libc::EINVAL);
                assert_eq!(delete(&mut mask.bits, signal), -1);
                assert_eq!(member(&mask.bits, signal), -1);
                assert_eq!(mask.bits, 0x1234);
            }
        }
        assert_eq!((mask.before, mask.after), (71, 82));
    }

    #[test]
    fn pthread_signal_masks_keep_native_errno_and_only_touch_the_owned_thread() {
        std::thread::spawn(|| {
            #[repr(C)] struct Mask { before: u64, bits: u64, after: u64 }
            let mut previous = Mask { before: 17, bits: 0, after: 19 };
            let mut current = Mask { before: 23, bits: 0, after: 29 };
            // SAFETY: Android's how constants are 0/1/2 and sigset_t is one
            // u64. Only this joined test thread changes its mask; no signal is sent.
            unsafe {
                let mask: unsafe extern "C" fn(c_int, *const u64, *mut u64) -> c_int =
                    std::mem::transmute(address(c"pthread_sigmask"));
                let usr1 = 1_u64 << 9;
                *libc::__errno_location() = 73;
                assert_eq!(mask(0, &usr1, &mut previous.bits), 0);
                assert_eq!(*libc::__errno_location(), 73);
                assert_eq!(mask(0, std::ptr::null(), &mut current.bits), 0);
                assert_ne!(current.bits & usr1, 0);
                assert_eq!(mask(99, &usr1, std::ptr::null_mut()), libc::EINVAL);
                assert_eq!(*libc::__errno_location(), 73);
                assert_eq!(mask(2, &previous.bits, std::ptr::null_mut()), 0);
            }
            assert_eq!((previous.before, previous.after, current.before, current.after), (17, 19, 23, 29));
        }).join().unwrap();
    }

    #[repr(C)]
    struct AddrInfo {
        flags: c_int, family: c_int, kind: c_int, protocol: c_int, length: u32,
        canonical: *mut c_char, address: *mut c_void, next: *mut AddrInfo,
    }
    struct Addresses { head: *mut AddrInfo, free: unsafe extern "C" fn(*mut AddrInfo) }
    impl Drop for Addresses {
        fn drop(&mut self) {
            // SAFETY: the guard exclusively owns the head returned by this
            // Bionic service and releases it with its paired allocator once.
            unsafe { (self.free)(self.head) };
        }
    }

    #[test]
    fn numeric_ipv4_ipv6_resolution_translates_layout_flags_and_errors() {
        assert_eq!(std::mem::size_of::<AddrInfo>(), 48);
        assert_eq!(std::mem::offset_of!(AddrInfo, address), 32);
        // SAFETY: independent Android LP64 addrinfo and socklen_t layouts;
        // only numeric addresses/services are used, without external DNS/network I/O.
        unsafe {
            let lookup: unsafe extern "C" fn(*const c_char, *const c_char, *const AddrInfo, *mut *mut AddrInfo) -> c_int =
                std::mem::transmute(address(c"getaddrinfo"));
            let free: unsafe extern "C" fn(*mut AddrInfo) = std::mem::transmute(address(c"freeaddrinfo"));
            let reverse: unsafe extern "C" fn(*const c_void, u32, *mut c_char, u32, *mut c_char, u32, c_int) -> c_int =
                std::mem::transmute(address(c"getnameinfo"));
            for (host, family, length) in [(c"127.0.0.1", 2, 16), (c"::1", 10, 28)] {
                let mut hints = AddrInfo { flags: 12, family, kind: 1, protocol: 0, length: 0,
                    canonical: std::ptr::null_mut(), address: std::ptr::null_mut(), next: std::ptr::null_mut() };
                #[repr(C)] struct ResultSlot { before: u64, head: *mut AddrInfo, after: u64 }
                let mut slot = ResultSlot { before: 71, head: std::ptr::null_mut(), after: 82 };
                assert_eq!(lookup(host.as_ptr(), c"4567".as_ptr(), &hints, &mut slot.head), 0);
                assert!(!slot.head.is_null());
                let result = Addresses { head: slot.head, free };
                assert_eq!((slot.before, slot.after), (71, 82));
                let record = &*result.head;
                assert_eq!((record.family, record.kind, record.length), (family, 1, length));
                assert_eq!(record.flags & 12, 12);
                let mut host_text = [0 as c_char; 64];
                let mut service = [0 as c_char; 16];
                // Android NI_NUMERICHOST|NI_NUMERICSERV is 2|8, not GNU's values.
                assert_eq!(reverse(record.address, record.length, host_text.as_mut_ptr(), 64, service.as_mut_ptr(), 16, 10), 0);
                assert_eq!(CStr::from_ptr(host_text.as_ptr()), host);
                assert_eq!(CStr::from_ptr(service.as_ptr()), c"4567");
                assert_eq!(reverse(record.address, record.length, host_text.as_mut_ptr(), 1, service.as_mut_ptr(), 16, 10), 14);
                assert_eq!(reverse(record.address, record.length, host_text.as_mut_ptr(), 64, service.as_mut_ptr(), 16, 32), 3);
                for (flags, family, node, expected) in [(0x200, family, host, 3), (12, 12345, host, 5), (12, family, c"not-a-numeric-address", 8)] {
                    hints.flags = flags;
                    hints.family = family;
                    let mut output = std::ptr::dangling_mut();
                    assert_eq!(lookup(node.as_ptr(), c"4567".as_ptr(), &hints, &mut output), expected);
                    assert!(output.is_null(), "failure must clear its result without transferring ownership");
                }
            }
            free(std::ptr::null_mut());
        }
    }

    #[test]
    fn every_android_pathconf_selector_preserves_native_value_and_errno() {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open("/dev/null").unwrap();
        // Android 15 unistd.h numbers these consecutively from zero; GNU uses
        // a different order. Compare the live service, including unlimited -1.
        let native = [libc::_PC_FILESIZEBITS, libc::_PC_LINK_MAX, libc::_PC_MAX_CANON,
            libc::_PC_MAX_INPUT, libc::_PC_NAME_MAX, libc::_PC_PATH_MAX, libc::_PC_PIPE_BUF,
            libc::_PC_2_SYMLINKS, libc::_PC_ALLOC_SIZE_MIN, libc::_PC_REC_INCR_XFER_SIZE,
            libc::_PC_REC_MAX_XFER_SIZE, libc::_PC_REC_MIN_XFER_SIZE, libc::_PC_REC_XFER_ALIGN,
            libc::_PC_SYMLINK_MAX, libc::_PC_CHOWN_RESTRICTED, libc::_PC_NO_TRUNC,
            libc::_PC_VDISABLE, libc::_PC_ASYNC_IO, libc::_PC_PRIO_IO, libc::_PC_SYNC_IO];
        // SAFETY: exact path/fd scalar prototypes and a live owned descriptor;
        // queries read only and all errno observations belong to this thread.
        unsafe {
            let path: unsafe extern "C" fn(*const c_char, c_int) -> libc::c_long = std::mem::transmute(address(c"pathconf"));
            let fd: unsafe extern "C" fn(c_int, c_int) -> libc::c_long = std::mem::transmute(address(c"fpathconf"));
            for (guest, &host) in native.iter().enumerate() {
                *libc::__errno_location() = 0;
                let expected = libc::pathconf(c"/tmp".as_ptr(), host);
                let errno = *libc::__errno_location();
                *libc::__errno_location() = 0;
                assert_eq!(path(c"/tmp".as_ptr(), guest as c_int), expected, "pathconf {guest}");
                assert_eq!(*libc::__errno_location(), errno);
                *libc::__errno_location() = 0;
                let expected = libc::fpathconf(file.as_raw_fd(), host);
                let errno = *libc::__errno_location();
                *libc::__errno_location() = 0;
                assert_eq!(fd(file.as_raw_fd(), guest as c_int), expected, "fpathconf {guest}");
                assert_eq!(*libc::__errno_location(), errno);
            }
            for selector in [-1, 20, i32::MAX] {
                assert_eq!(path(c"/tmp".as_ptr(), selector), -1);
                assert_eq!(*libc::__errno_location(), libc::EINVAL);
                assert_eq!(fd(file.as_raw_fd(), selector), -1);
                assert_eq!(*libc::__errno_location(), libc::EINVAL);
            }
            assert_eq!(fd(-1, 4), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "Bionic conformance requires Linux ARM64 GNU via docker-rust.sh"]
fn bionic_conformance_requires_the_target_abi() { panic!("run on Linux ARM64 GNU"); }
