//! INPUT:  Android LP64 opaque layouts, native pthread and aligned atomic handles
//! OUTPUT:  Bionic mutex/cond/rwlock/semaphore/barrier/spin/once and attribute addresses
//! POS:    stores one host synchronization object per guest facade, with no hot registry

use std::ffi::{c_int, c_void};
use std::mem::{size_of, MaybeUninit};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

// Android 15 pthread_types.h: these objects are opaque, aligned to 4 bytes,
// and sized 40/48/56; sem_t is 16 bytes with no static initializer.
// Pthread attributes are long (8 bytes), not glibc's structures.
// Only first materialization uses this guard; no guard spans a native wait.
static INIT: Mutex<()> = Mutex::new(());
// Cold lifecycle inventory only; hot lock/wait calls never inspect it. Guest
// globals can disappear at ELF unmap without an explicit pthread_destroy.
static OWNED: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
struct Entry {
    guest: usize,
    native: usize,
    kind: Kind,
}
const DESTROYED: usize = 0xffff;
#[cfg(test)]
static LIVE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
#[cfg(test)]
std::thread_local! { static FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
std::thread_local! { static FAIL_RECORD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[derive(Clone, Copy)]
enum Kind {
    Mutex,
    Cond,
    Rwlock,
    Semaphore,
    Barrier,
    Spin,
}

impl Kind {
    fn guest_size(self) -> usize {
        match self {
            Self::Mutex => 40,
            Self::Cond => 48,
            Self::Rwlock => 56,
            Self::Semaphore => 16,
            // Android LP64: four i64 words, and one i64 word respectively.
            Self::Barrier => 32,
            Self::Spin => 8,
        }
    }
    fn native_size(self) -> usize {
        match self {
            Self::Mutex => size_of::<libc::pthread_mutex_t>(),
            Self::Cond => size_of::<libc::pthread_cond_t>(),
            Self::Rwlock => size_of::<libc::pthread_rwlock_t>(),
            Self::Semaphore => size_of::<libc::sem_t>(),
            Self::Barrier => size_of::<libc::pthread_barrier_t>(),
            Self::Spin => size_of::<libc::pthread_spinlock_t>(),
        }
    }
}

// The aligned word is wholly inside every guest layout even at base % 8 == 4.
// No Rust Box/Vec ownership is transferred: the payload is a native C object
// allocated by calloc, initialized/destroyed by pthread, and released by free.
unsafe fn slot<'a>(guest: *mut c_void) -> &'a AtomicUsize {
    let offset = (8 - (guest as usize & 7)) & 7;
    AtomicUsize::from_ptr(guest.cast::<u8>().add(offset).cast())
}

unsafe fn publish(guest: *mut c_void, kind: Kind, value: usize) {
    let offset = (8 - (guest as usize & 7)) & 7;
    // Explicit init/destroy require a quiescent object. Lazy init is serialized;
    // concurrent first-touch readers access only the atomic handle word.
    std::ptr::write_bytes(guest.cast::<u8>(), 0, offset);
    std::ptr::write_bytes(
        guest.cast::<u8>().add(offset + 8),
        0,
        kind.guest_size() - offset - 8,
    );
    slot(guest).store(value, Ordering::Release);
}

unsafe fn initialize(guest: *mut c_void, attr: isize, kind: Kind) -> c_int {
    if guest.is_null() {
        return libc::EINVAL;
    }
    let shared = match kind {
        Kind::Mutex => 0x10,
        _ => 1,
    };
    let explicit = matches!(kind, Kind::Semaphore | Kind::Barrier | Kind::Spin);
    if !explicit && attr & shared != 0 {
        return libc::ENOTSUP;
    }
    let allowed = match kind {
        Kind::Mutex => 0x3f,
        _ => 3,
    };
    if !explicit && (attr & !allowed != 0 || matches!(kind, Kind::Mutex) && attr & 15 > 2) {
        return libc::EINVAL;
    }
    #[cfg(test)]
    if FAIL.with(|fail| fail.replace(false)) {
        return libc::ENOMEM;
    }
    let native = libc::calloc(1, kind.native_size());
    if native.is_null() {
        return libc::ENOMEM;
    }
    let result = match kind {
        // SAFETY: native C allocations have native alignment/size; the explicit
        // public wrappers validate count/pshared before calling this initializer.
        Kind::Barrier => libc::pthread_barrier_init(native.cast(), std::ptr::null(), attr as u32),
        Kind::Spin => libc::pthread_spin_init(native.cast(), libc::PTHREAD_PROCESS_PRIVATE),
        Kind::Semaphore => {
            if libc::sem_init(native.cast(), 0, attr as u32) == 0 { 0 }
            else { *libc::__errno_location() }
        }
        Kind::Mutex => {
            let mut host = MaybeUninit::<libc::pthread_mutexattr_t>::uninit();
            let host = host.as_mut_ptr();
            let mut result = libc::pthread_mutexattr_init(host);
            if result == 0 {
                result = libc::pthread_mutexattr_settype(host, (attr & 15) as c_int);
                if result == 0 {
                    result = libc::pthread_mutexattr_setprotocol(host, ((attr >> 5) & 1) as c_int);
                }
                if result == 0 {
                    result = libc::pthread_mutex_init(native.cast(), host);
                }
                libc::pthread_mutexattr_destroy(host);
            }
            result
        }
        Kind::Cond => {
            let mut host = MaybeUninit::<libc::pthread_condattr_t>::uninit();
            let host = host.as_mut_ptr();
            let mut result = libc::pthread_condattr_init(host);
            if result == 0 {
                result = libc::pthread_condattr_setclock(host, ((attr >> 1) & 1) as c_int);
                if result == 0 {
                    result = libc::pthread_cond_init(native.cast(), host);
                }
                libc::pthread_condattr_destroy(host);
            }
            result
        }
        Kind::Rwlock => {
            let mut host = MaybeUninit::<libc::pthread_rwlockattr_t>::uninit();
            let host = host.as_mut_ptr();
            let mut result = libc::pthread_rwlockattr_init(host);
            if result == 0 {
                // Android writer-nonrecursive = 1; GNU writer-nonrecursive = 2.
                result =
                    libc::pthread_rwlockattr_setkind_np(host, if attr & 2 != 0 { 2 } else { 0 });
                if result == 0 {
                    result = libc::pthread_rwlock_init(native.cast(), host);
                }
                libc::pthread_rwlockattr_destroy(host);
            }
            result
        }
    };
    if result == 0 {
        let mut records = OWNED.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(test)]
        let fail_record = FAIL_RECORD.with(|fail| fail.replace(false));
        #[cfg(not(test))]
        let fail_record = false;
        if fail_record || records.try_reserve(1).is_err() {
            native_destroy(native, kind);
            libc::free(native);
            return libc::ENOMEM;
        }
        records.push(Entry {
            guest: guest as usize,
            native: native as usize,
            kind,
        });
        #[cfg(test)]
        LIVE.fetch_add(kind.native_size() as isize, Ordering::Relaxed);
        publish(guest, kind, native as usize);
    } else {
        libc::free(native);
    }
    result
}

unsafe fn native_destroy(native: *mut c_void, kind: Kind) -> c_int {
    match kind {
        // SAFETY: each recorded payload was initialized as exactly this native kind.
        Kind::Barrier => libc::pthread_barrier_destroy(native.cast()),
        Kind::Spin => libc::pthread_spin_destroy(native.cast()),
        Kind::Mutex => libc::pthread_mutex_destroy(native.cast()),
        Kind::Cond => libc::pthread_cond_destroy(native.cast()),
        Kind::Rwlock => libc::pthread_rwlock_destroy(native.cast()),
        Kind::Semaphore => {
            if libc::sem_destroy(native.cast()) == 0 { 0 } else { *libc::__errno_location() }
        },
    }
}

unsafe fn ensure(guest: *mut c_void, kind: Kind) -> Result<*mut c_void, c_int> {
    if guest.is_null() {
        return Err(libc::EINVAL);
    }
    let mut handle = slot(guest).load(Ordering::Acquire);
    if handle == DESTROYED {
        return Err(libc::EINVAL);
    }
    if handle >= 0x10000 {
        return Ok(handle as *mut c_void);
    }
    if matches!(kind, Kind::Semaphore | Kind::Barrier | Kind::Spin) { return Err(libc::EINVAL); }
    let _guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
    handle = slot(guest).load(Ordering::Acquire);
    if handle == DESTROYED {
        return Err(libc::EINVAL);
    }
    if handle < 0x10000 {
        let state = guest.cast::<u32>().read();
        let attr = match kind {
            Kind::Mutex if state & !0xe000 == 0 => {
                ((state >> 14) & 3) as isize | if state & 0x2000 != 0 { 0x10 } else { 0 }
            }
            Kind::Cond if state & !3 == 0 => state as isize,
            Kind::Rwlock if state == 0 => 0,
            _ => return Err(libc::EINVAL),
        };
        let result = initialize(guest, attr, kind);
        if result != 0 {
            return Err(result);
        }
        handle = slot(guest).load(Ordering::Acquire);
    }
    Ok(handle as *mut c_void)
}

unsafe fn destroy(guest: *mut c_void, kind: Kind) -> c_int {
    if guest.is_null() {
        return libc::EINVAL;
    }
    let handle = slot(guest).load(Ordering::Acquire);
    if handle == DESTROYED {
        return libc::EINVAL;
    }
    if handle >= 0x10000 {
        let result = native_destroy(handle as *mut c_void, kind);
        if result != 0 {
            return result;
        }
        libc::free(handle as *mut c_void);
        let mut records = OWNED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = records
            .iter()
            .position(|e| e.guest == guest as usize && e.native == handle)
        {
            records.swap_remove(index);
        }
        if records.is_empty() {
            *records = Vec::new();
        }
        #[cfg(test)]
        LIVE.fetch_sub(kind.native_size() as isize, Ordering::Relaxed);
    }
    publish(guest, kind, DESTROYED);
    0
}

/// Called after guest destructors and before munmap. All users of objects in
/// this image must already be quiescent/joined, as required by pthread_destroy.
pub(crate) unsafe fn finalize_range(start: usize, length: usize) {
    let mut records = OWNED.lock().unwrap_or_else(|e| e.into_inner());
    let mut index = 0;
    while index < records.len() {
        let entry = &records[index];
        if entry.guest >= start && entry.guest - start < length {
            if native_destroy(entry.native as *mut c_void, entry.kind) == 0 {
                libc::free(entry.native as *mut c_void);
                publish(entry.guest as *mut c_void, entry.kind, DESTROYED);
                #[cfg(test)]
                LIVE.fetch_sub(entry.kind.native_size() as isize, Ordering::Relaxed);
                records.swap_remove(index);
                continue;
            }
        }
        index += 1;
    }
    if records.is_empty() {
        *records = Vec::new();
    }
}

fn sem_error(code: c_int) -> c_int {
    // SAFETY: glibc's errno location is valid for the calling thread.
    if code == 0 { 0 } else { unsafe { *libc::__errno_location() = code; } -1 }
}
unsafe extern "C" fn semaphore_init(guest: *mut c_void, shared: c_int, value: u32) -> c_int {
    if shared != 0 { return sem_error(libc::ENOTSUP); }
    if value > c_int::MAX as u32 { return sem_error(libc::EINVAL); }
    sem_error(initialize(guest, value as isize, Kind::Semaphore))
}
unsafe extern "C" fn semaphore_destroy(guest: *mut c_void) -> c_int {
    sem_error(destroy(guest, Kind::Semaphore))
}
macro_rules! semaphore_operation {
    ($name:ident,$native:ident) => {
        unsafe extern "C" fn $name(guest: *mut c_void) -> c_int {
            match ensure(guest, Kind::Semaphore) {
                Ok(pointer) => libc::$native(pointer.cast()), Err(code) => sem_error(code),
            }
        }
    };
}
semaphore_operation!(semaphore_post,sem_post);
semaphore_operation!(semaphore_wait,sem_wait);
semaphore_operation!(semaphore_trywait,sem_trywait);
unsafe extern "C" fn semaphore_getvalue(guest: *mut c_void, value: *mut c_int) -> c_int {
    match ensure(guest, Kind::Semaphore) {
        Ok(pointer) => libc::sem_getvalue(pointer.cast(), value), Err(code) => sem_error(code),
    }
}
unsafe extern "C" fn semaphore_timedwait(guest: *mut c_void, deadline: *const libc::timespec) -> c_int {
    match ensure(guest, Kind::Semaphore) {
        Ok(pointer) => libc::sem_timedwait(pointer.cast(), deadline), Err(code) => sem_error(code),
    }
}

macro_rules! lifecycle {
    ($init:ident, $destroy:ident, $kind:ident) => {
        unsafe extern "C" fn $init(guest: *mut c_void, attr: *const isize) -> c_int {
            initialize(
                guest,
                if attr.is_null() { 0 } else { attr.read() },
                Kind::$kind,
            )
        }
        unsafe extern "C" fn $destroy(guest: *mut c_void) -> c_int {
            destroy(guest, Kind::$kind)
        }
    };
}
lifecycle!(mutex_init, mutex_destroy, Mutex);
lifecycle!(cond_init, cond_destroy, Cond);
lifecycle!(rwlock_init, rwlock_destroy, Rwlock);

macro_rules! operation {
    ($name:ident, $native:ident, $kind:ident) => {
        unsafe extern "C" fn $name(guest: *mut c_void) -> c_int {
            match ensure(guest, Kind::$kind) {
                Ok(p) => libc::$native(p.cast()),
                Err(e) => e,
            }
        }
    };
}
operation!(mutex_lock, pthread_mutex_lock, Mutex);
operation!(mutex_trylock, pthread_mutex_trylock, Mutex);
operation!(mutex_unlock, pthread_mutex_unlock, Mutex);
operation!(cond_signal, pthread_cond_signal, Cond);
operation!(cond_broadcast, pthread_cond_broadcast, Cond);
operation!(rwlock_rdlock, pthread_rwlock_rdlock, Rwlock);
operation!(rwlock_wrlock, pthread_rwlock_wrlock, Rwlock);
operation!(rwlock_tryrdlock, pthread_rwlock_tryrdlock, Rwlock);
operation!(rwlock_trywrlock, pthread_rwlock_trywrlock, Rwlock);
operation!(rwlock_unlock, pthread_rwlock_unlock, Rwlock);
operation!(spin_lock, pthread_spin_lock, Spin);
operation!(spin_trylock, pthread_spin_trylock, Spin);
operation!(spin_unlock, pthread_spin_unlock, Spin);

/// # Safety
/// guest is a writable 32-byte, 8-aligned Android LP64 barrier, quiescent for
/// initialization. A non-null attr lends exactly one initialized 32-bit int.
unsafe extern "C" fn barrier_init(guest:*mut c_void,attr:*const c_int,count:u32)->c_int {
    // SAFETY: caller lends the scalar attribute; no host pthread layout is exposed to guest.
    unsafe {
        if guest.is_null() || count==0 {return libc::EINVAL;}
        let shared=if attr.is_null() {0} else {*attr};
        if shared!=0 {return if shared==1 {libc::ENOTSUP} else {libc::EINVAL};}
        initialize(guest,count as isize,Kind::Barrier)
    }
}
/// # Safety
/// Initialized Android barrier; all participants have returned/joined before destruction.
unsafe extern "C" fn barrier_destroy(guest:*mut c_void)->c_int {
    // SAFETY: caller establishes quiescence; shared native-payload retirement runs once.
    unsafe {destroy(guest,Kind::Barrier)}
}
/// # Safety
/// Initialized live Android barrier, kept alive until every native wait returns.
unsafe extern "C" fn barrier_wait(guest:*mut c_void)->c_int {
    // SAFETY: one native payload owns blocking/wakeup state; no Rust lock spans wait.
    unsafe {match ensure(guest,Kind::Barrier) {
        Ok(p)=>{let result=libc::pthread_barrier_wait(p.cast());if result==libc::PTHREAD_BARRIER_SERIAL_THREAD {-1} else {result}},Err(e)=>e,
    }}
}
/// # Safety
/// guest is a writable 8-byte, 8-aligned Android LP64 spin facade, quiescent at init.
unsafe extern "C" fn spin_init(guest:*mut c_void,shared:c_int)->c_int {
    if guest.is_null() {return libc::EINVAL;}
    if shared!=0 {return if shared==1 {libc::ENOTSUP} else {libc::EINVAL};}
    // SAFETY: guest facade reserves one aligned handle; native spin state lives on the C heap.
    unsafe {initialize(guest,0,Kind::Spin)}
}
/// # Safety
/// Initialized spin facade; unlocked and quiescent before destruction.
unsafe extern "C" fn spin_destroy(guest:*mut c_void)->c_int {
    // SAFETY: follows the native pthread lifetime contract and existing owner ledger.
    unsafe {destroy(guest,Kind::Spin)}
}
/// # Safety
/// attr lends one writable Android barrierattr int. No isize-sized writes are allowed.
unsafe extern "C" fn barrierattr_init(attr:*mut c_int)->c_int {
    if attr.is_null() {return libc::EINVAL;}
    // SAFETY: exactly one caller-owned 32-bit word; neighboring canaries stay untouched.
    unsafe {*attr=0;}0
}
/// # Safety
/// attr lends one writable initialized Android barrierattr int, no concurrent users.
unsafe extern "C" fn barrierattr_destroy(attr:*mut c_int)->c_int {
    // SAFETY: the scalar facade owns no heap object or host attribute.
    unsafe {barrierattr_init(attr)}
}
/// # Safety
/// attr and out each lend one initialized/readable and writable 32-bit word respectively.
unsafe extern "C" fn barrierattr_getpshared(attr:*const c_int,out:*mut c_int)->c_int {
    if attr.is_null() || out.is_null() {return libc::EINVAL;}
    // SAFETY: bounded scalar read/write, native API returns ordinary error numbers.
    unsafe {let v=*attr;if !matches!(v,0|1) {return libc::EINVAL;}*out=v;}0
}
/// # Safety
/// attr lends one writable Android barrierattr int for this scalar update.
unsafe extern "C" fn barrierattr_setpshared(attr:*mut c_int,value:c_int)->c_int {
    if attr.is_null() || !matches!(value,0|1) {return libc::EINVAL;}
    // SAFETY: one bounded scalar write. Shared attributes are representable, while init
    // explicitly rejects a process-shared object whose native heap handle cannot be shared.
    unsafe {*attr=value;}0
}

macro_rules! timed_operation {
    ($name:ident, $native:path, $kind:ident) => {
        unsafe extern "C" fn $name(guest: *mut c_void, deadline: *const libc::timespec) -> c_int {
            if deadline.is_null() {
                return libc::EINVAL;
            }
            match ensure(guest, Kind::$kind) {
                Ok(p) => $native(p.cast(), deadline),
                Err(e) => e,
            }
        }
    };
}
// LP64 timespec is two signed 64-bit scalars on both ABIs. These legacy timed
// APIs avoid GLIBC_2.30 pthread_*clock* symbols on older handheld firmware.
timed_operation!(mutex_timedlock, libc::pthread_mutex_timedlock, Mutex);
timed_operation!(rwlock_timedrdlock, pthread_rwlock_timedrdlock, Rwlock);
timed_operation!(rwlock_timedwrlock, pthread_rwlock_timedwrlock, Rwlock);

unsafe extern "C" fn cond_wait(cond: *mut c_void, mutex: *mut c_void) -> c_int {
    let c = match ensure(cond, Kind::Cond) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let m = match ensure(mutex, Kind::Mutex) {
        Ok(p) => p,
        Err(e) => return e,
    };
    libc::pthread_cond_wait(c.cast(), m.cast())
}
unsafe extern "C" fn cond_timedwait(
    cond: *mut c_void,
    mutex: *mut c_void,
    deadline: *const libc::timespec,
) -> c_int {
    if deadline.is_null() {
        return libc::EINVAL;
    }
    let c = match ensure(cond, Kind::Cond) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let m = match ensure(mutex, Kind::Mutex) {
        Ok(p) => p,
        Err(e) => return e,
    };
    libc::pthread_cond_timedwait(c.cast(), m.cast(), deadline)
}

unsafe extern "C" fn attr_init(attr: *mut isize) -> c_int {
    if attr.is_null() {
        return libc::EINVAL;
    }
    attr.write(0);
    0
}
unsafe extern "C" fn attr_destroy(attr: *mut isize) -> c_int {
    if attr.is_null() {
        return libc::EINVAL;
    }
    attr.write(-1);
    0
}
macro_rules! attribute {
    ($get:ident, $set:ident, $mask:expr, $shift:expr, $max:expr) => {
        unsafe extern "C" fn $get(attr: *const isize, value: *mut c_int) -> c_int {
            if attr.is_null() || value.is_null() {
                return libc::EINVAL;
            }
            let field = ((attr.read() & $mask) >> $shift) as c_int;
            if field > $max {
                return libc::EINVAL;
            }
            value.write(field);
            0
        }
        unsafe extern "C" fn $set(attr: *mut isize, value: c_int) -> c_int {
            if attr.is_null() || value < 0 || value > $max {
                return libc::EINVAL;
            }
            attr.write((attr.read() & !$mask) | ((value as isize) << $shift));
            0
        }
    };
}
attribute!(mutexattr_gettype, mutexattr_settype, 15, 0, 2);
attribute!(mutexattr_getpshared, mutexattr_setpshared, 16, 4, 1);
attribute!(mutexattr_getprotocol, mutexattr_setprotocol, 32, 5, 1);
attribute!(condattr_getpshared, condattr_setpshared, 1, 0, 1);
attribute!(condattr_getclock, condattr_setclock, 2, 1, 1);
attribute!(rwlockattr_getpshared, rwlockattr_setpshared, 1, 0, 1);
attribute!(rwlockattr_getkind, rwlockattr_setkind, 2, 1, 1);

unsafe extern "C" {
    // Both once objects are 4-byte integers with zero initial state. All guest
    // accesses go through native pthread_once; its cancellation/fork handling
    // remains native too. No custom futex or callback-owning Rust state.
    fn pthread_rwlock_timedrdlock(
        lock: *mut libc::pthread_rwlock_t,
        deadline: *const libc::timespec,
    ) -> c_int;
    fn pthread_rwlock_timedwrlock(
        lock: *mut libc::pthread_rwlock_t,
        deadline: *const libc::timespec,
    ) -> c_int;
    fn pthread_once(control: *mut c_int, init: unsafe extern "C" fn()) -> c_int;
}

symbol_provider! {
    b"pthread_barrier_init" => barrier_init as *const () as usize,
    b"pthread_barrier_destroy" => barrier_destroy as *const () as usize,
    b"pthread_barrier_wait" => barrier_wait as *const () as usize,
    b"pthread_barrierattr_init" => barrierattr_init as *const () as usize,
    b"pthread_barrierattr_destroy" => barrierattr_destroy as *const () as usize,
    b"pthread_barrierattr_getpshared" => barrierattr_getpshared as *const () as usize,
    b"pthread_barrierattr_setpshared" => barrierattr_setpshared as *const () as usize,
    b"pthread_spin_init" => spin_init as *const () as usize,
    b"pthread_spin_destroy" => spin_destroy as *const () as usize,
    b"pthread_spin_lock" => spin_lock as *const () as usize,
    b"pthread_spin_trylock" => spin_trylock as *const () as usize,
    b"pthread_spin_unlock" => spin_unlock as *const () as usize,
    b"sem_init" => semaphore_init as *const () as usize,
    b"sem_destroy" => semaphore_destroy as *const () as usize,
    b"sem_post" => semaphore_post as *const () as usize,
    b"sem_wait" => semaphore_wait as *const () as usize,
    b"sem_trywait" => semaphore_trywait as *const () as usize,
    b"sem_getvalue" => semaphore_getvalue as *const () as usize,
    b"sem_timedwait" => semaphore_timedwait as *const () as usize,
    b"pthread_mutex_init" => mutex_init as *const () as usize,
    b"pthread_mutex_destroy" => mutex_destroy as *const () as usize,
    b"pthread_mutex_lock" => mutex_lock as *const () as usize,
    b"pthread_mutex_trylock" => mutex_trylock as *const () as usize,
    b"pthread_mutex_unlock" => mutex_unlock as *const () as usize,
    b"pthread_mutex_timedlock" => mutex_timedlock as *const () as usize,
    b"pthread_cond_init" => cond_init as *const () as usize,
    b"pthread_cond_destroy" => cond_destroy as *const () as usize,
    b"pthread_cond_signal" => cond_signal as *const () as usize,
    b"pthread_cond_broadcast" => cond_broadcast as *const () as usize,
    b"pthread_cond_wait" => cond_wait as *const () as usize,
    b"pthread_cond_timedwait" => cond_timedwait as *const () as usize,
    b"pthread_rwlock_init" => rwlock_init as *const () as usize,
    b"pthread_rwlock_destroy" => rwlock_destroy as *const () as usize,
    b"pthread_rwlock_rdlock" => rwlock_rdlock as *const () as usize,
    b"pthread_rwlock_wrlock" => rwlock_wrlock as *const () as usize,
    b"pthread_rwlock_tryrdlock" => rwlock_tryrdlock as *const () as usize,
    b"pthread_rwlock_trywrlock" => rwlock_trywrlock as *const () as usize,
    b"pthread_rwlock_unlock" => rwlock_unlock as *const () as usize,
    b"pthread_rwlock_timedrdlock" => rwlock_timedrdlock as *const () as usize,
    b"pthread_rwlock_timedwrlock" => rwlock_timedwrlock as *const () as usize,
    b"pthread_mutexattr_init" | b"pthread_condattr_init" | b"pthread_rwlockattr_init" => {
        attr_init as *const () as usize
    },
    b"pthread_mutexattr_destroy"
    | b"pthread_condattr_destroy"
    | b"pthread_rwlockattr_destroy" => attr_destroy as *const () as usize,
    b"pthread_mutexattr_gettype" => mutexattr_gettype as *const () as usize,
    b"pthread_mutexattr_settype" => mutexattr_settype as *const () as usize,
    b"pthread_mutexattr_getpshared" => mutexattr_getpshared as *const () as usize,
    b"pthread_mutexattr_setpshared" => mutexattr_setpshared as *const () as usize,
    b"pthread_mutexattr_getprotocol" => mutexattr_getprotocol as *const () as usize,
    b"pthread_mutexattr_setprotocol" => mutexattr_setprotocol as *const () as usize,
    b"pthread_condattr_getclock" => condattr_getclock as *const () as usize,
    b"pthread_condattr_setclock" => condattr_setclock as *const () as usize,
    b"pthread_condattr_getpshared" => condattr_getpshared as *const () as usize,
    b"pthread_condattr_setpshared" => condattr_setpshared as *const () as usize,
    b"pthread_rwlockattr_getkind_np" => rwlockattr_getkind as *const () as usize,
    b"pthread_rwlockattr_setkind_np" => rwlockattr_setkind as *const () as usize,
    b"pthread_rwlockattr_getpshared" => rwlockattr_getpshared as *const () as usize,
    b"pthread_rwlockattr_setpshared" => rwlockattr_setpshared as *const () as usize,
    b"pthread_once" => pthread_once as *const () as usize,
}

#[cfg(test)]
#[path = "../tests/unit/sync.rs"]
mod tests;
