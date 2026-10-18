//! INPUT:  production Bionic provider, native FILE locking and Rust allocation counter
//! OUTPUT:  shared identities, zero-Rust-allocation C/I/O/POSIX calls and FD recovery
//! POS:    measures adapter overhead separately from host libc's own buffering
#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {

    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::ffi::{c_char, c_void, CStr};

    thread_local! {
        static TRACK: Cell<bool> = const { Cell::new(false) };
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
        static BYTES: Cell<usize> = const { Cell::new(0) };
        static LIVE: Cell<isize> = const { Cell::new(0) };
        static FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
    }
    struct Allocator;
    fn record(size: usize) {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.with(|n| n.set(n.get() + 1));
            BYTES.with(|n| n.set(n.get() + size));
        }
    }
    fn retained(size: usize) {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            LIVE.with(|n| n.set(n.get() + size as isize));
        }
    }
    fn should_fail() -> bool {
        FAIL_AFTER
            .try_with(|state| match state.get() {
                None => false,
                Some(0) => {
                    state.set(None);
                    true
                }
                Some(n) => {
                    state.set(Some(n - 1));
                    false
                }
            })
            .unwrap_or(false)
    }
    fn released(size: usize) {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            LIVE.with(|n| n.set(n.get() - size as isize));
        }
    }
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record(layout.size());
            if should_fail() {
                return std::ptr::null_mut();
            }
            let pointer = System.alloc(layout);
            if !pointer.is_null() {
                retained(layout.size());
            }
            pointer
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record(layout.size());
            if should_fail() {
                return std::ptr::null_mut();
            }
            let pointer = System.alloc_zeroed(layout);
            if !pointer.is_null() {
                retained(layout.size());
            }
            pointer
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            record(size);
            if should_fail() {
                return std::ptr::null_mut();
            }
            let pointer = System.realloc(p, layout, size);
            if !pointer.is_null() {
                released(layout.size());
                retained(size);
            }
            pointer
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            released(layout.size());
            System.dealloc(p, layout)
        }
    }
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;

    unsafe fn address(name: &CStr) -> usize {
        let address = android_bionic::rudroid_bionic_symbol(
            name.as_ptr(),
            c"libc.so".as_ptr(),
            c"LIBC".as_ptr(),
            std::ptr::null_mut(),
        );
        assert_ne!(address, 0, "missing {}", name.to_string_lossy());
        address
    }

    #[test]
    fn semaphore_and_numeric_calls_have_no_rust_hot_allocation() {
        unsafe {
            let init: unsafe extern "C" fn(*mut c_void,i32,u32)->i32=std::mem::transmute(address(c"sem_init"));
            let post: unsafe extern "C" fn(*mut c_void)->i32=std::mem::transmute(address(c"sem_post"));
            let wait: unsafe extern "C" fn(*mut c_void)->i32=std::mem::transmute(address(c"sem_trywait"));
            let destroy: unsafe extern "C" fn(*mut c_void)->i32=std::mem::transmute(address(c"sem_destroy"));
            let pow: unsafe extern "C" fn(f64,f64)->f64=std::mem::transmute(android_bionic::rudroid_bionic_symbol(c"pow".as_ptr(),c"libm.so".as_ptr(),c"LIBC".as_ptr(),std::ptr::null_mut()));
            let mut semaphore=[0usize;2]; let pointer=semaphore.as_mut_ptr().cast();
            assert_eq!(init(pointer,0,0),0);
            ALLOCS.with(|value| value.set(0)); BYTES.with(|value| value.set(0));
            let mut good=true; TRACK.with(|value| value.set(true));
            for _ in 0..10_000 {
                good &= post(pointer)==0; good &= wait(pointer)==0; good &= pow(2.,3.)==8.;
            }
            TRACK.with(|value| value.set(false));
            let allocations=ALLOCS.with(Cell::get); let bytes=BYTES.with(Cell::get);
            assert!(good); assert_eq!((allocations,bytes),(0,0)); assert_eq!(destroy(pointer),0);
            println!("BIONIC_SEM_MATH_COST calls=30000 rust_allocations={allocations} rust_requested_bytes={bytes}");
        }
    }

    #[test]
    fn basic_c_queries_and_calls_do_not_allocate_rust_objects() {
        unsafe {
            let copy: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> *mut c_void =
                std::mem::transmute(address(c"memcpy"));
            let bounded: unsafe extern "C" fn(*mut c_char, *const c_char, usize) -> usize =
                std::mem::transmute(address(c"strlcpy"));
            assert_eq!(address(c"strlen"), libc::strlen as *const () as usize);
            let mut buffer = [0u8; 16];
            ALLOCS.with(|value| value.set(0));
            BYTES.with(|value| value.set(0));
            let mut good = true;
            TRACK.with(|value| value.set(true));
            for _ in 0..10_000 {
                let length: unsafe extern "C" fn(*const c_char) -> usize =
                    std::mem::transmute(address(c"strlen"));
                good &= length(c"shared".as_ptr()) == 6;
                good &= copy(buffer.as_mut_ptr().cast(), c"shared".as_ptr().cast(), 7)
                    == buffer.as_mut_ptr().cast();
                good &= bounded(buffer.as_mut_ptr().cast(), c"shared".as_ptr(), 16) == 6;
            }
            TRACK.with(|value| value.set(false));
            assert!(good);
            let allocations = ALLOCS.with(Cell::get);
            let bytes = BYTES.with(Cell::get);
            assert_eq!((allocations, bytes), (0, 0));
            println!("BIONIC_BASIC_COST calls=30000 source_queries=10000 rust_allocations={allocations} rust_requested_bytes={bytes}");
        }
    }

    unsafe extern "C" fn count_destructor(argument: *mut c_void) {
        *argument.cast::<usize>() += 1;
    }

    #[test]
    fn unicode_state_tables_and_properties_have_no_rust_hot_allocation() {
        unsafe {
            let decode: unsafe extern "C" fn(*mut u32,*const c_char,usize,*mut u8)->usize =
                std::mem::transmute(address(c"mbrtowc"));
            let classify: unsafe extern "C" fn(u32,*mut c_void)->i32 =
                std::mem::transmute(address(c"iswalpha_l"));
            let maximum: unsafe extern "C" fn()->usize =
                std::mem::transmute(address(c"__ctype_get_mb_cur_max"));
            let property: unsafe extern "C" fn(*const c_char,*mut c_char)->i32 =
                std::mem::transmute(address(c"__system_property_get"));
            assert_ne!(classify(0x4e2d,std::ptr::null_mut()),0);
            let mut state=[0u8;8];
            let mut out=0u32;
            let mut value=[0 as c_char;92];
            ALLOCS.with(|v|v.set(0));
            BYTES.with(|v|v.set(0));
            TRACK.with(|v|v.set(true));
            let mut good=true;
            for _ in 0..10000 {
                good &= decode(&mut out,c"中".as_ptr(),3,state.as_mut_ptr())==3 && out==0x4e2d;
                good &= classify(out,std::ptr::null_mut())!=0;
                good &= maximum()==4;
                good &= property(c"ro.product.cpu.abi".as_ptr(),value.as_mut_ptr())==9;
            }
            TRACK.with(|v|v.set(false));
            assert!(good);
            assert_eq!((ALLOCS.with(Cell::get),BYTES.with(Cell::get)),(0,0));
            println!("BIONIC_UNICODE_COST calls=40000 rust_allocations=0 rust_requested_bytes=0");
        }
    }
    #[test]
    fn posix_clocks_resource_queries_and_repeated_log_identity_have_no_rust_allocation() {
        unsafe {
            let clock: unsafe extern "C" fn(i32, *mut libc::timespec) -> i32 =
                std::mem::transmute(address(c"clock_gettime"));
            let query: unsafe extern "C" fn(i32) -> i64 = std::mem::transmute(address(c"sysconf"));
            let openlog: unsafe extern "C" fn(*const c_char, i32, i32) =
                std::mem::transmute(address(c"openlog"));
            let closelog: unsafe extern "C" fn() = std::mem::transmute(address(c"closelog"));
            openlog(c"cost-probe".as_ptr(), 0, 8);
            let page = libc::sysconf(libc::_SC_PAGESIZE);
            let mut time = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            ALLOCS.with(|v| v.set(0));
            BYTES.with(|v| v.set(0));
            TRACK.with(|v| v.set(true));
            let mut good = true;
            for _ in 0..10000 {
                good &= clock(1, &mut time) == 0 && query(0x28) == page;
                openlog(c"cost-probe".as_ptr(), 0, 8);
            }
            TRACK.with(|v| v.set(false));
            assert!(good);
            assert_eq!((ALLOCS.with(Cell::get), BYTES.with(Cell::get)), (0, 0));
            closelog();
            println!("BIONIC_POSIX_COST calls=30000 rust_allocations=0 rust_requested_bytes=0");
        }
    }

    #[test]
    fn synchronization_hot_calls_do_not_allocate_rust_objects() {
        unsafe {
            type Operation = unsafe extern "C" fn(*mut c_void) -> i32;
            let lock: Operation = std::mem::transmute(address(c"pthread_mutex_lock"));
            let unlock: Operation = std::mem::transmute(address(c"pthread_mutex_unlock"));
            let read: Operation = std::mem::transmute(address(c"pthread_rwlock_rdlock"));
            let release: Operation = std::mem::transmute(address(c"pthread_rwlock_unlock"));
            let destroy: Operation = std::mem::transmute(address(c"pthread_mutex_destroy"));
            let destroy_rw: Operation = std::mem::transmute(address(c"pthread_rwlock_destroy"));
            let mut mutex = [0u32; 10];
            let mut rwlock = [0u32; 14];
            let m = mutex.as_mut_ptr().cast();
            let r = rwlock.as_mut_ptr().cast();
            assert_eq!(lock(m), 0);
            assert_eq!(unlock(m), 0);
            assert_eq!(read(r), 0);
            assert_eq!(release(r), 0);
            ALLOCS.with(|v| v.set(0));
            BYTES.with(|v| v.set(0));
            TRACK.with(|v| v.set(true));
            let mut good = true;
            for _ in 0..10000 {
                good &= lock(m) == 0 && unlock(m) == 0 && read(r) == 0 && release(r) == 0;
            }
            TRACK.with(|v| v.set(false));
            assert!(good);
            assert_eq!((ALLOCS.with(Cell::get), BYTES.with(Cell::get)), (0, 0));
            assert_eq!(destroy(m), 0);
            assert_eq!(destroy_rw(r), 0);
            println!("BIONIC_PTHREAD_COST calls=40000 rust_allocations=0 rust_requested_bytes=0");
        }
    }

    #[test]
    fn scoped_destructor_cycles_release_all_rust_registry_storage() {
        unsafe {
            type Callback = unsafe extern "C" fn(*mut c_void);
            let process: unsafe extern "C" fn(Option<Callback>, *mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(address(c"__cxa_atexit"));
            let thread: unsafe extern "C" fn(Option<Callback>, *mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(address(c"__cxa_thread_atexit_impl"));
            let cleanup = android_bionic::rudroid_bionic_finalize_range;
            let mut count = 0usize;
            let argument = (&mut count as *mut usize).cast::<c_void>();
            // Warm process hook/key initialization outside the measured cycles.
            assert_eq!(process(Some(count_destructor), argument, argument), 0);
            assert_eq!(thread(Some(count_destructor), argument, argument), 0);
            cleanup(argument as usize, 1);
            assert_eq!(count, 2);
            LIVE.with(|value| value.set(0));
            ALLOCS.with(|value| value.set(0));
            BYTES.with(|value| value.set(0));
            TRACK.with(|value| value.set(true));
            let mut good = true;
            for _ in 0..2_000 {
                good &= process(Some(count_destructor), argument, argument) == 0;
                good &= thread(Some(count_destructor), argument, argument) == 0;
                cleanup(argument as usize, 1);
            }
            TRACK.with(|value| value.set(false));
            assert!(good);
            assert_eq!(count, 4_002);
            assert_eq!(
                LIVE.with(Cell::get),
                0,
                "no registry Vec/Box remains after scoped cleanup"
            );
            println!(
                "BIONIC_DESTRUCTOR_COST cycles=2000 callbacks=4000 rust_live_bytes_after_cleanup=0"
            );
            // Injection happens only on this thread and inside the C registry
            // call: no test framework/string formatting runs with a failed allocator.
            for (register, successful_allocations) in [(process, 0), (thread, 0), (thread, 1)] {
                LIVE.with(|value| value.set(0));
                TRACK.with(|value| value.set(true));
                FAIL_AFTER.with(|value| value.set(Some(successful_allocations)));
                let result = register(Some(count_destructor), argument, argument);
                FAIL_AFTER.with(|value| value.set(None));
                TRACK.with(|value| value.set(false));
                assert_eq!(result, -1);
                assert_eq!(*libc::__errno_location(), libc::ENOMEM);
                assert_eq!(
                    LIVE.with(Cell::get),
                    0,
                    "failed registration must release a newly allocated payload"
                );
            }
            assert_eq!(
                count, 4_002,
                "failed registration cannot leave a runnable callback"
            );
            assert_eq!(process(Some(count_destructor), argument, argument), 0);
            assert_eq!(thread(Some(count_destructor), argument, argument), 0);
            cleanup(argument as usize, 1);
            assert_eq!(
                count, 4_004,
                "the registry must recover after allocation failure"
            );
            println!("BIONIC_DESTRUCTOR_OOM cases=3 result=ENOMEM live_bytes=0 recovery=passed");
        }
    }

    #[test]
    fn shared_streams_hot_io_and_close_recovery() {
        unsafe {
            let base = address(c"__sF");
            for (index, name) in [c"stdin", c"stdout", c"stderr"].into_iter().enumerate() {
                assert_eq!((address(name) as *const usize).read(), base + index * 152);
            }
            let open: unsafe extern "C" fn() -> *mut c_void =
                std::mem::transmute(address(c"tmpfile"));
            let close: unsafe extern "C" fn(*mut c_void) -> i32 =
                std::mem::transmute(address(c"fclose"));
            let fopen: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void =
                std::mem::transmute(address(c"fopen"));
            let fputs: unsafe extern "C" fn(*const c_char, *mut c_void) -> i32 =
                std::mem::transmute(address(c"fputs"));
            let fileno: unsafe extern "C" fn(*mut c_void) -> i32 =
                std::mem::transmute(address(c"fileno"));
            let error: unsafe extern "C" fn(*mut c_void) -> i32 =
                std::mem::transmute(address(c"ferror"));
            let tell: unsafe extern "C" fn(*mut c_void) -> i64 =
                std::mem::transmute(address(c"ftello"));
            let lock: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(address(c"flockfile"));
            let unlock: unsafe extern "C" fn(*mut c_void) =
                std::mem::transmute(address(c"funlockfile"));
            let trylock: unsafe extern "C" fn(*mut c_void) -> i32 =
                std::mem::transmute(address(c"ftrylockfile"));
            let file = open();
            assert!(!file.is_null());
            let fd = fileno(file);
            assert!(fd >= 0);
            assert_eq!(tell(file), 0);
            // No Rust-side exclusive lock may be held while waiting for a native
            // FILE lock: its owner must still be able to call the same adapter.
            lock(file);
            let pointer = file as usize; // C ABI handle only, retained until join.
            let worker = std::thread::spawn(move || {
                assert_ne!(trylock(pointer as *mut c_void), 0);
            });
            worker.join().unwrap();
            assert_eq!(fileno(file), fd);
            assert_eq!(trylock(file), 0);
            unlock(file);
            unlock(file);

            let mut good = true;
            TRACK.with(|value| value.set(true));
            for _ in 0..10_000 {
                good &= fileno(file) == fd && tell(file) == 0 && error(file) == 0;
            }
            TRACK.with(|value| value.set(false));
            let allocations = ALLOCS.with(Cell::get);
            let bytes = BYTES.with(Cell::get);
            assert!(good);
            assert_eq!((allocations, bytes), (0, 0));
            println!("BIONIC_STDIO_COST calls=30000 rust_allocations={allocations} rust_requested_bytes={bytes} facade_bytes=152");
            assert_eq!(close(file), 0);
            assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1, "native fd must close");

            let fd_count = || std::fs::read_dir("/proc/self/fd").unwrap().count();
            let before = fd_count();
            for _ in 0..2_000 {
                let file = open();
                assert!(!file.is_null());
                let fd = fileno(file);
                assert!(fd >= 0);
                assert_eq!(close(file), 0);
                assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1);
            }
            assert_eq!(fd_count(), before);
            let file = fopen(c"/dev/full".as_ptr(), c"w".as_ptr());
            assert!(!file.is_null(), "requires Linux /dev/full");
            let fd = fileno(file);
            assert!(fputs(c"buffered pending bytes".as_ptr(), file) >= 0);
            assert_eq!(close(file), -1, "fclose must report native flush failure");
            assert_eq!(*libc::__errno_location(), libc::ENOSPC);
            assert_eq!(
                libc::fcntl(fd, libc::F_GETFD),
                -1,
                "flush failure must still close the fd"
            );
            assert_eq!(fd_count(), before);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU native stdio; use docker-rust.sh test"]
fn shared_streams_hot_io_and_close_recovery() {
    panic!("run with docker-rust.sh test on Linux ARM64 GNU");
}
