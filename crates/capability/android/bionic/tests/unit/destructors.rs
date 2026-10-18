//! INPUT:  private items of `src/destructors.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/destructors.rs`; declared there with `#[path]`
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Observation {
    events: Mutex<Vec<usize>>,
}
static HOST_EXIT: AtomicUsize = AtomicUsize::new(0);
extern "C" fn host_exit() {
    HOST_EXIT.store(1, Ordering::Relaxed);
}
unsafe extern "C" fn record(pointer: *mut c_void) {
    let observation = &*pointer.cast::<Observation>();
    observation.events.lock().unwrap().push(1);
}
unsafe extern "C" fn thread_reenter(pointer: *mut c_void) {
    let observation = &*pointer.cast::<Observation>();
    observation.events.lock().unwrap().push(2);
    assert_eq!(register_thread(Some(record), pointer, pointer), 0);
    // Exercise nested range cleanup while the outer thread drainer owns
    // its payload. This must not free that payload out from under it.
    rudroid_bionic_finalize_range(0, usize::MAX);
}

#[test]
fn scoped_process_and_native_thread_cleanup_is_reentrant_and_guest_only() {
    let _serial = crate::LIFECYCLE_TEST_LOCK.lock().unwrap();
    unsafe {
        assert_eq!(libc::atexit(host_exit), 0);
        let observation = Arc::new(Observation {
            events: Mutex::new(Vec::new()),
        });
        let pointer = Arc::as_ptr(&observation) as *mut c_void;
        assert_eq!(register(None, pointer, pointer), -1);
        assert_eq!(*libc::__errno_location(), libc::EINVAL);
        assert_eq!(register(Some(record), pointer, pointer), 0);
        finalize(std::ptr::null_mut());
        finalize(pointer);
        assert_eq!(*observation.events.lock().unwrap(), [1]);
        assert_eq!(
            HOST_EXIT.load(Ordering::Relaxed),
            0,
            "guest null finalization cannot run host callbacks"
        );
        observation.events.lock().unwrap().clear();

        let retained = Arc::clone(&observation);
        let worker = std::thread::spawn(move || {
            let pointer = Arc::as_ptr(&retained) as *mut c_void;
            assert_eq!(register_thread(Some(record), pointer, pointer), 0);
            assert_eq!(register_thread(Some(thread_reenter), pointer, pointer), 0);
        });
        worker.join().unwrap(); // main Arc keeps arguments live through native key destruction
        assert_eq!(*observation.events.lock().unwrap(), [2, 1, 1]);
        assert_eq!(HOST_EXIT.load(Ordering::Relaxed), 0);
        assert!(lock().is_empty());
    }
}
