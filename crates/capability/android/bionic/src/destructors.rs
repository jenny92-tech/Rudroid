//! INPUT:  guest C++ callbacks/DSO addresses and native pthread key/atexit services
//! OUTPUT:  scoped C++ finalizers and pre-unmap callback/synchronization cleanup
//! POS:    guest destructor registry never hands an Android DSO to glibc's linker

use std::ffi::{c_int, c_void};
use std::sync::{Mutex, OnceLock};

type Callback = unsafe extern "C" fn(*mut c_void);
struct Entry {
    callback: Callback,
    // Opaque guest ABI addresses, not Rust references or owned Rust objects.
    // The caller retains the argument until this callback executes.
    argument: usize,
    dso: usize,
}
static PROCESS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static EXIT: OnceLock<c_int> = OnceLock::new();
static THREAD_KEY: OnceLock<Result<libc::pthread_key_t, c_int>> = OnceLock::new();

fn lock() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    PROCESS.lock().unwrap_or_else(|e| e.into_inner())
}

extern "C" fn process_exit() {
    // SAFETY: process exit; guest images stay mapped while their callbacks run.
    unsafe {
        finalize_thread(|_| true);
        drain_process(|_| true);
    }
}

fn exit_ready() -> bool {
    // SAFETY: `process_exit` is a plain `extern "C" fn()` with static lifetime.
    *EXIT.get_or_init(|| unsafe { libc::atexit(process_exit) }) == 0
}

unsafe fn run(entry: Entry) {
    // C++ ABI: executable callback and argument remain live through finalize.
    // Pop before calling; callbacks may register/finalize other callbacks.
    (entry.callback)(entry.argument as *mut c_void);
}

unsafe fn drain_process(matches: impl Fn(&Entry) -> bool) {
    loop {
        let next = {
            let mut entries = lock();
            let next = entries
                .iter()
                .rposition(&matches)
                .map(|i| entries.remove(i));
            if entries.is_empty() {
                *entries = Vec::new();
            }
            next
        };
        let Some(entry) = next else {
            break;
        };
        run(entry); // never hold the process registry lock across guest code
    }
}

pub(crate) unsafe extern "C" fn register(
    callback: Option<Callback>,
    argument: *mut c_void,
    dso: *mut c_void,
) -> c_int {
    let Some(callback) = callback else {
        *libc::__errno_location() = libc::EINVAL;
        return -1;
    };
    if !exit_ready() {
        *libc::__errno_location() = libc::ENOMEM;
        return -1;
    }
    let mut entries = lock();
    if entries.try_reserve(1).is_err() {
        *libc::__errno_location() = libc::ENOMEM;
        return -1;
    }
    entries.push(Entry {
        callback,
        argument: argument as usize,
        dso: dso as usize,
    });
    0
}

pub(crate) unsafe extern "C" fn finalize(dso: *mut c_void) {
    drain_process(|entry| dso.is_null() || entry.dso == dso as usize);
    unsafe extern "C" { fn rudroid_bionic_atfork_finalize_dso(dso:*mut c_void); }
    rudroid_bionic_atfork_finalize_dso(dso);
}

fn thread_key() -> Result<libc::pthread_key_t, c_int> {
    // SAFETY: `key` is a valid out-param; `thread_exit` matches the destructor ABI.
    *THREAD_KEY.get_or_init(|| unsafe {
        let mut key = 0;
        let error = libc::pthread_key_create(&mut key, Some(thread_exit));
        if error == 0 {
            Ok(key)
        } else {
            Err(error)
        }
    })
}

struct ThreadEntries {
    entries: Vec<Entry>,
    // Reentrant range finalization must not free the outer drainer's payload.
    draining: usize,
}

unsafe fn try_thread_payload() -> Option<Box<ThreadEntries>> {
    // Box::new aborts on OOM, while this C ABI must report ENOMEM. Allocate
    // with the SAME Rust global allocator/layout that Box will deallocate;
    // initialize before forming the sole Box owner. This never uses C malloc
    // to create a Rust object, nor transfers Rust ownership to guest code.
    let pointer =
        std::alloc::alloc(std::alloc::Layout::new::<ThreadEntries>()).cast::<ThreadEntries>();
    if pointer.is_null() {
        return None;
    }
    pointer.write(ThreadEntries {
        entries: Vec::new(),
        draining: 0,
    });
    Some(Box::from_raw(pointer))
}

