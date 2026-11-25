//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::registry::HostStr;
use std::ffi::CStr;

unsafe fn query(name: &CStr, library: &CStr, version: &CStr) -> usize {
    rudroid_bionic_symbol(
        name.as_ptr(),
        library.as_ptr(),
        version.as_ptr(),
        std::ptr::null_mut(),
    )
}

#[test]
fn accepts_only_the_attested_library_version_and_allocators() {
    unsafe {
        assert_eq!(
            query(c"free", c"libc.so", c"LIBC"),
            libc::free as *const () as usize
        );
        assert_eq!(query(c"free", c"libc.so.6", c"LIBC"), 0);
        assert_eq!(query(c"free", c"libc.so", c"GLIBC_2.17"), 0);
        assert_eq!(query(c"free", c"libc.so", c"LIBC_PRIVATE"), 0);
        assert_eq!(query(c"free", c"libc.so", c"LIBC_P"), 0);
        assert_eq!(query(c"aligned_alloc", c"libc.so", c"LIBC"), 0);
        assert_eq!(
            query(c"aligned_alloc", c"libc.so", c"LIBC_P"),
            libc::aligned_alloc as *const () as usize
        );
        for name in [
            c"sigqueue",
            c"pthread_barrier_wait",
            c"sem_clockwait",
            c"unknown",
        ] {
            assert_eq!(query(name, c"libc.so", c"LIBC"), 0);
        }
        for name in [
            c"freopen", c"tmpnam", c"abs", c"inet_aton", c"hstrerror", c"iscntrl", c"ispunct",
        ] {
            assert_ne!(query(name, c"libc.so", c"LIBC"), 0, "{name:?}");
        }
        for name in [c"sem_wait", c"getaddrinfo", c"newlocale", c"environ"] {
            assert_ne!(query(name,c"libc.so",c"LIBC"),0);
        }
        for name in [
            c"nice", c"random", c"sleep", c"execv", c"settimeofday",
            c"__open_2", c"__openat_2", c"__read_chk", c"__strchr_chk",
            c"__strcpy_chk", c"__vsprintf_chk", c"_ctype_",
        ] {
            assert_ne!(query(name, c"libc.so", c"LIBC"), 0, "{name:?}");
            assert_eq!(query(name, c"libc.so.6", c"LIBC"), 0, "{name:?}");
            assert_eq!(query(name, c"libc.so", c"LIBC_P"), 0, "{name:?}");
        }
        // nice(0) queries the current process priority without changing it.
        let nice: unsafe extern "C" fn(std::ffi::c_int) -> std::ffi::c_int =
            std::mem::transmute(query(c"nice", c"libc.so", c"LIBC"));
        assert!(nice(0) >= -20);
        assert_ne!(query(c"nexttowardf",c"libm.so",c"LIBC"),0);
        assert_eq!(query(c"nexttowardf",c"libc.so",c"LIBC"),0);
        assert_eq!(query(c"malloc",c"libm.so",c"LIBC"),0);
        assert_eq!(query(c"pow",c"libm.so",c"GLIBC_2.17"),0);
        assert_ne!(query(c"__sF", c"libc.so", c"LIBC"), 0);
        assert_ne!(query(c"fopen", c"libc.so", c"LIBC"), 0);
        assert_eq!(query(c"fopen64", c"libc.so", c"LIBC"), 0);
        assert_ne!(query(c"fopen64", c"libc.so", c"LIBC_N"), 0);
        for name in [c"pthread_attr_getinheritsched", c"pthread_attr_setinheritsched",
            c"pthread_mutexattr_getprotocol", c"pthread_mutexattr_setprotocol"] {
            assert_eq!(query(name, c"libc.so", c"LIBC"), 0);
            assert_ne!(query(name, c"libc.so", c"LIBC_P"), 0);
        }
        assert_eq!(query(c"pthread_getname_np", c"libc.so", c"LIBC"), 0);
        assert_ne!(query(c"pthread_getname_np", c"libc.so", c"LIBC_O"), 0);
        assert_eq!(query(c"fopen64", c"libc.so", c"LIBC_P"), 0);
        assert_eq!(query(c"__sF", c"libc.so", c"LIBC_N"), 0);
        for name in [
            c"_Znwm", c"_Znam", c"_ZdlPv", c"_ZdaPv",
            c"__cxa_guard_acquire", c"__cxa_guard_release",
            c"__cxa_guard_abort", c"__cxa_pure_virtual",
        ] {
            assert_ne!(query(name, c"libstdc++.so", c"LIBC_O"), 0, "{name:?}");
            assert_eq!(query(name, c"libc.so", c"LIBC_O"), 0, "{name:?}");
            assert_eq!(query(name, c"libstdc++.so", c"LIBC"), 0, "{name:?}");
            assert_eq!(query(name, c"libstdc++.so.6", c"LIBC_O"), 0, "{name:?}");
        }
        assert_eq!(
            rudroid_bionic_symbol(
                c"free".as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut()
            ),
            libc::free as *const () as usize
        );
        assert_eq!(
            rudroid_bionic_symbol(
                c"free".as_ptr(),
                c"libc.so".as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut()
            ),
            0
        );
        assert_eq!(
            rudroid_bionic_symbol(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut()
            ),
            0
        );
    }
}

