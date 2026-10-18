//! INPUT:  private items of `src/basic.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/basic.rs`; declared there with `#[path]`
use super::*;

#[test]
fn fortify_memory_imports_preserve_bounds_overlap_and_native_fatal_checks() {
    // SAFETY: exact LP64 native prototypes, initialized bounded spans, and
    // isolated fork children that only call libc and immediately abort/exit.
    unsafe {
        type Copy=unsafe extern "C" fn(*mut c_void,*const c_void,usize,usize)->*mut c_void;
        type Fill=unsafe extern "C" fn(*mut c_void,c_int,usize,usize)->*mut c_void;
        let copy:Copy=std::mem::transmute(symbol(c"__memcpy_chk"));let move_bytes:Copy=std::mem::transmute(symbol(c"__memmove_chk"));
        let fill:Fill=std::mem::transmute(symbol(c"__memset_chk"));
        let mut dest=[91u8;8];let p=dest.as_mut_ptr().add(1).cast();
        assert_eq!(copy(p,b"abc".as_ptr().cast(),3,3),p);assert_eq!(dest,b"[abc[[[["[..]);
        assert_eq!(move_bytes(dest.as_mut_ptr().add(2).cast(),dest.as_ptr().add(1).cast(),3,3),dest.as_mut_ptr().add(2).cast());
        assert_eq!(dest,b"[aabc[[["[..]);assert_eq!(fill(p,0x35,3,3),p);assert_eq!(dest,b"[555c[[["[..]);
        assert_eq!(copy(p,b"xyz".as_ptr().cast(),0,0),p);assert_eq!(dest,b"[555c[[["[..]);
        for mode in 0..3 {
            let pid=libc::fork();assert!(pid>=0);
            if pid==0 {
                // The deliberately rejected calls must not create a device core file.
                libc::prctl(libc::PR_SET_DUMPABLE,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong);
                if mode==0 {copy(p,b"abcd".as_ptr().cast(),4,3);}
                else if mode==1 {move_bytes(p,b"abcd".as_ptr().cast(),4,3);}
                else {fill(p,0,4,3);}
                libc::_exit(77);
            }
            let mut status=0;assert_eq!(libc::waitpid(pid,&mut status,0),pid);
            assert!(libc::WIFSIGNALED(status));assert_eq!(libc::WTERMSIG(status),libc::SIGABRT);
        }
    }
}

#[test]
fn bounded_strings_preserve_canaries_and_zero_size_does_not_touch_destination() {
    unsafe {
        let mut buffer = [0x5bu8; 8];
        assert_eq!(
            strlcpy(buffer.as_mut_ptr().add(1).cast(), c"123456".as_ptr(), 4),
            6
        );
        assert_eq!(&buffer, b"[123\0[[[");
        assert_eq!(strlcpy(std::ptr::null_mut(), c"text".as_ptr(), 0), 4);
        assert_eq!(strlcat(std::ptr::null_mut(), c"text".as_ptr(), 0), 4);
        buffer[1..5].copy_from_slice(b"a\0[[");
        assert_eq!(
            strlcat(buffer.as_mut_ptr().add(1).cast(), c"bcdef".as_ptr(), 4),
            6
        );
        assert_eq!(&buffer, b"[abc\0[[[");
        buffer[1..5].copy_from_slice(b"xxxx");
        assert_eq!(
            strlcat(buffer.as_mut_ptr().add(1).cast(), c"yz".as_ptr(), 4),
            6
        );
        assert_eq!(&buffer, b"[xxxx[[[");
    }
}

#[test]
fn errno_and_thread_identity_share_native_thread_state() {
    unsafe {
        let errno: unsafe extern "C" fn() -> *mut c_int =
            std::mem::transmute(symbol(c"__errno"));
        *errno() = 73;
        assert_eq!(errno(), libc::__errno_location());
        let main_tid = gettid();
        let worker_tid = std::thread::spawn(move || {
            *errno() = 84;
            assert_eq!(*libc::__errno_location(), 84);
            gettid()
        })
        .join()
        .unwrap();
        assert_ne!(main_tid, worker_tid);
        assert_eq!(*errno(), 73);
    }
}
