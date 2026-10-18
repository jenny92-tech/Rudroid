//! INPUT:  production Bionic symbol source, bounded native buffers and private Linux PTYs
//! OUTPUT:  legacy Unity libc ABI, locale isolation, FORTIFY and kernel-termios regressions
//! POS:    verifies the actual missing import families without forwarding foreign structures
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {
    use std::ffi::{c_char, c_int, c_void, CStr};

    unsafe fn address(name: &CStr) -> usize {
        // SAFETY: immutable C names and the production library/version contract.
        let value = unsafe { android_bionic::rudroid_bionic_symbol(
            name.as_ptr(), c"libc.so".as_ptr(), c"LIBC".as_ptr(), std::ptr::null_mut(),
        ) };
        assert_ne!(value, 0, "missing {}", name.to_string_lossy());
        value
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
            for (alias, base) in [(c"stat64", c"stat"), (c"lstat64", c"lstat"), (c"fstat64", c"fstat"), (c"fstatat64", c"fstatat")] {
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
            use std::io::{Seek, SeekFrom, Write};
            let mut sparse = std::fs::OpenOptions::new().write(true).read(true).open(root.join("assets/file")).unwrap();
            let offset = 0x1_0000_0000_i64;
            sparse.seek(SeekFrom::Start(offset as u64)).unwrap(); sparse.write_all(b"sparse64").unwrap();
            let map: unsafe extern "C" fn(*mut c_void, usize, c_int, c_int, c_int, i64) -> *mut c_void = std::mem::transmute(address(c"mmap64"));
            assert_eq!(address(c"mmap64"), address(c"mmap"));
            use std::os::fd::AsRawFd;
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
