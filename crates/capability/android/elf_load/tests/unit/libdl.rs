//! INPUT:  private items of `src/libdl.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/libdl.rs`; declared there with `#[path]`
use super::*;
#[test]
fn iterator_prefix_and_library_versions_are_explicit() {
    assert_eq!(std::mem::size_of::<PhdrInfo>(), 32);
    assert_eq!(std::mem::offset_of!(PhdrInfo, phnum), 24);
    assert_ne!(
        symbol(
            "dl_iterate_phdr",
            Some(SymbolVersion {
                file: "libdl.so",
                name: "LIBC"
            })
        ),
        0
    );
    for v in [
        SymbolVersion {
            file: "libc.so",
            name: "LIBC",
        },
        SymbolVersion {
            file: "libdl.so",
            name: "LIBC_PRIVATE",
        },
    ] {
        assert_eq!(symbol("dl_iterate_phdr", Some(v)), 0);
    }
    assert_ne!(symbol("dlopen", None), 0);
    assert_eq!(std::mem::size_of::<DlInfo>(), 32);
    assert_eq!(unsafe { iterate(None, std::ptr::null_mut()) }, -1);
}

#[test]
fn errors_are_per_thread_and_consumed_once() {
    unsafe {
        // No OpenSL implementation is deployed. A successful dummy dlopen
        // would trap FMOD on this backend instead of selecting AudioTrack.
        assert!(!synthetic(c"libOpenSLES.so"));
        assert!(open(c"libOpenSLES.so".as_ptr(), 1).is_null());
        assert!(!error().is_null());
        assert!(open(c"not-loaded.so".as_ptr(), 2).is_null());
        std::thread::spawn(|| {
            assert!(error().is_null());
            assert_eq!(close(std::ptr::null_mut()), -1);
            assert!(!error().is_null());
            assert!(error().is_null());
        })
        .join()
        .unwrap();
        assert!(!error().is_null());
        assert!(error().is_null());
        let process = open(std::ptr::null(), 2);
        assert!(!lookup(process, c"dlopen".as_ptr()).is_null());
        assert_eq!(close(process), 0);
        assert!(open(std::ptr::null(), 0x1000).is_null());
        assert!(!error().is_null());
    }
}