#[test]
fn fsync_uses_the_native_descriptor_and_errno_with_the_android_version_gate() {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("rudroid-fsync-{}-{nonce}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"native descriptor sync").unwrap();

    // SAFETY: live C strings, the attested int(int) prototype and a live
    // host descriptor. -1 is a valid invalid-fd input; errno is thread-local.
    unsafe {
        let address = query(c"fsync", c"libc.so", c"LIBC");
        assert_ne!(address, 0);
        assert_eq!(query(c"fsync", c"libc.so", c"LIBC_P"), 0);
        assert_eq!(query(c"fsync", c"libm.so", c"LIBC"), 0);
        let sync: unsafe extern "C" fn(std::ffi::c_int) -> std::ffi::c_int =
            std::mem::transmute(address);
        assert_eq!(sync(file.as_raw_fd()), 0);
        assert_eq!(sync(-1), -1);
        assert_eq!(*libc::__errno_location(), libc::EBADF);
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"native descriptor sync");
    drop(file);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn extracted_asset_uris_share_open_stat_access_and_stdio_contents() {
    use std::ffi::CString;
    let root = std::env::temp_dir().join(format!(
        "rudroid-assets-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(root.join("assets/aa")).unwrap();
    std::fs::write(root.join("assets/aa/settings.json"), b"resource-data").unwrap();
    let uri = CString::new(format!("jar:file://{}!/assets/aa/settings.json", root.display())).unwrap();
    let prefixed_uri = CString::new(format!("{}/jar:file://!/assets/aa/settings.json", root.display())).unwrap();
    let missing = CString::new(format!("jar:file://{}!/assets/aa/missing.json", root.display())).unwrap();
    // SAFETY: the declared ARM64 C prototypes, live NUL strings and output
    // storage; descriptors/stdio facades are consumed once after reading.
    unsafe {
        let open: unsafe extern "C" fn(*const std::ffi::c_char, i32, ...) -> i32 =
            std::mem::transmute(query(c"open", c"libc.so", c"LIBC"));
        let stat: unsafe extern "C" fn(*const std::ffi::c_char, *mut libc::stat) -> i32 =
            std::mem::transmute(query(c"stat", c"libc.so", c"LIBC"));
        let access: unsafe extern "C" fn(*const std::ffi::c_char, i32) -> i32 =
            std::mem::transmute(query(c"access", c"libc.so", c"LIBC"));
        let fopen: unsafe extern "C" fn(*const std::ffi::c_char, *const std::ffi::c_char) -> *mut stdio::File =
            std::mem::transmute(query(c"fopen", c"libc.so", c"LIBC"));
        let fread: unsafe extern "C" fn(*mut std::ffi::c_void, usize, usize, *mut stdio::File) -> usize =
            std::mem::transmute(query(c"fread", c"libc.so", c"LIBC"));
        let fclose: unsafe extern "C" fn(*mut stdio::File) -> i32 =
            std::mem::transmute(query(c"fclose", c"libc.so", c"LIBC"));
        let fd = open(uri.as_ptr(), libc::O_RDONLY);
        assert!(fd >= 0, "extracted URI read failed: {}", std::io::Error::last_os_error());
        let mut bytes = [0_u8; 13];
        assert_eq!(libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()), 13);
        assert_eq!(&bytes, b"resource-data");
        assert_eq!(libc::close(fd), 0);
        let mut metadata = std::mem::zeroed();
        assert_eq!(stat(uri.as_ptr(), &mut metadata), 0);
        assert_eq!(metadata.st_size, 13);
        assert_eq!(access(uri.as_ptr(), libc::R_OK), 0);
        let file = fopen(uri.as_ptr(), c"rb".as_ptr());
        assert!(!file.is_null());
        bytes.fill(0);
        assert_eq!(fread(bytes.as_mut_ptr().cast(), 1, bytes.len(), file), 13);
        assert_eq!(&bytes, b"resource-data");
        assert_eq!(fclose(file), 0);
        let fd = open(prefixed_uri.as_ptr(), libc::O_RDONLY);
        assert!(fd >= 0);
        bytes.fill(0);
        assert_eq!(libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()), 13);
        assert_eq!(&bytes, b"resource-data");
        assert_eq!(libc::close(fd), 0);
        assert_eq!(stat(prefixed_uri.as_ptr(), &mut metadata), 0);
        let file = fopen(prefixed_uri.as_ptr(), c"rb".as_ptr());
        assert!(!file.is_null());
        assert_eq!(fread(bytes.as_mut_ptr().cast(), 1, bytes.len(), file), 13);
        assert_eq!(&bytes, b"resource-data");
        assert_eq!(fclose(file), 0);
        assert_eq!(open(missing.as_ptr(), libc::O_RDONLY), -1);
        assert_eq!(*libc::__errno_location(), libc::ENOENT);
        assert!(fopen(missing.as_ptr(), c"rb".as_ptr()).is_null());
        assert_eq!(*libc::__errno_location(), libc::ENOENT);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inverse_hyperbolic_imports_keep_the_libm_scalar_abi_and_version_gate() {
    // SAFETY: live strings and the ARM64 double(double) libm prototype;
    // only symbols accepted under the exact libm.so/LIBC node are called.
    unsafe {
        for (name, input, expected) in [
            (c"acosh", 2.0_f64, (2.0_f64 + 3.0_f64.sqrt()).ln()),
            (c"asinh", 1.0_f64, (1.0_f64 + 2.0_f64.sqrt()).ln()),
            (c"atanh", 0.5_f64, 3.0_f64.ln() / 2.0),
        ] {
            let address = query(name, c"libm.so", c"LIBC");
            assert_ne!(address, 0);
            assert_eq!(query(name, c"libc.so", c"LIBC"), 0);
            assert_eq!(query(name, c"libm.so", c"LIBC_P"), 0);
            let function: unsafe extern "C" fn(f64) -> f64 = std::mem::transmute(address);
            assert!((function(input) - expected).abs() < 1e-12, "{name:?}");
        }
    }
}

#[test]
fn writev_preserves_iovec_contents_native_descriptor_and_errno() {
    use std::os::fd::AsRawFd;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("rudroid-writev-{}-{nonce}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let chunks: [&[u8]; 2] = [b"vectored ", b"native I/O"];
    let vectors = chunks.map(|bytes| libc::iovec {
        iov_base: bytes.as_ptr().cast_mut().cast(),
        iov_len: bytes.len(),
    });
    // SAFETY: the compile-attested ARM64 iovec layout and ssize_t(int,
    // const iovec*, int) prototype; both buffers and the descriptor are live.
    unsafe {
        let address = query(c"writev", c"libc.so", c"LIBC");
        assert_ne!(address, 0);
        assert_eq!(query(c"writev", c"libc.so", c"LIBC_P"), 0);
        let write: unsafe extern "C" fn(std::ffi::c_int, *const libc::iovec, std::ffi::c_int) -> isize =
            std::mem::transmute(address);
        assert_eq!(write(file.as_raw_fd(), vectors.as_ptr(), 2), 19);
        assert_eq!(write(-1, vectors.as_ptr(), 2), -1);
        assert_eq!(*libc::__errno_location(), libc::EBADF);
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"vectored native I/O");
    drop(file);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn isspace_classifies_ascii_bytes_without_indexing_outside_the_table() {
    // SAFETY: live lookup strings and the int(int) byte-classification ABI.
    unsafe {
        let address = query(c"isspace", c"libc.so", c"LIBC");
        assert_ne!(address, 0);
        assert_eq!(query(c"isspace", c"libc.so", c"LIBC_P"), 0);
        let classify: unsafe extern "C" fn(std::ffi::c_int) -> std::ffi::c_int =
            std::mem::transmute(address);
        for byte in 0..=255 {
            assert_eq!(classify(byte), i32::from(byte == 32 || (9..=13).contains(&byte)));
        }
        for outside in [-1, -128, 256, i32::MAX] {
            assert_eq!(classify(outside), 0);
        }
    }
}

#[test]
fn android_logging_formats_bounded_text_into_the_shared_callback() {
    unsafe extern "C" { fn rudroid_bionic_install_logger(log: Option<rudroid_common::host_api::LogFn>, diagnostics: i32); }
    static CAPTURED: std::sync::Mutex<Option<(Vec<u8>, Vec<u8>)>> = std::sync::Mutex::new(None);
    extern "C" fn capture(tag: HostStr<'_>, message: HostStr<'_>) {
        *CAPTURED.lock().unwrap_or_else(|e| e.into_inner()) = Some((
            tag.to_cstr().unwrap().to_bytes().to_vec(),
            message.to_cstr().unwrap().to_bytes().to_vec(),
        ));
        // SAFETY: the calling thread's errno slot.
        unsafe { *libc::__errno_location()=91; }
    }
    let take = || CAPTURED.lock().unwrap_or_else(|e| e.into_inner()).take().expect("native log callback");
    // SAFETY: the resolved liblog C prototypes match the native entries;
    // every format/string buffer is terminated and lives through its call.
    unsafe {
        type Print = unsafe extern "C" fn(std::ffi::c_int,*const c_char,*const c_char,...)->std::ffi::c_int;
        type Write = unsafe extern "C" fn(std::ffi::c_int,*const c_char,*const c_char)->std::ffi::c_int;
        let print: Print=std::mem::transmute(rudroid_bionic_symbol(c"__android_log_print".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut()));
        let write: Write=std::mem::transmute(rudroid_bionic_symbol(c"__android_log_write".as_ptr(),std::ptr::null(),std::ptr::null(),std::ptr::null_mut()));
        rudroid_bionic_install_logger(Some(capture),1);
        *libc::__errno_location()=90;
        assert_eq!(print(3,c"shared".as_ptr(),c"%d %.2f".as_ptr(),7i32,1.25f64),-1);
        assert!(CAPTURED.lock().unwrap_or_else(|e| e.into_inner()).is_none());
        assert_eq!(print(4,c"shared".as_ptr(),c"%d %.2f".as_ptr(),7i32,1.25f64),1);
        assert_eq!(take(),(b"shared".to_vec(),b"7 1.25".to_vec()));
        assert_eq!(*libc::__errno_location(),90);

        let long = std::ffi::CString::new(format!("{}sdk-caller-tail", "x".repeat(3000))).unwrap();
        let large = std::ffi::CString::new("x".repeat(8192)).unwrap();
        for priority in [6,7] {
            // The caller tail must survive both printf formatting and the
            // ERROR tag wrapper, including the remaining varargs.
            assert_eq!(print(priority,c"shared".as_ptr(),c"%s:%d:%.2f".as_ptr(),long.as_ptr(),7i32,1.25f64),1);
            let expected = format!("[shared] {}:7:1.25",long.to_str().unwrap()).into_bytes();
            assert_eq!(take(),(b"ERROR".to_vec(),expected));
            assert_eq!(*libc::__errno_location(),90);
            assert_eq!(print(priority,c"shared".as_ptr(),c"%s".as_ptr(),large.as_ptr()),1);
            let (tag,message)=take();
            assert_eq!(tag,b"ERROR");
            assert_eq!(&message[..9],b"[shared] ");
            assert_eq!(&message[9..],vec![b'x';4095]);
        }
        for priority in [4,5] {
            assert_eq!(print(priority,c"shared".as_ptr(),c"%s".as_ptr(),large.as_ptr()),1);
            assert_eq!(take(),(b"shared".to_vec(),vec![b'x';1023]));
        }
        assert_eq!(write(6,c"shared".as_ptr(),long.as_ptr()),1);
        assert_eq!(take(),(b"ERROR".to_vec(),format!("[shared] {}",long.to_str().unwrap()).into_bytes()));
        assert_eq!(write(6,c"shared".as_ptr(),large.as_ptr()),1);
        let (tag,message)=take();
        assert_eq!(tag,b"ERROR");
        assert_eq!(message.len(),4223);
        assert_eq!(&message[..9],b"[shared] ");
        assert!(message[9..].iter().all(|&byte| byte==b'x'));

        rudroid_bionic_install_logger(Some(capture),0);
        assert_eq!(print(4,c"shared".as_ptr(),c"suppressed".as_ptr()),-1);
        assert_eq!(print(5,c"shared".as_ptr(),c"suppressed".as_ptr()),-1);
        assert!(CAPTURED.lock().unwrap_or_else(|e| e.into_inner()).is_none());
        assert_eq!(print(6,c"shared".as_ptr(),c"%s".as_ptr(),large.as_ptr()),1);
        assert_eq!(take(),(b"ERROR".to_vec(),[b"[shared] ".as_slice(),vec![b'x';1023].as_slice()].concat()));
        assert_eq!(write(6,c"shared".as_ptr(),large.as_ptr()),1);
        let (tag,message)=take();
        assert_eq!(tag,b"ERROR");
        assert_eq!(message.len(),1151);
        assert_eq!(&message[..9],b"[shared] ");
        assert!(message[9..].iter().all(|&byte| byte==b'x'));
        assert_eq!(print(6,c"shared".as_ptr(),c"important".as_ptr()),1);
        assert_eq!(take(),(b"ERROR".to_vec(),b"[shared] important".to_vec()));
        assert_eq!(*libc::__errno_location(),90);
        rudroid_bionic_install_logger(None,0);
    }
}

#[test]
fn native_allocation_family_round_trips_through_provider_addresses() {
    // Typed conversion is only a C ABI test: production returns the
    // original native address directly, without wrapping or retaining it.
    unsafe {
        let calloc: unsafe extern "C" fn(usize, usize) -> *mut c_void =
            std::mem::transmute(query(c"calloc", c"libc.so", c"LIBC"));
        let realloc: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void =
            std::mem::transmute(query(c"realloc", c"libc.so", c"LIBC"));
        let free: unsafe extern "C" fn(*mut c_void) =
            std::mem::transmute(query(c"free", c"libc.so", c"LIBC"));
        let usable: unsafe extern "C" fn(*mut c_void) -> usize =
            std::mem::transmute(query(c"malloc_usable_size", c"libc.so", c"LIBC"));
        let pointer = calloc(8, 8);
        assert!(!pointer.is_null());
        assert!(std::slice::from_raw_parts(pointer.cast::<u8>(), 64)
            .iter()
            .all(|&b| b == 0));
        std::ptr::write_bytes(pointer, 0x3c, 64);
        let grown = realloc(pointer, 256);
        assert!(!grown.is_null());
        assert!(std::slice::from_raw_parts(grown.cast::<u8>(), 64)
            .iter()
            .all(|&b| b == 0x3c));
        assert!(usable(grown) >= 256);
        free(grown);
        free(std::ptr::null_mut());
        let align: unsafe extern "C" fn(*mut *mut c_void, usize, usize) -> i32 =
            std::mem::transmute(query(c"posix_memalign", c"libc.so", c"LIBC"));
        let mut aligned = std::ptr::null_mut();
        assert_eq!(align(&mut aligned, 64, 128), 0);
        assert!(!aligned.is_null());
        assert_eq!(aligned as usize % 64, 0);
        free(aligned);
    }
}

#[test]
fn soul_sdk_libc_imports_resolve_and_keep_bionic_bounds() {
    unsafe {
        let exact17 = [
            c"__fgets_chk", c"__strcat_chk", c"__strlcpy_chk", c"__strncat_chk",
            c"__strncpy_chk", c"__strncpy_chk2", c"__strrchr_chk",
            c"__system_property_find_nth", c"llabs", c"memmem", c"mincore",
            c"inotify_init", c"inotify_add_watch", c"popen", c"pclose", c"inet_ntoa",
            c"strtok",
        ];
        assert_eq!(exact17.len(), 17);
        for name in exact17 {
            assert_ne!(query(name, c"libc.so", c"LIBC"), 0, "missing {name:?}");
        }

        type Memmem = unsafe extern "C" fn(*const c_void, usize, *const c_void, usize)
            -> *mut c_void;
        let memmem: Memmem = std::mem::transmute(query(c"memmem", c"libc.so", c"LIBC"));
        let haystack = [1u8, 2, 3, 4];
        let needle = [3u8, 4];
        assert_eq!(
            memmem(haystack.as_ptr().cast(), haystack.len(), needle.as_ptr().cast(), needle.len()),
            haystack.as_ptr().add(2).cast_mut().cast()
        );

        type FindNth = unsafe extern "C" fn(u32) -> *const c_void;
        type PropertyRead = unsafe extern "C" fn(*const c_void, *mut c_char, *mut c_char) -> i32;
        let find_nth: FindNth = std::mem::transmute(query(c"__system_property_find_nth", c"libc.so", c"LIBC"));
        let read: PropertyRead = std::mem::transmute(query(c"__system_property_read", c"libc.so", c"LIBC"));
        let first = find_nth(0);
        assert!(!first.is_null());
        assert!(find_nth(3).is_null());
        let mut name = [0 as c_char; 64];
        let mut value = [0 as c_char; 64];
        assert!(read(first, name.as_mut_ptr(), value.as_mut_ptr()) > 0);
        assert_eq!(CStr::from_ptr(name.as_ptr()).to_bytes(), b"ro.product.cpu.abilist");
        assert_eq!(CStr::from_ptr(value.as_ptr()).to_bytes(), b"arm64-v8a");

        type Strtok = unsafe extern "C" fn(*mut c_char, *const c_char) -> *mut c_char;
        let strtok: Strtok = std::mem::transmute(query(c"strtok", c"libc.so", c"LIBC"));
        let mut text = *b"a,b;;c\0";
        let delimiters = c",;";
        let mut tokens = Vec::new();
        let mut cursor = text.as_mut_ptr().cast::<c_char>();
        loop {
            let token = strtok(cursor, delimiters.as_ptr());
            if token.is_null() { break; }
            tokens.push(CStr::from_ptr(token).to_bytes().to_vec());
            cursor = std::ptr::null_mut();
        }
        assert_eq!(tokens, [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }
}

#[test]
fn popen_returns_a_bionic_file_facade_for_fgets_chk_and_pclose() {
    unsafe {
        type Popen = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut stdio::File;
        type FgetsChk = unsafe extern "C" fn(*mut c_char, std::ffi::c_int, *mut stdio::File, usize)
            -> *mut c_char;
        type Pclose = unsafe extern "C" fn(*mut stdio::File) -> std::ffi::c_int;
        let open: Popen = std::mem::transmute(query(c"popen", c"libc.so", c"LIBC"));
        let fgets_chk: FgetsChk = std::mem::transmute(query(c"__fgets_chk", c"libc.so", c"LIBC"));
        let close: Pclose = std::mem::transmute(query(c"pclose", c"libc.so", c"LIBC"));
        let file = open(c"printf sdk-shell".as_ptr(), c"r".as_ptr());
        assert!(!file.is_null());
        let mut buffer = [0 as c_char; 32];
        assert_eq!(
            fgets_chk(buffer.as_mut_ptr(), buffer.len() as std::ffi::c_int, file, buffer.len()),
            buffer.as_mut_ptr()
        );
        assert_eq!(CStr::from_ptr(buffer.as_ptr()).to_bytes(), b"sdk-shell");
        assert_eq!(close(file), 0);
    }
}
