//! INPUT:  production ELF/Bionic cdylibs, local gcc and version-tagged C guest
//! OUTPUT:  relocation/heap/C/POSIX/stdio/pthread/destructor checks and unsupported-ABI rejection
//! POS:    dynamically assembled guest bridge; no capability is statically linked

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires the Linux ARM64 GNU executable guest backend"]
fn versioned_guest_heap_and_stdio_share_host_services() {
    panic!("run with docker-rust.sh test on Linux ARM64");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[test]
fn versioned_guest_heap_and_stdio_share_host_services() {
    use rudroid_common::{symbols, ModuleKind};
    use rudroid_test_support::cargo::ensure_workspace_cdylib;
    use rudroid_unity_loader::{guest, load_deploy_dir, runtime, Selection};
    use std::ffi::{c_char, c_void, CString};
    use std::fs;
    use std::process::Command;

    let root = std::env::temp_dir().join(format!("rudroid-bionic-guest-{}", std::process::id()));
    let kit = root.join("kit");
    fs::create_dir_all(&kit).unwrap();
    for (package, artifact) in [
        ("rudroid-android-framework", "android_base"),
        ("rudroid-android-elf-load", "android_elf_load"),
        ("rudroid-android-bionic", "android_bionic"),
    ] {
        let path = ensure_workspace_cdylib(package, artifact);
        fs::copy(&path, kit.join(path.file_name().unwrap())).unwrap();
    }
    let compile = |args: &[&str]| {
        let output = Command::new("gcc")
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "gcc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let jump_child = std::env::var_os("RUDROID_JUMP_CORRUPT_CHILD");
    let close_child = std::env::var_os("RUDROID_STDIO_CLOSE_CHILD").is_some();
    // Link-only symbol definitions produce Android-style VERNEED tags.
    // This stub is never loaded, mapped, or deployed as an implementation.
    fs::write(
        root.join("stub.c"),
        [
            "malloc",
            "calloc",
            "realloc",
            "free",
            "posix_memalign",
            "memalign",
            "aligned_alloc",
            "malloc_usable_size",
            "pthread_barrier_wait",
            "sigemptyset", "sigaddset", "sigismember", "sigprocmask", "sigaction", "sigaltstack", "raise",
            "sem_init",
            "sem_destroy",
            "sem_post",
            "sem_wait",
            "sem_trywait",
            "sem_getvalue",
            "sem_timedwait",
            "socketpair",
            "send",
            "recv",
            "sendmsg",
            "recvmsg",
            "select",
            "__FD_SET_chk",
            "__FD_ISSET_chk",
            "fcntl",
            "getnameinfo",
            "getaddrinfo",
            "freeaddrinfo",
            "inet_pton",
            "inet_ntop",
            "statfs",
            "uname",
            "ldiv",
            "prctl", "ptrace", "process_vm_readv", "getpid",
            "setjmp", "_setjmp", "sigsetjmp", "longjmp", "_longjmp", "siglongjmp",
            "__register_atfork",
            "fork",
            "waitpid",
            "_exit",
            "strstr",
            "newlocale",
            "duplocale",
            "freelocale",
            "uselocale",
            "setlocale",
            "localeconv",
            "__ctype_get_mb_cur_max",
            "mbsinit",
            "mbrtowc",
            "mbrlen",
            "mbsnrtowcs",
            "mbsrtowcs",
            "wcsnrtombs",
            "wcrtomb",
            "iswalpha_l",
            "iswlower_l",
            "towlower_l",
            "strcoll_l",
            "strtoll_l",
            "strtold_l",
            "pathconf",
            "fpathconf",
            "statvfs",
            "fstatvfs",
            "__system_property_get",
            "__system_property_find",
            "__system_property_read",
            "strerror_r",
            "__gnu_strerror_r",
            "pipe",
            "write",
            "ioctl",
            "fgetwc",
            "fputwc",
            "ungetwc",
            "fwide",
            "clock_getres",
            "clock_gettime",
            "close",
            "closedir",
            "fdopendir",
            "fstat",
            "getauxval",
            "getpagesize",
            "gmtime_r",
            "lseek",
            "nanosleep",
            "open",
            "opendir",
            "pwrite",
            "read",
            "readdir",
            "stat",
            "strtold",
            "swprintf",
            "sysconf",
            "wcstold",
            "pthread_attr_destroy",
            "pthread_attr_getstack",
            "pthread_attr_init",
            "pthread_attr_setstacksize",
            "pthread_cond_broadcast",
            "pthread_cond_destroy",
            "pthread_cond_timedwait",
            "pthread_cond_wait",
            "pthread_create",
            "pthread_equal",
            "pthread_getattr_np",
            "pthread_getspecific",
            "pthread_join",
            "pthread_key_create",
            "pthread_key_delete",
            "pthread_mutex_destroy",
            "pthread_mutex_init",
            "pthread_mutex_lock",
            "pthread_mutex_trylock",
            "pthread_mutex_unlock",
            "pthread_mutexattr_destroy",
            "pthread_mutexattr_init",
            "pthread_mutexattr_setpshared",
            "pthread_mutexattr_settype",
            "pthread_once",
            "pthread_rwlock_destroy",
            "pthread_rwlock_rdlock",
            "pthread_rwlock_trywrlock",
            "pthread_rwlock_unlock",
            "pthread_rwlock_wrlock",
            "pthread_self",
            "pthread_setspecific",
            "pthread_sigmask",
            "__cxa_atexit",
            "__cxa_finalize",
            "__cxa_thread_atexit_impl",
            "memcpy",
            "memmove",
            "memset",
            "memcmp",
            "strlen",
            "strlcpy",
            "strlcat",
            "strdup",
            "strndup",
            "strcmp",
            "strtol",
            "strtod",
            "__errno",
            "gettid",
            "syscall",
            "qsort",
            "bsearch",
            "fopen",
            "fdopen",
            "tmpfile",
            "fclose",
            "fflush",
            "fileno",
            "fread",
            "fwrite",
            "fseek",
            "ftell",
            "fgetpos",
            "fsetpos",
            "fgetc",
            "fputc",
            "ungetc",
            "fputs",
            "feof",
            "ferror",
            "fgets",
            "clearerr",
            "rewind",
            "setbuf",
            "setvbuf",
            "flockfile",
            "funlockfile",
            "ftrylockfile",
            "fprintf",
            "fscanf",
            "vfprintf",
            "vfscanf",
            "vsnprintf",
            "vsscanf",
            "vasprintf",
            "printf",
            "vprintf",
            "puts",
            "putchar",
            "fopen64",
            "fseeko64",
            "ftello64",
        ]
        .map(|name| format!("void {name}(void) {{}}\n"))
        .join("")
            + "unsigned char __sF[456]; void *stdin, *stdout, *stderr;\n",
    )
    .unwrap();
    fs::write(root.join("versions.map"), "LIBC { global: prctl; ptrace; process_vm_readv; getpid; setjmp; _setjmp; sigsetjmp; longjmp; _longjmp; siglongjmp; sigemptyset; sigaddset; sigismember; sigprocmask; sigaction; sigaltstack; raise; sem_init; sem_destroy; sem_post; sem_wait; sem_trywait; sem_getvalue; sem_timedwait; socketpair; send; recv; sendmsg; recvmsg; select; __FD_SET_chk; __FD_ISSET_chk; fcntl; getaddrinfo; freeaddrinfo; getnameinfo; inet_pton; inet_ntop; statfs; uname; ldiv; malloc; calloc; realloc; free; posix_memalign; memalign; malloc_usable_size; pthread_barrier_wait; __register_atfork; fork; waitpid; _exit; strstr; newlocale; duplocale; freelocale; uselocale; setlocale; localeconv; __ctype_get_mb_cur_max; mbsinit; mbrtowc; mbrlen; mbsnrtowcs; mbsrtowcs; wcsnrtombs; wcrtomb; iswalpha_l; iswlower_l; towlower_l; strcoll_l; strtoll_l; strtold_l; pathconf; fpathconf; statvfs; fstatvfs; __system_property_get; __system_property_find; __system_property_read; strerror_r; __gnu_strerror_r; pipe; write; ioctl; fgetwc; fputwc; ungetwc; fwide; clock_getres; clock_gettime; close; closedir; fdopendir; fstat; getauxval; getpagesize; gmtime_r; lseek; nanosleep; open; opendir; pwrite; read; readdir; stat; strtold; swprintf; sysconf; wcstold; pthread_attr_destroy; pthread_attr_getstack; pthread_attr_init; pthread_attr_setstacksize; pthread_cond_broadcast; pthread_cond_destroy; pthread_cond_timedwait; pthread_cond_wait; pthread_create; pthread_equal; pthread_getattr_np; pthread_getspecific; pthread_join; pthread_key_create; pthread_key_delete; pthread_mutex_destroy; pthread_mutex_init; pthread_mutex_lock; pthread_mutex_trylock; pthread_mutex_unlock; pthread_mutexattr_destroy; pthread_mutexattr_init; pthread_mutexattr_setpshared; pthread_mutexattr_settype; pthread_once; pthread_rwlock_destroy; pthread_rwlock_rdlock; pthread_rwlock_trywrlock; pthread_rwlock_unlock; pthread_rwlock_wrlock; pthread_self; pthread_setspecific; pthread_sigmask; __cxa_atexit; __cxa_finalize; __cxa_thread_atexit_impl; __sF; stdin; stdout; stderr; fopen; fdopen; tmpfile; fclose; fflush; fileno; fread; fwrite; fseek; ftell; fgetpos; fsetpos; fgetc; fputc; ungetc; fputs; feof; ferror; fgets; clearerr; rewind; setbuf; setvbuf; flockfile; funlockfile; ftrylockfile; fprintf; fscanf; vfprintf; vfscanf; vsnprintf; vsscanf; vasprintf; printf; vprintf; puts; putchar; memcpy; memmove; memset; memcmp; strlen; strlcpy; strlcat; strdup; strndup; strcmp; strtol; strtod; __errno; gettid; syscall; qsort; bsearch; local: *; };\nLIBC_N { global: fopen64; fseeko64; ftello64; } LIBC;\nLIBC_P { global: aligned_alloc; } LIBC_N;\n").unwrap();
    compile(&[
        "-shared",
        "-fPIC",
        "-fno-builtin",
        "-nostdlib",
        "stub.c",
        "-Wl,-soname,libc.so",
        "-Wl,--version-script=versions.map",
        "-o",
        "stub.so",
    ]);
    fs::write(
        root.join("guest.c"),
        include_str!("fixtures/bionic_alloc.c"),
    )
    .unwrap();
    fs::write(root.join("logstub.c"), "void __android_log_print(void) {}\n").unwrap();
    compile(&["-shared","-fPIC","-nostdlib","-Wl,-soname,liblog.so","logstub.c","-o","logstub.so"]);
    fs::write(root.join("dlstub.c"), "void dl_iterate_phdr(void) {} void dlopen(void) {} void dlclose(void) {} void dlerror(void) {} void dlsym(void) {} void dladdr(void) {}\n").unwrap();
    fs::write(root.join("dlversions.map"), "LIBC { global: dl_iterate_phdr; dlopen; dlclose; dlerror; dlsym; dladdr; local: *; };\n").unwrap();
    compile(&["-shared","-fPIC","-nostdlib","-Wl,-soname,libdl.so",
        "-Wl,--version-script=dlversions.map","dlstub.c","-o","dlstub.so"]);
    fs::write(
        root.join("stdio.c"),
        include_str!("fixtures/bionic_stdio.c"),
    )
    .unwrap();
    fs::write(
        root.join("destructors.c"),
        include_str!("fixtures/bionic_destructors.c"),
    )
    .unwrap();
    fs::write(
        root.join("basic.c"),
        include_str!("fixtures/bionic_basic.c"),
    )
    .unwrap();
    fs::write(
        root.join("pthread.c"),
        include_str!("fixtures/bionic_pthread.c"),
    )
    .unwrap();
    fs::write(
        root.join("posix.c"),
        include_str!("fixtures/bionic_posix.c"),
    )
    .unwrap();
    fs::write(root.join("locale.c"), include_str!("fixtures/bionic_locale.c")).unwrap();
    fs::write(root.join("phdr.c"), include_str!("fixtures/bionic_phdr.c")).unwrap();
    fs::write(root.join("fork.c"), include_str!("fixtures/bionic_fork.c")).unwrap();
    fs::write(root.join("services.c"), include_str!("fixtures/bionic_services.c")).unwrap();
    fs::write(root.join("jump.c"), include_str!("fixtures/bionic_jump.c")).unwrap();
    fs::write(root.join("jump_regs.S"), include_str!("fixtures/bionic_jump_regs.S")).unwrap();
    fs::write(root.join("math.c"), include_str!("fixtures/bionic_math.c")).unwrap();
    fs::write(root.join("mathstub.c"), "void sin(void) {} void cos(void) {} void pow(void) {} void modf(void) {} void ldexp(void) {} void sqrtf(void) {} void nexttowardf(void) {} void atan2f(void) {} void sincos(void) {}\n").unwrap();
    fs::write(root.join("mathversions.map"), "LIBC { global: sin; cos; pow; modf; ldexp; sqrtf; nexttowardf; atan2f; sincos; local: *; };\n").unwrap();
    compile(&["-shared","-fPIC","-nostdlib","-Wl,-soname,libm.so",
        "-Wl,--version-script=mathversions.map","mathstub.c","-o","mathstub.so"]);
    compile(&[
        "-shared",
        "-fPIC",
        "-mno-outline-atomics",
        "-fno-builtin",
        "-nostdlib",
        "guest.c",
        "stdio.c",
        "destructors.c",
        "basic.c",
        "pthread.c",
        "posix.c",
        "locale.c",
        "phdr.c",
        "fork.c",
        "math.c",
        "services.c",
        "jump.c",
        "jump_regs.S",
        "-L.",
        "-l:stub.so",
        "-l:dlstub.so",
        "-l:mathstub.so",
        "-l:logstub.so",
        "-Wl,-z,defs",
        "-o",
        "guest.so",
    ]);

    fs::write(
        root.join("bad.c"),
        "extern void pthread_barrier_wait(void);\nvoid bad_probe(void) { pthread_barrier_wait(); }\n",
    )
    .unwrap();
    compile(&[
        "-shared",
        "-fPIC",
        "-fno-builtin",
        "-nostdlib",
        "bad.c",
        "-L.",
        "-l:stub.so",
        "-Wl,-z,defs",
        "-o",
        "bad.so",
    ]);
    let capabilities = unsafe { load_deploy_dir(&kit, &Selection::All) }.unwrap();
    let plan = guest::GuestPlan {
        game_files: root.clone(),
        entries: vec![guest::GuestEntry {
            kind: Some(ModuleKind::Main),
            relative_path: "guest.so",
            required: true,
            gates: &[],
        }],
    };
    let elf = capabilities
        .iter()
        .find(|c| c.descriptor.id == "android_elf_load")
        .unwrap();
    let bionic = capabilities
        .iter()
        .find(|c| c.descriptor.id == "android_bionic")
        .unwrap();
    unsafe {
        use rudroid_common::symbols::elf_load::{ImportVisitor, VISIT_IMPORTS};
        use std::ffi::CStr;
        type Record = (String, Option<String>, Option<String>, u32);
        unsafe extern "C" fn record(
            name: *const c_char,
            library: *const c_char,
            version: *const c_char,
            flags: u32,
            user: *mut c_void,
        ) {
            let records = &mut *user.cast::<Vec<Record>>();
            let owned = |pointer| {
                if pointer == std::ptr::null() {
                    None
                } else {
                    Some(CStr::from_ptr(pointer).to_str().unwrap().to_owned())
                }
            };
            records.push((owned(name).unwrap(), owned(library), owned(version), flags));
        }
        let visit: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, Option<ImportVisitor>, *mut c_void) -> i32,
        > = elf.library().get(VISIT_IMPORTS.as_bytes()).unwrap();
        let path = CString::new(root.join("guest.so").to_str().unwrap()).unwrap();
        let mut records = Vec::<Record>::new();
        assert_eq!(
            visit(
                path.as_ptr(),
                Some(record),
                (&mut records as *mut Vec<Record>).cast()
            ),
            0
        );
        let free = records
            .iter()
            .filter(|entry| entry.0 == "free")
            .collect::<Vec<_>>();
        assert_eq!(
            free.len(),
            1,
            "one visit per dynsym import despite multiple relocations"
        );
        assert_eq!(
            *free[0],
            (
                "free".into(),
                Some("libc.so".into()),
                Some("LIBC".into()),
                0
            )
        );
        assert!(records
            .iter()
            .any(|entry| entry.0 == "__cxa_thread_atexit_impl"));
        assert_eq!(visit(path.as_ptr(), None, std::ptr::null_mut()), -1);
        let invalid = CString::new(root.join("stub.c").to_str().unwrap()).unwrap();
        assert_eq!(
            visit(
                invalid.as_ptr(),
                Some(record),
                (&mut records as *mut Vec<Record>).cast()
            ),
            -1
        );
    }
    let mut bad_plan = plan.clone();
    bad_plan.entries[0].relative_path = "bad.so";
    let error = unsafe { guest::bring_up(&capabilities, &bad_plan) }
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("pthread_barrier_wait") && error.contains("LIBC"),
        "{error}"
    );
    unsafe {
        let load: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_void> = elf
            .library()
            .get(symbols::elf_exec::EXEC_LOAD.as_bytes())
            .unwrap();
        let path = CString::new(root.join("guest.so").to_str().unwrap()).unwrap();
        assert!(
            load(path.as_ptr()).is_null(),
            "failed bring-up must revoke its native ABI source"
        );
    }
    let events = unsafe { guest::bring_up(&capabilities, &plan) }
        .expect("versioned guest maps through the production source");
    assert_eq!(events.len(), 1);
    if let Some(case) = jump_child {
        unsafe {
            #[repr(C)] struct Limit { current: u64, maximum: u64 }
            unsafe extern "C" { fn setrlimit(resource: i32, limit: *const Limit) -> i32; }
            assert_eq!(setrlimit(4,&Limit { current:0,maximum:0 }),0);
            let name=if case=="cookie" { "rudroid_jump_cookie_corrupt_probe" } else { "rudroid_jump_corrupt_probe" };
            let address=runtime::guest_exec_symbol(events[0].base,name).unwrap();
            let probe: unsafe extern "C" fn()=std::mem::transmute(address);
            probe(); panic!("damaged jump buffer returned");
        }
    }
    if close_child {
        unsafe {
            let address =
                runtime::guest_exec_symbol(events[0].base, "rudroid_standard_close_probe").unwrap();
            let probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
            assert_eq!(
                probe(),
                0,
                "closing stderr must not free static __sF memory"
            );
        }
        fs::remove_dir_all(root).unwrap();
        return;
    }
    let fd_count = || fs::read_dir("/proc/self/fd").unwrap().count();
    let before = fd_count();
    unsafe {
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_stdio_probe").unwrap();
        let probe: unsafe extern "C" fn(*const c_char) -> i32 = std::mem::transmute(address);
        let path = CString::new(root.join("io.txt").to_str().unwrap()).unwrap();
        for _ in 0..128 {
            assert_eq!(probe(path.as_ptr()), 0, "native stdio guest probe failed");
        }
        assert_eq!(fs::read(root.join("io.txt")).unwrap(), b"shared");
        use std::os::fd::IntoRawFd;
        let fd = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(root.join("owned.txt"))
            .unwrap()
            .into_raw_fd();
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_fdopen_probe").unwrap();
        let adopt: unsafe extern "C" fn(i32) -> i32 = std::mem::transmute(address);
        assert_eq!(adopt(fd), 0);
        assert_eq!(fs::read(root.join("owned.txt")).unwrap(), b"owned");
    }
    assert_eq!(
        fd_count(),
        before,
        "guest fclose must release every native file descriptor"
    );
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "versioned_guest_heap_and_stdio_share_host_services",
            "--nocapture",
        ])
        .env("RUDROID_STDIO_CLOSE_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "standard close child failed: {:?}\n{}\n{}",
        child.status,
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    use std::os::unix::process::ExitStatusExt;
    for case in ["checksum","cookie"] {
        let corrupt = Command::new(std::env::current_exe().unwrap())
            .args(["--exact","versioned_guest_heap_and_stdio_share_host_services","--nocapture"])
            .env("RUDROID_JUMP_CORRUPT_CHILD",case).output().unwrap();
        assert_eq!(corrupt.status.signal(),Some(6));
        assert!(String::from_utf8_lossy(&corrupt.stderr).contains("Bionic jump buffer validation failed"));
    }
    unsafe {
        let address=runtime::guest_exec_symbol(events[0].base,"rudroid_jump_probe").unwrap();
        let probe: unsafe extern "C" fn()->i32=std::mem::transmute(address);
        for _ in 0..128 { assert_eq!(probe(),0,"guest stack/register/mask jump recovery failed"); }
    }
    unsafe {
        use std::io::Write;
        use std::os::fd::IntoRawFd;
        let path = root.join("posix.txt");
        let directory = CString::new(root.to_str().unwrap()).unwrap();
        let path_arg = CString::new(path.to_str().unwrap()).unwrap();
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_posix_probe").unwrap();
        let posix: unsafe extern "C" fn(*const c_char, *const c_char, i32) -> i32 =
            std::mem::transmute(address);
        let before = fd_count();
        for _ in 0..128 {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            file.write_all(b"host").unwrap();
            assert_eq!(
                posix(directory.as_ptr(), path_arg.as_ptr(), file.into_raw_fd()),
                0,
                "guest stat/dirent/time/sysconf/FP128 ABI failed"
            );
        }
        assert_eq!(
            fd_count(),
            before,
            "guest close/closedir must reclaim all native FDs"
        );
        assert_eq!(fs::read(&path).unwrap(), b"shared fd");
        unsafe extern "C" {
            fn setlocale(category: i32, name: *const c_char) -> *mut c_char;
        }
        let native_locale_before = std::ffi::CStr::from_ptr(setlocale(6, std::ptr::null())).to_owned();
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_locale_probe").unwrap();
        let locale_probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        for _ in 0..128 {
            assert_eq!(locale_probe(), 0, "guest locale/UTF-8 contract failed");
        }
        assert_eq!(std::ffi::CStr::from_ptr(setlocale(6, std::ptr::null())), native_locale_before.as_c_str(), "guest setlocale must preserve the native process locale");
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_phdr_probe").unwrap();
        let phdr_probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        for _ in 0..128 {
            assert_eq!(phdr_probe(), 0, "guest constructor/iterator/reentrant phdr contract failed");
        }
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_fork_probe").unwrap();
        let fork_probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        for _ in 0..32 { assert_eq!(fork_probe(), 0, "guest fork/DSO callback contract failed"); }
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_math_probe").unwrap();
        let math_probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        for _ in 0..128 { assert_eq!(math_probe(), 0, "guest libm FP register ABI failed"); }
        let before = fd_count();
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_services_probe").unwrap();
        let services_probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        for _ in 0..128 { assert_eq!(services_probe(), 0, "guest native semaphore/socket/DNS/POSIX ABI failed"); }
        assert_eq!(fd_count(), before, "network service descriptors must be reclaimed");
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_pthread_probe").unwrap();
        let probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        assert_eq!(
            probe(),
            0,
            "guest pthread contention/TLS/4-byte alignment failed"
        );
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_basic_probe").unwrap();
        let probe: unsafe extern "C" fn() -> i32 = std::mem::transmute(address);
        for _ in 0..128 {
            assert_eq!(probe(), 0, "native C/errno/callback guest ABI failed");
        }
        let address = runtime::guest_exec_symbol(events[0].base, "rudroid_allocation_probe").unwrap();
        let probe: unsafe extern "C" fn(*mut c_void) -> i32 = std::mem::transmute(address);
        unsafe extern "C" {
            fn malloc(size: usize) -> *mut c_void;
        }
        for _ in 0..1_000 {
            let host_pointer = malloc(16);
            assert!(!host_pointer.is_null());
            assert_eq!(
                probe(host_pointer),
                0,
                "all eight guest imports execute against the same native heap"
            );
        }
        let query: libloading::Symbol<symbols::elf_exec::VersionedSymbolSource> =
            bionic.library().get(b"rudroid_bionic_symbol").unwrap();
        assert_eq!(
            query(
                c"pthread_barrier_wait".as_ptr(),
                c"libc.so".as_ptr(),
                c"LIBC".as_ptr(),
                std::ptr::null_mut()
            ),
            0
        );
        assert_eq!(
            query(
                c"free".as_ptr(),
                c"libc.so.6".as_ptr(),
                c"GLIBC_2.17".as_ptr(),
                std::ptr::null_mut()
            ),
            0
        );
    }
    unsafe {
        let load: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_void> = elf
            .library()
            .get(symbols::elf_exec::EXEC_LOAD.as_bytes())
            .unwrap();
        let symbol: libloading::Symbol<unsafe extern "C" fn(*mut c_void, *const c_char) -> usize> =
            elf.library()
                .get(symbols::elf_exec::EXEC_SYMBOL.as_bytes())
                .unwrap();
        let destroy: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> = elf
            .library()
            .get(symbols::elf_exec::EXEC_DESTROY.as_bytes())
            .unwrap();
        let path = CString::new(root.join("guest.so").to_str().unwrap()).unwrap();
        let handle = load(path.as_ptr());
        assert!(!handle.is_null());
        let thread_address = symbol(handle, c"rudroid_thread_destructor_probe".as_ptr());
        assert_ne!(thread_address, 0);
        let thread_probe: unsafe extern "C" fn(*mut i32) -> i32 =
            std::mem::transmute(thread_address);
        let mut thread_results = [0i32; 16];
        let output_address = thread_results.as_mut_ptr() as usize;
        std::thread::spawn(move || {
            assert_eq!(thread_probe(output_address as *mut i32), 0);
        })
        .join()
        .unwrap(); // retain mapping/observation buffer until native TLS destructors finish
        assert_eq!(&thread_results[..4], &[3, 7, 8, 6]);
        let address = symbol(handle, c"rudroid_destructor_probe".as_ptr());
        assert_ne!(address, 0);
        let probe: unsafe extern "C" fn(*mut i32) -> i32 = std::mem::transmute(address);
        let mut results = [0i32; 16];
        assert_eq!(probe(results.as_mut_ptr()), 0);
        assert_eq!(&results[..5], &[4, 3, 2, 4, 1]);
        destroy(handle); // runs live guest TLS/process callbacks BEFORE munmap
        assert_eq!(&results[..10], &[9, 3, 2, 4, 1, 7, 8, 6, 5, 9]);
    }
    fs::remove_dir_all(root).unwrap();
}