unsafe extern "C" fn thread_exit(payload: *mut c_void) {
    let pointer = payload.cast::<ThreadEntries>();
    let Ok(key) = thread_key() else {
        return;
    };
    // pthread clears its key before invoking us. Restore our exclusively
    // owned payload so reentrant C++ registrations use the same vector;
    // only short borrows occur, and none survives a guest callback.
    let restored = libc::pthread_setspecific(key, payload) == 0;
    (*pointer).draining += 1;
    loop {
        let next = (*pointer).entries.pop();
        let Some(entry) = next else {
            break;
        };
        run(entry);
    }
    if restored && libc::pthread_setspecific(key, std::ptr::null()) != 0 {
        // Keep ownership with the key if clearing ever fails; a subsequent
        // native destructor iteration may retry, but must never see a freed Box.
        (*pointer).draining -= 1;
        return;
    }
    // The native key owned this Rust Box, never the guest. It was allocated
    // by register_thread; reclaim exactly once after all callbacks return.
    drop(Box::from_raw(pointer));
}

pub(crate) unsafe extern "C" fn register_thread(
    callback: Option<Callback>,
    argument: *mut c_void,
    dso: *mut c_void,
) -> c_int {
    let Some(callback) = callback else {
        *libc::__errno_location() = libc::EINVAL;
        return -1;
    };
    if !exit_ready() {
        *libc::__errno_location() = libc::ENOMEM;
        return -1;
    }
    let key = match thread_key() {
        Ok(key) => key,
        Err(error) => {
            *libc::__errno_location() = error;
            return -1;
        }
    };
    let mut pointer = libc::pthread_getspecific(key).cast::<ThreadEntries>();
    let created = pointer.is_null();
    if pointer.is_null() {
        let Some(payload) = try_thread_payload() else {
            *libc::__errno_location() = libc::ENOMEM;
            return -1;
        };
        pointer = Box::into_raw(payload);
        let error = libc::pthread_setspecific(key, pointer.cast());
        if error != 0 {
            drop(Box::from_raw(pointer));
            *libc::__errno_location() = error;
            return -1;
        }
    }
    // Only the owning native thread accesses its payload.
    if (*pointer).entries.try_reserve(1).is_err() {
        if created && libc::pthread_setspecific(key, std::ptr::null()) == 0 {
            drop(Box::from_raw(pointer));
        }
        *libc::__errno_location() = libc::ENOMEM;
        return -1;
    }
    (*pointer).entries.push(Entry {
        callback,
        argument: argument as usize,
        dso: dso as usize,
    });
    0
}

unsafe fn finalize_thread(matches: impl Fn(&Entry) -> bool) {
    let Some(Ok(key)) = THREAD_KEY.get() else {
        return;
    };
    let pointer = libc::pthread_getspecific(*key).cast::<ThreadEntries>();
    if pointer.is_null() {
        return;
    }
    (*pointer).draining += 1;
    loop {
        let next = {
            let entries = &mut (*pointer).entries;
            entries
                .iter()
                .rposition(&matches)
                .map(|i| entries.remove(i))
        };
        let Some(entry) = next else {
            break;
        };
        run(entry);
    }
    (*pointer).draining -= 1;
    if (*pointer).draining == 0
        && (*pointer).entries.is_empty()
        && libc::pthread_setspecific(*key, std::ptr::null()) == 0
    {
        drop(Box::from_raw(pointer));
    }
}

/// # Safety
/// Called before unmapping a quiescent image. All other guest threads and
/// concurrent finalizers that can use this image must have completed/joined.
/// callback/DSO addresses inside the range are still executable/readable.
#[no_mangle]
pub unsafe extern "C" fn rudroid_bionic_finalize_range(start: usize, length: usize) {
    let in_range = |address: usize| address >= start && address - start < length;
    let matches = |entry: &Entry| in_range(entry.dso) || in_range(entry.callback as usize);
    finalize_thread(matches);
    drain_process(matches);
    unsafe extern "C" {
        fn rudroid_bionic_atfork_finalize_range(start: usize,length:usize);
    }
    rudroid_bionic_atfork_finalize_range(start,length);
    crate::sync::finalize_range(start, length);
}

#[cfg(test)]
#[path = "../tests/unit/destructors.rs"]
mod tests;
