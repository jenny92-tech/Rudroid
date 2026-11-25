//! INPUT:  production Bionic symbol source, bounded native buffers and private Linux PTYs
//! OUTPUT:  legacy Unity libc ABI, locale isolation, FORTIFY and kernel-termios regressions
//! POS:    verifies the actual missing import families without forwarding foreign structures
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {
    use std::ffi::{c_char, c_int, c_void, CStr};

    unsafe fn address_for(name: &CStr, library: &CStr, version: &CStr) -> usize {
        // SAFETY: immutable C names and the production library/version contract.
        let value = unsafe { android_bionic::rudroid_bionic_symbol(
            name.as_ptr(), library.as_ptr(), version.as_ptr(), std::ptr::null_mut(),
        ) };
        assert_ne!(value, 0, "missing {}", name.to_string_lossy());
        value
    }

    unsafe fn address(name: &CStr) -> usize {
        unsafe { address_for(name, c"libc.so", c"LIBC") }
    }

    unsafe fn expect_sigabrt<F: FnOnce()>(label: &str, call: F) {
        let pid = libc::fork();
        assert!(pid >= 0);
        if pid == 0 {
            libc::prctl(
                libc::PR_SET_DUMPABLE,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            );
            call();
            libc::_exit(77);
        }
        let mut status = 0;
        assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
        assert!(libc::WIFSIGNALED(status), "{label}: expected SIGABRT");
        assert_eq!(
            libc::WTERMSIG(status),
            libc::SIGABRT,
            "{label}: FORTIFY must abort before a guard-page SIGSEGV"
        );
    }

    #[test]
    fn mapped_directory_enumeration_uses_the_resource_authority_and_closes_native_owners() {
        use std::os::unix::ffi::OsStrExt;
        let root = std::env::temp_dir().join(format!("rudroid-opendir-{}",std::process::id()));
        let dir=root.join("assets/fonts");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Fallback.ttf"),b"test font directory entry").unwrap();
        let uri=std::ffi::CString::new(format!("jar:file:{}!/assets/fonts",root.display())).unwrap();
        let missing=std::ffi::CString::new(format!("jar:file:{}!/assets/missing",root.display())).unwrap();
        let file=std::ffi::CString::new(dir.join("Fallback.ttf").as_os_str().as_bytes()).unwrap();
        let fd_count=||std::fs::read_dir("/proc/self/fd").unwrap().filter_map(Result::ok).filter(|entry|std::fs::read_link(entry.path()).ok().as_ref()==Some(&dir)).count();let before=fd_count();
        // SAFETY: exact ARM64 DIR owner and layout-gated dirent ABI; each live
        // directory is consumed once, and borrowed names are copied before readdir.
        unsafe {
            let open:unsafe extern "C" fn(*const c_char)->*mut libc::DIR=std::mem::transmute(address(c"opendir"));
            let read:unsafe extern "C" fn(*mut libc::DIR)->*mut libc::dirent=std::mem::transmute(address(c"readdir"));
            let close:unsafe extern "C" fn(*mut libc::DIR)->c_int=std::mem::transmute(address(c"closedir"));
            for _ in 0..32 {
                let owner=open(uri.as_ptr());assert!(!owner.is_null());let mut found=false;
                loop {let entry=read(owner);if entry.is_null(){break;}found|=CStr::from_ptr((*entry).d_name.as_ptr())==c"Fallback.ttf";}
                assert!(found);assert_eq!(close(owner),0);
            }
            assert!(open(missing.as_ptr()).is_null());assert_eq!(*libc::__errno_location(),libc::ENOENT);
            assert!(open(file.as_ptr()).is_null());assert_eq!(*libc::__errno_location(),libc::ENOTDIR);
        }
        assert_eq!(fd_count(),before);std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_file_aliases_keep_paths_layouts_offsets_and_native_errno() {
        use std::os::unix::ffi::OsStrExt;
        let root = std::env::temp_dir().join(format!("rudroid-stat64-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/file"), b"seven77").unwrap();
        std::os::unix::fs::symlink("assets/file", root.join("link")).unwrap();
        let uri = std::ffi::CString::new(format!("jar:file:{}!/assets/file", root.display())).unwrap();
        let link = std::ffi::CString::new(root.join("link").as_os_str().as_bytes()).unwrap();
        let file = std::ffi::CString::new(root.join("assets/file").as_os_str().as_bytes()).unwrap();
        // SAFETY: exact LP64 prototypes with bounded stat out buffers; the C provider
        // gates every public offset/width against AndroidStat before exposing these aliases.
        unsafe {
            for (alias, base) in [(c"stat64", c"stat"), (c"lstat64", c"lstat"), (c"fstat64", c"fstat"), (c"fstatat64", c"fstatat"),
                (c"open64", c"open"), (c"openat64", c"openat"), (c"pread64", c"pread"),
                (c"pwrite64", c"pwrite"), (c"ftruncate64", c"ftruncate")] {
                assert_eq!(address(alias), address(base));
            }
            type Stat = unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int;
            let stat: Stat = std::mem::transmute(address(c"stat64"));
            let lstat: Stat = std::mem::transmute(address(c"lstat64"));
            let fstat: unsafe extern "C" fn(c_int, *mut libc::stat) -> c_int = std::mem::transmute(address(c"fstat64"));
            let at: unsafe extern "C" fn(c_int, *const c_char, *mut libc::stat, c_int) -> c_int = std::mem::transmute(address(c"fstatat64"));
            #[repr(C)] struct Guard { before: u64, stat: libc::stat, after: u64 }
            let mut out = Guard { before: 17, stat: std::mem::zeroed(), after: 91 };
            assert_eq!(stat(uri.as_ptr(), &mut out.stat), 0);
            assert_eq!(out.stat.st_size, 7);
            assert_eq!(stat(link.as_ptr(), &mut out.stat), 0);
            assert_eq!(out.stat.st_mode & libc::S_IFMT, libc::S_IFREG);
            assert_eq!(lstat(link.as_ptr(), &mut out.stat), 0);
            assert_eq!(out.stat.st_mode & libc::S_IFMT, libc::S_IFLNK);
            let fd = libc::open(file.as_ptr(), libc::O_RDONLY);
            assert!(fd >= 0);
            assert_eq!(fstat(fd, &mut out.stat), 0); assert_eq!(out.stat.st_size, 7);
            assert_eq!(libc::close(fd), 0);
            assert_eq!(at(libc::AT_FDCWD, link.as_ptr(), &mut out.stat, libc::AT_SYMLINK_NOFOLLOW), 0);
            assert_eq!(out.stat.st_mode & libc::S_IFMT, libc::S_IFLNK);
            assert_eq!(fstat(-1, &mut out.stat), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
            assert_eq!((out.before, out.after), (17, 91));
            let open: unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int = std::mem::transmute(address(c"open64"));
            let openat: unsafe extern "C" fn(c_int, *const c_char, c_int, ...) -> c_int = std::mem::transmute(address(c"openat64"));
            let pread: unsafe extern "C" fn(c_int, *mut c_void, usize, i64) -> isize = std::mem::transmute(address(c"pread64"));
            let pwrite: unsafe extern "C" fn(c_int, *const c_void, usize, i64) -> isize = std::mem::transmute(address(c"pwrite64"));
            let truncate: unsafe extern "C" fn(c_int, i64) -> c_int = std::mem::transmute(address(c"ftruncate64"));
            use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
            let fd = open(uri.as_ptr(), libc::O_RDWR);
            assert!(fd >= 0, "open64 must retain the asset URI adapter");
            let sparse = OwnedFd::from_raw_fd(fd);
            // A mapped absolute asset path ignores an otherwise invalid dirfd.
            let fd = openat(-1, uri.as_ptr(), libc::O_RDONLY);
            assert!(fd >= 0, "openat64 must map the URI before native I/O");
            let readable = OwnedFd::from_raw_fd(fd);
            let mut prefix = [0u8; 7];
            assert_eq!(pread(readable.as_raw_fd(), prefix.as_mut_ptr().cast(), 7, 0), 7);
            assert_eq!(&prefix, b"seven77");
            let offset = 0x1_0000_0000_i64;
            assert_eq!(truncate(sparse.as_raw_fd(), offset + 8), 0);
            assert_eq!(pwrite(sparse.as_raw_fd(), b"sparse64".as_ptr().cast(), 8, offset), 8);
            assert_eq!(fstat(sparse.as_raw_fd(), &mut out.stat), 0);
            assert_eq!(out.stat.st_size, offset + 8);
            let mut buffer = [91u8; 10];
            assert_eq!(pread(readable.as_raw_fd(), buffer.as_mut_ptr().add(1).cast(), 8, offset), 8);
            assert_eq!(&buffer, b"[sparse64[");
            assert_eq!(libc::lseek(sparse.as_raw_fd(), 0, libc::SEEK_CUR), 0);
            assert_eq!(pread(-1, buffer.as_mut_ptr().cast(), 1, 0), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
            assert_eq!(pwrite(-1, buffer.as_ptr().cast(), 1, 0), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
            assert_eq!(truncate(-1, 0), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
            assert_eq!(pread(readable.as_raw_fd(), buffer.as_mut_ptr().cast(), 1, -1), -1);
            assert_eq!(*libc::__errno_location(), libc::EINVAL);
            let map: unsafe extern "C" fn(*mut c_void, usize, c_int, c_int, c_int, i64) -> *mut c_void = std::mem::transmute(address(c"mmap64"));
            assert_eq!(address(c"mmap64"), address(c"mmap"));
            let mapped = map(std::ptr::null_mut(), 4096, libc::PROT_READ, libc::MAP_PRIVATE, sparse.as_raw_fd(), offset);
            assert_ne!(mapped, libc::MAP_FAILED);
            assert_eq!(std::slice::from_raw_parts(mapped.cast::<u8>(), 8), b"sparse64");
            assert_eq!(libc::munmap(mapped, 4096), 0);
            assert_eq!(map(std::ptr::null_mut(), 4096, libc::PROT_READ, libc::MAP_PRIVATE, -1, offset), libc::MAP_FAILED);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_classification_and_collation_keep_the_host_locale_unchanged() {
        // SAFETY: exact C scalar/string prototypes; all spans and locale
        // tokens remain live until their last call and the token is freed once.
        unsafe {
            let host_locale = libc::uselocale(std::ptr::null_mut());
            type Classify = unsafe extern "C" fn(c_int) -> c_int;
            type WithLocale = unsafe extern "C" fn(c_int, *mut c_void) -> c_int;
            let create: unsafe extern "C" fn(c_int, *const c_char, *mut c_void) -> *mut c_void =
                std::mem::transmute(address(c"newlocale"));
            let free: unsafe extern "C" fn(*mut c_void) = std::mem::transmute(address(c"freelocale"));
            let token = create(libc::LC_CTYPE_MASK, c"C.UTF-8".as_ptr(), std::ptr::null_mut());
            assert!(!token.is_null());
            for (name, localized, expected) in [
                (c"isalpha", c"isalpha_l", (u8::is_ascii_alphabetic as fn(&u8) -> bool)),
                (c"isalnum", c"isalnum_l", u8::is_ascii_alphanumeric),
                (c"isdigit", c"isdigit_l", u8::is_ascii_digit),
                (c"isxdigit", c"isxdigit_l", u8::is_ascii_hexdigit),
                (c"islower", c"islower_l", u8::is_ascii_lowercase),
                (c"isupper", c"isupper_l", u8::is_ascii_uppercase),
                (c"isgraph", c"isgraph_l", u8::is_ascii_graphic),
                (c"iscntrl", c"iscntrl_l", u8::is_ascii_control),
                (c"ispunct", c"ispunct_l", u8::is_ascii_punctuation),
            ] {
                let plain: Classify = std::mem::transmute(address(name));
                let local: WithLocale = std::mem::transmute(address(localized));
                for value in 0..=255 {
                    assert_eq!(plain(value) != 0, expected(&(value as u8)), "{} {value}", name.to_string_lossy());
                    assert_eq!(local(value, token) != 0, plain(value) != 0);
                }
                assert_eq!(plain(-1), 0);
                assert_eq!(plain(256), 0);
            }
            let print: Classify = std::mem::transmute(address(c"isprint"));
            let lower: Classify = std::mem::transmute(address(c"tolower"));
            let upper: WithLocale = std::mem::transmute(address(c"toupper_l"));
            for value in -1..=256 {
                assert_eq!(print(value) != 0, (32..=126).contains(&value));
                assert_eq!(lower(value), if (65..=90).contains(&value) { value + 32 } else { value });
                assert_eq!(upper(value, token), if (97..=122).contains(&value) { value - 32 } else { value });
            }
            let collate: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int = std::mem::transmute(address(c"strcoll"));
            assert!(collate(c"Z".as_ptr(), c"a".as_ptr()) < 0);
            let transform: unsafe extern "C" fn(*mut c_char, *const c_char, usize) -> usize = std::mem::transmute(address(c"strxfrm"));
            assert_eq!(transform(std::ptr::null_mut(), c"abc".as_ptr(), 0), 3);
            let mut bytes = [0x5b_u8; 8];
            assert_eq!(transform(bytes.as_mut_ptr().add(1).cast(), c"abc".as_ptr(), 4), 3);
            assert_eq!(&bytes, b"[abc\0[[[");
            bytes.fill(0x5b);
            assert_eq!(transform(bytes.as_mut_ptr().add(1).cast(), c"abc".as_ptr(), 2), 3);
            assert_eq!(bytes[0], 0x5b);
            assert_eq!(&bytes[3..], &[0x5b; 5]);
            free(token);
            assert_eq!(libc::uselocale(std::ptr::null_mut()), host_locale);
        }
    }

    #[test]
    fn legacy_ctype_symbol_is_a_pointer_to_the_aosp_compatibility_table() {
        // SAFETY: the public query returns the address of the data symbol;
        // its pointed-to table is the fixed 257-byte AOSP-compatible object.
        unsafe {
            let symbol = address_for(c"_ctype_", c"libc.so", c"LIBC");
            let table = *(symbol as *const *const c_char);
            assert!(!table.is_null());
            let bytes = std::slice::from_raw_parts(table.cast::<u8>(), 257);
            assert_eq!(bytes[0], 0);
            // The table reserves index 0 for EOF; byte c is at c + 1.
            assert_eq!(bytes[b'0' as usize + 1], 0x04);
            assert_eq!(bytes[b'A' as usize + 1], 0x41);
            assert_eq!(bytes[b'Z' as usize + 1], 0x01);
            assert_eq!(bytes[b'a' as usize + 1], 0x42);
            assert_eq!(bytes[b'z' as usize + 1], 0x02);
            assert_eq!(bytes[b' ' as usize + 1], 0x88);
            assert_eq!(bytes[b'\n' as usize + 1], 0x28);
            assert_eq!(bytes[b'_' as usize + 1], 0x10);
            assert_eq!(bytes[0x7f + 1], 0x20);
            assert!(bytes[0x80 + 1..].iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn xlua_standard_imports_keep_bionic_abs_network_and_freopen_semantics() {
        use std::os::unix::ffi::OsStrExt;
        let root = std::env::temp_dir().join(format!("rudroid-xlua7-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("alpha"), b"alpha").unwrap();
        std::fs::write(root.join("beta"), b"beta").unwrap();
        std::fs::write(root.join("assets/asset"), b"asset").unwrap();
        let c_path = |path: &std::path::Path| {
            std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap()
        };
        let alpha = c_path(&root.join("alpha"));
        let beta = c_path(&root.join("beta"));
        let append = c_path(&root.join("append"));
        let missing = c_path(&root.join("missing"));
        let asset = std::ffi::CString::new(format!(
            "jar:file:{}!/assets/asset",
            root.display()
        ))
        .unwrap();
        // SAFETY: exact C scalar/string prototypes and bounded buffers. Every
        // stream is consumed once; the failed freopen consumes its own facade.
        unsafe {
            let abs: unsafe extern "C" fn(c_int) -> c_int = std::mem::transmute(address(c"abs"));
            assert_eq!((abs(0), abs(7), abs(-7)), (0, 7, 7));

            #[repr(C)]
            struct InAddr { s_addr: u32 }
            let inet_aton: unsafe extern "C" fn(*const c_char, *mut InAddr) -> c_int =
                std::mem::transmute(address(c"inet_aton"));
            for (text, expected) in [
                ("127.0.0.1", [127, 0, 0, 1]),
                ("127.1", [127, 0, 0, 1]),
                ("0177.1", [127, 0, 0, 1]),
                ("0x7f000001", [127, 0, 0, 1]),
                ("0", [0, 0, 0, 0]),
            ] {
                let text = std::ffi::CString::new(text).unwrap();
                let mut address = InAddr { s_addr: 0 };
                assert_eq!(inet_aton(text.as_ptr(), &mut address), 1, "{text:?}");
                assert_eq!(address.s_addr.to_ne_bytes(), expected, "{text:?}");
            }
            for text in ["", "256.1.1.1", "1.2.3.4.5"] {
                let text = std::ffi::CString::new(text).unwrap();
                let mut address = InAddr { s_addr: 0xdead_beef };
                assert_eq!(inet_aton(text.as_ptr(), &mut address), 0, "{text:?}");
                assert_eq!(address.s_addr, 0xdead_beef, "invalid input must not write");
            }

            let hstrerror: unsafe extern "C" fn(c_int) -> *const c_char =
                std::mem::transmute(address(c"hstrerror"));
            let get_h_errno: unsafe extern "C" fn() -> *mut c_int =
                std::mem::transmute(address(c"__get_h_errno"));
            let h_errno = get_h_errno();
            *h_errno = 77;
            *libc::__errno_location() = 1234;
            for (code, expected) in [
                (-1, b"Resolver internal error".as_slice()),
                (0, b"Resolver Error 0 (no error)".as_slice()),
                (1, b"Unknown host".as_slice()),
                (2, b"Host name lookup failure".as_slice()),
                (3, b"Unknown server error".as_slice()),
                (4, b"No address associated with name".as_slice()),
                (5, b"Unknown resolver error".as_slice()),
                (99, b"Unknown resolver error".as_slice()),
            ] {
                assert_eq!(CStr::from_ptr(hstrerror(code)).to_bytes(), expected, "hstrerror({code})");
                assert_eq!(*libc::__errno_location(), 1234, "hstrerror must not change errno");
                assert_eq!(*h_errno, 77, "hstrerror must not change h_errno");
            }

            let open: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void =
                std::mem::transmute(address(c"fopen"));
            let reopen: unsafe extern "C" fn(*const c_char, *const c_char, *mut c_void) -> *mut c_void =
                std::mem::transmute(address(c"freopen"));
            let read: unsafe extern "C" fn(*mut c_void, usize, usize, *mut c_void) -> usize =
                std::mem::transmute(address(c"fread"));
            let close: unsafe extern "C" fn(*mut c_void) -> c_int =
                std::mem::transmute(address(c"fclose"));
            let mut bytes = [0_u8; 8];
            let stream = open(alpha.as_ptr(), c"r".as_ptr());
            assert!(!stream.is_null());
            assert_eq!(read(bytes.as_mut_ptr().cast(), 1, 5, stream), 5);
            assert_eq!(&bytes[..5], b"alpha");
            assert_eq!(reopen(beta.as_ptr(), c"r".as_ptr(), stream), stream);
            assert_eq!(read(bytes.as_mut_ptr().cast(), 1, 4, stream), 4);
            assert_eq!(&bytes[..4], b"beta");
            assert_eq!(reopen(asset.as_ptr(), c"r".as_ptr(), stream), stream);
            assert_eq!(read(bytes.as_mut_ptr().cast(), 1, 5, stream), 5);
            assert_eq!(&bytes[..5], b"asset");
            assert_eq!(close(stream), 0);

            let writable = open(append.as_ptr(), c"w".as_ptr());
            assert!(!writable.is_null());
            assert_eq!(reopen(std::ptr::null(), c"a".as_ptr(), writable), writable,
                "a null filename must change only the mode on the same facade");
            assert_eq!(close(writable), 0);

            let failed = open(alpha.as_ptr(), c"r".as_ptr());
            assert!(!failed.is_null());
            *libc::__errno_location() = 0;
            assert!(reopen(missing.as_ptr(), c"r".as_ptr(), failed).is_null());
            assert_eq!(*libc::__errno_location(), libc::ENOENT,
                "freopen must preserve the native failure errno");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tmpnam_honors_a_long_tmpdir_and_normalizes_native_failure() {
        let root = std::env::temp_dir().join(format!(
            "rudroid-tmpnam-parent-{}-long-enough-for-android-l_tmpnam",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "linux::tmpnam_child_honors_4096_contract",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("TMPDIR", &root)
            .output()
            .unwrap();
        let child_output = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success()
                && child_output.contains(
                    "test linux::tmpnam_child_honors_4096_contract ... ok"
                )
                && child_output.contains(
                    "test result: ok. 1 passed; 0 failed; 0 ignored;"
                ),
            "{}",
            child_output
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "spawned by tmpnam_honors_a_long_tmpdir_and_normalizes_native_failure"]
    fn tmpnam_child_honors_4096_contract() {
        let tmpdir = std::env::var_os("TMPDIR").expect("parent supplies the long TMPDIR");
        let tmpdir = tmpdir.to_str().unwrap();
        assert!(tmpdir.len() > 32, "fixture must exceed the old fixed 32-byte path");
        #[repr(C)]
        struct Guard { before: u64, buffer: [u8; 4096], after: u64 }
        // SAFETY: exact tmpnam caller-identity ABI and a 4096-byte Android
        // L_tmpnam buffer surrounded by canaries. The child owns TMPDIR.
        unsafe {
            let tmpnam: unsafe extern "C" fn(*mut c_char) -> *mut c_char =
                std::mem::transmute(address(c"tmpnam"));
            let mut guard = Guard { before: 0x1122_3344_5566_7788, buffer: [0x5a; 4096], after: 0x8877_6655_4433_2211 };
            let first_ptr = tmpnam(guard.buffer.as_mut_ptr().cast());
            assert_eq!(first_ptr, guard.buffer.as_mut_ptr().cast(), "must return the caller buffer");
            assert_eq!((guard.before, guard.after), (0x1122_3344_5566_7788, 0x8877_6655_4433_2211));
            let first = CStr::from_ptr(first_ptr).to_bytes().to_vec();
            assert!(first.starts_with(tmpdir.as_bytes()));
            let first_text = std::str::from_utf8(&first).unwrap();
            assert!(matches!(std::fs::symlink_metadata(first_text), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "tmpnam must return a nonexistent name without creating it");

            let second_ptr = tmpnam(guard.buffer.as_mut_ptr().cast());
            assert_eq!(second_ptr, first_ptr);
            let second = CStr::from_ptr(second_ptr).to_bytes().to_vec();
            assert!(second.starts_with(tmpdir.as_bytes()));
            assert_ne!(first, second, "successive names must differ");
            let second_text = std::str::from_utf8(&second).unwrap();
            assert!(matches!(std::fs::symlink_metadata(second_text), Err(error) if error.kind() == std::io::ErrorKind::NotFound));

            let static_first = tmpnam(std::ptr::null_mut());
            assert!(!static_first.is_null());
            let first_static = CStr::from_ptr(static_first).to_bytes().to_vec();
            let static_second = tmpnam(std::ptr::null_mut());
            assert_eq!(static_first, static_second, "null caller buffer uses one process-static object");
            assert_ne!(first_static.as_slice(), CStr::from_ptr(static_second).to_bytes());
            assert!(matches!(std::fs::symlink_metadata(std::str::from_utf8(&first_static).unwrap()), Err(error) if error.kind() == std::io::ErrorKind::NotFound));

            let loop_path = format!("{tmpdir}/loop");
            std::os::unix::fs::symlink("loop", &loop_path).unwrap();
            let loop_c = std::ffi::CString::new(loop_path).unwrap();
            assert_eq!(libc::setenv(c"TMPDIR".as_ptr(), loop_c.as_ptr(), 1), 0);
            let mut failure_buffer = [0x5a_u8; 4096];
            *libc::__errno_location() = 0;
            assert!(tmpnam(failure_buffer.as_mut_ptr().cast()).is_null(),
                "native mktemp failure must normalize to NULL");
            assert_eq!(*libc::__errno_location(), libc::ELOOP,
                "the native failure errno must survive normalization");
        }
    }

    #[test]
    fn wide_transforms_and_overlapping_moves_preserve_buffer_canaries() {
        // SAFETY: Linux/Android ARM64 wchar_t is 32 bits; disjoint copy/fill
        // ranges and the explicit memmove overlap remain within these arrays.
        unsafe {
            type Transform = unsafe extern "C" fn(*mut i32, *const i32, usize) -> usize;
            type Copy = unsafe extern "C" fn(*mut i32, *const i32, usize) -> *mut i32;
            let transform: Transform = std::mem::transmute(address(c"wcsxfrm"));
            let collate: unsafe extern "C" fn(*const i32, *const i32) -> c_int = std::mem::transmute(address(c"wcscoll"));
            let copy: Copy = std::mem::transmute(address(c"wmemcpy"));
            let move_words: Copy = std::mem::transmute(address(c"wmemmove"));
            let fill: unsafe extern "C" fn(*mut i32, i32, usize) -> *mut i32 = std::mem::transmute(address(c"wmemset"));
            let source = [0x41_i32, 0x4e2d, 0];
            let mut out = [91_i32; 8];
            assert_eq!(transform(std::ptr::null_mut(), source.as_ptr(), 0), 2);
            assert_eq!(transform(out.as_mut_ptr().add(1), source.as_ptr(), 3), 2);
            assert_eq!(&out, &[91, 0x41, 0x4e2d, 0, 91, 91, 91, 91]);
            assert_eq!(collate(source.as_ptr(), out.as_ptr().add(1)), 0);
            assert_eq!(copy(out.as_mut_ptr().add(1), source.as_ptr(), 3), out.as_mut_ptr().add(1));
            assert_eq!(move_words(out.as_mut_ptr().add(2), out.as_ptr().add(1), 3), out.as_mut_ptr().add(2));
            assert_eq!(&out, &[91, 0x41, 0x41, 0x4e2d, 0, 91, 91, 91]);
            assert_eq!(fill(out.as_mut_ptr().add(1), 0x20, 3), out.as_mut_ptr().add(1));
            assert_eq!(&out, &[91, 0x20, 0x20, 0x20, 0, 91, 91, 91]);
        }
    }

    #[test]
    fn fortify_keeps_bounds_and_android_variadic_formatting() {
        // SAFETY: exact AAPCS64 scalar/variadic prototypes and bounded spans.
        // Invalid operations run in children, with core dumps disabled.
        unsafe {
            let length: unsafe extern "C" fn(*const c_char, usize) -> usize = std::mem::transmute(address(c"__strlen_chk"));
            let format: unsafe extern "C" fn(*mut c_char, usize, c_int, usize, *const c_char, ...) -> c_int = std::mem::transmute(address(c"__snprintf_chk"));
            assert_ne!(address(c"__vsnprintf_chk"), 0);
            assert_eq!(length(c"abc".as_ptr(), 4), 3);
            assert_eq!(length(c"abc".as_ptr(), usize::MAX), 3);
            let mut out = [91_u8; 8];
            assert_eq!(format(out.as_mut_ptr().add(1).cast(), 4, 1, 4, c"%d-%s".as_ptr(), 42_i32, c"xy".as_ptr()), 5);
            assert_eq!(&out, b"[42-\0[[[");
            assert_eq!(format(std::ptr::null_mut(), 0, 0, 0, c"%d".as_ptr(), 123_i32), 3);
            for mode in 0..3 {
                let pid = libc::fork();
                assert!(pid >= 0);
                if pid == 0 {
                    libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong);
                    if mode == 0 { length(c"abc".as_ptr(), 3); }
                    else if mode == 1 { format(out.as_mut_ptr().cast(), 8, 0, 4, c"%d".as_ptr(), 1_i32); }
                    else {
                        let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
                        let p = libc::mmap(std::ptr::null_mut(), page * 2, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
                        if p == libc::MAP_FAILED { libc::_exit(78); }
                        if libc::mprotect(p.add(page), page, libc::PROT_NONE) != 0 { libc::_exit(79); }
                        let end = p.cast::<u8>().add(page - 4); std::ptr::write_bytes(end, 0x41, 4);
                        length(end.cast(), 4);
                    }
                    libc::_exit(77);
                }
                let mut status = 0;
                assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
                assert!(libc::WIFSIGNALED(status));
                assert_eq!(libc::WTERMSIG(status), libc::SIGABRT, "mode={mode}; guard-page access must not SIGSEGV");
            }
        }
    }

    #[test]
    fn fortify_string_io_and_formatting_reject_exact_boundaries() {
        // SAFETY: exact LP64 prototypes, guard-page-backed bounded spans and
        // isolated fork children. Every invalid call is expected to abort
        // before reading or writing the PROT_NONE page.
        unsafe {
            type Strchr = unsafe extern "C" fn(*const c_char, c_int, usize) -> *mut c_char;
            type Read = unsafe extern "C" fn(c_int, *mut c_void, usize, usize) -> isize;
            type Sprintf = unsafe extern "C" fn(*mut c_char, c_int, usize, *const c_char, ...) -> c_int;
            let strchr: Strchr = std::mem::transmute(address(c"__strchr_chk"));
            let read: Read = std::mem::transmute(address(c"__read_chk"));
            let sprintf: Sprintf = std::mem::transmute(address(c"__sprintf_chk"));

            let unterminated = [b'x', b'y'];
            assert_eq!(
                strchr(unterminated.as_ptr().cast(), i32::from(b'x'), 2),
                unterminated.as_ptr().cast_mut().cast()
            );
            let terminated = [b'x', 0, b'y'];
            assert!(strchr(terminated.as_ptr().cast(), i32::from(b'y'), 3).is_null());

            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            assert!(page >= 2);
            let mapping = libc::mmap(
                std::ptr::null_mut(), page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0,
            );
            assert_ne!(mapping, libc::MAP_FAILED);
            assert_eq!(
                libc::mprotect(mapping.cast::<u8>().add(page).cast(), page, libc::PROT_NONE),
                0
            );
            let edge = mapping.cast::<u8>().add(page - 2);
            edge.write(b'x');
            edge.add(1).write(b'y');
            assert_eq!(strchr(edge.cast(), i32::from(b'x'), 2), edge.cast());

            let mut fds = [0; 2];
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            assert_eq!(libc::write(fds[1], b"ok".as_ptr().cast(), 2), 2);
            assert_eq!(read(fds[0], edge.cast(), 2, 2), 2);
            assert_eq!(std::slice::from_raw_parts(edge, 2), b"ok");
            assert_eq!(libc::close(fds[0]), 0);
            assert_eq!(libc::close(fds[1]), 0);

            let mut out = [0x5b_u8; 8];
            *libc::__errno_location() = 0;
            assert_eq!(sprintf(out.as_mut_ptr().cast(), 0, out.len(), c"%".as_ptr()), -1);
            assert_eq!(*libc::__errno_location(), libc::EINVAL);

            for mode in 0..4 {
                let pid = libc::fork();
                assert!(pid >= 0);
                if pid == 0 {
                    libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong);
                    match mode {
                        0 => { strchr(edge.cast(), i32::from(b'z'), 2); }
                        1 => { read(-1, edge.cast(), 2, 1); }
                        2 => { read(-1, edge.cast(), isize::MAX as usize + 1, usize::MAX); }
                        _ => { sprintf(out.as_mut_ptr().cast(), 0, 4, c"abcd".as_ptr()); }
                    }
                    libc::_exit(77);
                }
                let mut status = 0;
                assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
                assert!(libc::WIFSIGNALED(status), "mode={mode}");
                assert_eq!(libc::WTERMSIG(status), libc::SIGABRT, "mode={mode}");
            }
            assert_eq!(libc::munmap(mapping, page * 2), 0);
        }
    }

    #[test]
    fn fortify_strncpy_chk2_checks_each_source_read_and_pads_original_count() {
        // SAFETY: exact LP64 prototype. The successful call has a four-byte
        // destination guard and a two-byte source object. The failing child
        // places the source immediately before PROT_NONE, so reading index 2
        // would fault unless FORTIFY aborts first.
        unsafe {
            type StrncpyChk2 = unsafe extern "C" fn(
                *mut c_char,
                *const c_char,
                usize,
                usize,
                usize,
            ) -> *mut c_char;
            let copy: StrncpyChk2 = std::mem::transmute(address(c"__strncpy_chk2"));

            let source = [b'a', 0];
            let mut destination = [0x5b_u8; 6];
            let result = copy(
                destination.as_mut_ptr().add(1).cast(),
                source.as_ptr().cast(),
                4,
                4,
                2,
            );
            assert_eq!(result, destination.as_mut_ptr().add(1).cast());
            assert_eq!(&destination, &[0x5b, b'a', 0, 0, 0, 0x5b]);

            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            assert!(page >= 2);
            let mapping = libc::mmap(
                std::ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(mapping, libc::MAP_FAILED);
            assert_eq!(
                libc::mprotect(
                    mapping.cast::<u8>().add(page).cast(),
                    page,
                    libc::PROT_NONE,
                ),
                0
            );
            let edge = mapping.cast::<u8>().add(page - 2);
            edge.write(b'a');
            edge.add(1).write(b'b');
            let mut out = [0_u8; 4];
            expect_sigabrt("strncpy_chk2 source object boundary", || {
                copy(out.as_mut_ptr().cast(), edge.cast(), 4, out.len(), 2);
            });
            assert_eq!(libc::munmap(mapping, page * 2), 0);
        }
    }

    #[test]
    fn fortify_strrchr_requires_nul_within_the_declared_capacity() {
        // SAFETY: exact prototype, a terminated in-bounds success and
        // isolated capacity-exhaustion cases, including capacity zero at a
        // PROT_NONE boundary.
        unsafe {
            type StrrchrChk = unsafe extern "C" fn(*const c_char, c_int, usize) -> *mut c_char;
            let search: StrrchrChk = std::mem::transmute(address(c"__strrchr_chk"));
            let terminated = [b'a', b'b', 0];
            assert_eq!(
                search(terminated.as_ptr().cast(), i32::from(b'b'), terminated.len()),
                terminated.as_ptr().add(1).cast_mut().cast()
            );

            let unterminated = [b'a', b'b', b'c'];
            expect_sigabrt("strrchr_chk unterminated capacity", || {
                search(unterminated.as_ptr().cast(), i32::from(b'a'), unterminated.len());
            });

            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let mapping = libc::mmap(
                std::ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(mapping, libc::MAP_FAILED);
            assert_eq!(
                libc::mprotect(
                    mapping.cast::<u8>().add(page).cast(),
                    page,
                    libc::PROT_NONE,
                ),
                0
            );
            let edge = mapping.cast::<u8>().add(page - 2);
            edge.write(b'x');
            edge.add(1).write(b'y');
            expect_sigabrt("strrchr_chk guard boundary", || {
                search(edge.cast(), i32::from(b'x'), 2);
            });
            expect_sigabrt("strrchr_chk zero capacity", || {
                search(edge.cast(), i32::from(b'x'), 0);
            });
            assert_eq!(libc::munmap(mapping, page * 2), 0);
        }
    }

    #[test]
    fn fortify_string_append_bounds_source_scan_by_remaining_room() {
        // SAFETY: exact prototypes; invalid children place the source at a
        // guard-page edge, while the valid bounded strncat case reads only
        // the two accessible bytes.
        unsafe {
            type StrncatChk = unsafe extern "C" fn(
                *mut c_char,
                *const c_char,
                usize,
                usize,
            ) -> *mut c_char;
            type StrcatChk = unsafe extern "C" fn(*mut c_char, *const c_char, usize) -> *mut c_char;
            let append_n: StrncatChk = std::mem::transmute(address(c"__strncat_chk"));
            let append: StrcatChk = std::mem::transmute(address(c"__strcat_chk"));

            let unreadable = 1_usize as *mut c_char;
            assert_eq!(append_n(unreadable, unreadable, 0, 0), unreadable);

            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let mapping = libc::mmap(
                std::ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(mapping, libc::MAP_FAILED);
            assert_eq!(
                libc::mprotect(
                    mapping.cast::<u8>().add(page).cast(),
                    page,
                    libc::PROT_NONE,
                ),
                0
            );
            let edge = mapping.cast::<u8>().add(page - 2);
            edge.write(b'x');
            edge.add(1).write(b'y');

            let mut full = [0_u8; 2];
            expect_sigabrt("strcat_chk source cannot fit", || {
                append(full.as_mut_ptr().cast(), edge.cast(), full.len());
            });
            expect_sigabrt("strncat_chk source cannot fit", || {
                append_n(full.as_mut_ptr().cast(), edge.cast(), 2, full.len());
            });

            let mut bounded = [0_u8; 4];
            assert_eq!(
                append_n(bounded.as_mut_ptr().cast(), edge.cast(), 2, bounded.len()),
                bounded.as_mut_ptr().cast()
            );
            assert_eq!(&bounded, b"xy\0\0");
            assert_eq!(libc::munmap(mapping, page * 2), 0);
        }
    }

    #[test]
    fn fortify_fgets_rejects_nonpositive_size_before_file_conversion() {
        // SAFETY: exact prototype. Guest address 1 is intentionally not a
        // facade; Android 15 rejects size <= 0 before normalizing the FILE.
        unsafe {
            type FgetsChk = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void, usize) -> *mut c_char;
            let read: FgetsChk = std::mem::transmute(address(c"__fgets_chk"));
            let mut out = [0_u8; 4];
            expect_sigabrt("fgets_chk zero size", || {
                read(out.as_mut_ptr().cast(), 0, 1_usize as *mut c_void, out.len());
            });
            expect_sigabrt("fgets_chk negative size", || {
                read(out.as_mut_ptr().cast(), -1, 1_usize as *mut c_void, out.len());
            });
        }
    }

    #[test]
    fn inotify_add_watch_maps_extracted_asset_paths_and_reports_events() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rudroid-inotify-{}-{nonce}",
            std::process::id()
        ));
        let watched = root.join("assets/watched.txt");
        std::fs::create_dir_all(watched.parent().unwrap()).unwrap();
        std::fs::write(&watched, b"before").unwrap();
        let uri = std::ffi::CString::new(format!(
            "jar:file:{}!/assets/watched.txt",
            root.as_os_str().to_string_lossy()
        ))
        .unwrap();
        // SAFETY: exact Linux inotify prototypes; fd/event buffers are live
        // and belong only to this test.
        unsafe {
            type Init = unsafe extern "C" fn() -> c_int;
            type AddWatch = unsafe extern "C" fn(c_int, *const c_char, u32) -> c_int;
            let init: Init = std::mem::transmute(address(c"inotify_init"));
            let add: AddWatch = std::mem::transmute(address(c"inotify_add_watch"));
            let fd = init();
            assert!(fd >= 0, "{}", std::io::Error::last_os_error());
            let watch = add(fd, uri.as_ptr(), libc::IN_MODIFY | libc::IN_CLOSE_WRITE);
            assert!(watch >= 0, "{}", std::io::Error::last_os_error());

            std::fs::write(&watched, b"after").unwrap();
            let mut descriptor = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(libc::poll(&mut descriptor, 1, 3000), 1);
            let mut events = [0_u8; 1024];
            let count = libc::read(fd, events.as_mut_ptr().cast(), events.len());
            assert!(count > 0, "{}", std::io::Error::last_os_error());
            let event = std::ptr::read_unaligned(events.as_ptr().cast::<libc::inotify_event>());
            assert_ne!(event.mask & (libc::IN_MODIFY | libc::IN_CLOSE_WRITE), 0);
            assert_eq!(event.wd, watch);
            assert_eq!(libc::close(fd), 0);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pclose_preserves_wait_failure_errno_after_releasing_the_facade() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rudroid-pclose-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let pid_path = root.join("pid");
        let command = std::ffi::CString::new(format!(
            "printf '%s\\n' \"$$\" > '{}'",
            pid_path.display()
        ))
        .unwrap();
        // SAFETY: exact popen/pclose ABI. The shell records the exact child
        // pid; this test reaps it so pclose must report ECHILD, then checks
        // that facade deallocation does not overwrite that failure.
        unsafe {
            type Popen = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void;
            type Pclose = unsafe extern "C" fn(*mut c_void) -> c_int;
            let open: Popen = std::mem::transmute(address(c"popen"));
            let close: Pclose = std::mem::transmute(address(c"pclose"));
            let file = open(command.as_ptr(), c"r".as_ptr());
            assert!(!file.is_null(), "{}", std::io::Error::last_os_error());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let pid = loop {
                if let Ok(text) = std::fs::read_to_string(&pid_path) {
                    if let Some(line) = text.strip_suffix('\n') {
                        if !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit()) {
                            if let Ok(pid) = line.parse::<libc::pid_t>() {
                                if pid > 0 { break pid; }
                            }
                        }
                    }
                }
                assert!(std::time::Instant::now() < deadline, "popen shell did not publish a complete pid");
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
            assert!(libc::WIFEXITED(status));
            *libc::__errno_location() = 0;
            assert_eq!(close(file), -1);
            assert_eq!(*libc::__errno_location(), libc::ECHILD);

            let healthy = open(c"true".as_ptr(), c"r".as_ptr());
            assert!(!healthy.is_null());
            assert_eq!(close(healthy), 0);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminal_api_uses_the_android_kernel_layout_and_private_pty() {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Termios { flags: [u32; 4], line: u8, cc: [u8; 19] }
        #[repr(C)]
        struct Guarded { before: u32, value: Termios, after: u32 }
        assert_eq!(std::mem::size_of::<Termios>(), 36);
        // SAFETY: exact kernel structure and real private PTY descriptor;
        // no system terminal state or other process affinity is changed.
        unsafe {
            let get: unsafe extern "C" fn(c_int, *mut Termios) -> c_int = std::mem::transmute(address(c"tcgetattr"));
            let set: unsafe extern "C" fn(c_int, c_int, *const Termios) -> c_int = std::mem::transmute(address(c"tcsetattr"));
            let flush: unsafe extern "C" fn(c_int, c_int) -> c_int = std::mem::transmute(address(c"tcflush"));
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(fd >= 0, "{}", std::io::Error::last_os_error());
            let mut out = Guarded { before: 0x12345678, value: std::mem::zeroed(), after: 0x87654321 };
            assert_eq!(get(fd, &mut out.value), 0);
            assert_eq!((out.before, out.after), (0x12345678, 0x87654321));
            let original = out.value;
            out.value.flags[3] ^= libc::ECHO;
            assert_eq!(set(fd, 0, &out.value), 0);
            let changed = out.value.flags[3];
            assert_eq!(get(fd, &mut out.value), 0);
            assert_eq!(out.value.flags[3], changed);
            assert_eq!(set(fd, 0, &original), 0);
            assert_eq!(flush(fd, 2), 0);
            assert_eq!(set(fd, 1234, &original), -1);
            assert_eq!(*libc::__errno_location(), libc::EINVAL);
            assert_eq!(get(-1, &mut out.value), -1);
            assert_eq!(*libc::__errno_location(), libc::EBADF);
            assert_eq!((out.before, out.after), (0x12345678, 0x87654321));
            assert_eq!(libc::close(fd), 0);
            type Affinity = unsafe extern "C" fn(c_int, usize, *mut libc::cpu_set_t) -> c_int;
            let affinity: Affinity = std::mem::transmute(address(c"sched_getaffinity"));
            assert_ne!(address(c"sched_setaffinity"), 0);
            let mut mask: libc::cpu_set_t = std::mem::zeroed();
            assert_eq!(affinity(0, std::mem::size_of_val(&mask), &mut mask), 0);
            assert!(libc::CPU_COUNT(&mask) > 0);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU ABI; use docker-rust.sh test"]
fn legacy_unity_libc_abi() { panic!("run on Linux ARM64 GNU"); }
