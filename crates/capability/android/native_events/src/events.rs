//! INPUT:  libc's gated epoll/eventfd/pthread ABI and caller-owned NDK handles/callbacks
//! OUTPUT:  strict ALooper/ASensor addresses and fallible thread Looper preparation
//! POS:    one Rust event owner, indexed readiness lookup and allocation-free polling
use std::{
    alloc::{alloc, Layout}, cell::RefCell, ffi::{c_int, c_void},
    mem::{align_of, size_of}, os::fd::{AsRawFd, FromRawFd, OwnedFd}, ptr,
    sync::{atomic::{AtomicUsize, Ordering}, Mutex, MutexGuard, OnceLock},
};

const BATCH: usize = 16;
const NONE: u32 = u32::MAX;
const WAKE: c_int = -1;
const CALLBACK: c_int = -2;
const TIMEOUT: c_int = -3;
const ERROR: c_int = -4;
type Callback = unsafe extern "C" fn(c_int, c_int, *mut c_void) -> c_int;

const _: () = {
    assert!(size_of::<libc::epoll_event>() == 16);
    assert!(align_of::<libc::epoll_event>() == 8);
    assert!(std::mem::offset_of!(libc::epoll_event, u64) == 8);
    assert!(size_of::<libc::pthread_key_t>() == 4);
};

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    fd: c_int,
    ident: c_int,
    next_free: u32,
    callback: Option<Callback>,
    data: *mut c_void, // borrowed guest address; never dereferenced by the registry
}
impl Slot {
    fn token(self, index: usize) -> u64 {
        (u64::from(self.generation) << 32) | (index as u64 + 1)
    }
}

#[derive(Default)]
struct Registry {
    slots: Vec<Slot>,
    free: Option<u32>,
    live: usize,
    // Never reset when the empty vector is released: pending kernel tokens
    // must not become valid after fd reuse or reentrant remove/register.
    generation: u32,
}
impl Registry {
    fn fd(&self, fd: c_int) -> Option<usize> {
        self.slots.iter().position(|slot| slot.fd == fd)
    }
    fn snapshot(&self, token: u64) -> Option<Slot> {
        let index = (token as u32).checked_sub(1)? as usize;
        let slot = *self.slots.get(index)?;
        (slot.fd >= 0 && slot.token(index) == token).then_some(slot)
    }
    fn reserve(&mut self, fd: c_int) -> Result<(usize, bool, u32), c_int> {
        let generation = self.generation.checked_add(1).ok_or(libc::EOVERFLOW)?;
        if let Some(index) = self.fd(fd) { return Ok((index, false, generation)); }
        let index = if let Some(index) = self.free { index as usize } else {
            if self.slots.len() >= NONE as usize { return Err(libc::EOVERFLOW); }
            // Allocate exactly one slot initially, then geometric growth.
            // A one-fd Looper retains 32 B, instead of Vec's minimum 4 slots.
            if self.slots.len() == self.slots.capacity() {
                let additional = self.slots.len().max(1);
                self.slots.try_reserve_exact(additional).map_err(|_| libc::ENOMEM)?;
            }
            self.slots.len()
        };
        Ok((index, true, generation))
    }
    fn publish(&mut self, index: usize, fresh: bool, slot: Slot) {
        if fresh {
            if index == self.slots.len() { self.slots.push(slot); }
            else { self.free = (self.slots[index].next_free != NONE).then_some(self.slots[index].next_free); self.slots[index] = slot; }
            self.live += 1;
        } else { self.slots[index] = slot; }
        self.generation = slot.generation;
    }
    fn retire(&mut self, index: usize) {
        self.slots[index].fd = -1;
        self.slots[index].callback = None;
        self.slots[index].data = ptr::null_mut();
        self.slots[index].next_free = self.free.unwrap_or(NONE);
        self.free = Some(index as u32);
        self.live -= 1;
        if self.live == 0 { self.slots = Vec::new(); self.free = None; return; }
        if self.slots.last().is_some_and(|slot| slot.fd < 0) {
            // Trim only vacant suffixes: active indices/tokens never move.
            let length = self.slots.iter().rposition(|slot| slot.fd >= 0).unwrap() + 1;
            self.slots.truncate(length);
            self.free = None;
            for (index, slot) in self.slots.iter_mut().enumerate().rev() {
                if slot.fd < 0 { slot.next_free = self.free.unwrap_or(NONE); self.free = Some(index as u32); }
            }
            if self.slots.capacity() >= 16 && length <= self.slots.capacity() / 4 {
                // Shrink off the poll path, fallibly. OOM retains the valid old
                // storage; a successful removal must not abort during compaction.
                let mut smaller = Vec::new();
                if smaller.try_reserve_exact(length).is_ok() {
                    smaller.extend_from_slice(&self.slots); self.slots = smaller;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Ready { token: u64, events: c_int }
#[derive(Default)]
struct Pending { ready: [Ready; BATCH], count: usize, cursor: usize }

pub(super) struct Looper {
    refs: AtomicUsize,
    epoll: OwnedFd,
    wake: OwnedFd,
    allow_non_callbacks: bool,
    registry: Mutex<Registry>,
    // Only pollOnce, reached through THIS thread's native TLS, accesses this
    // RefCell. Cross-thread registration/wake touch only mutex/atomic/fds.
    // Borrows end before invoking any guest callback, including reentrant poll.
    pending: RefCell<Pending>,
}
impl Looper {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn snapshot(&self, token: u64) -> Option<Slot> { self.registry().snapshot(token) }
}

unsafe fn errno() -> c_int { *libc::__errno_location() }
unsafe fn fail(error: c_int) -> c_int { *libc::__errno_location() = error; -1 }

static THREAD_KEY: OnceLock<Result<libc::pthread_key_t, c_int>> = OnceLock::new();
fn thread_key() -> Result<libc::pthread_key_t, c_int> {
    // SAFETY: `key` is a valid out-param; `thread_exit` matches the destructor ABI.
    *THREAD_KEY.get_or_init(|| unsafe {
        let mut key = 0;
        let error = libc::pthread_key_create(&mut key, Some(thread_exit));
        if error == 0 { Ok(key) } else { Err(error) }
    })
}
unsafe extern "C" fn thread_exit(pointer: *mut c_void) { release(pointer.cast()); }
unsafe extern "C" fn for_thread() -> *mut Looper {
    match thread_key() {
        Ok(key) => libc::pthread_getspecific(key).cast(),
        Err(error) => { fail(error); ptr::null_mut() }
    }
}

fn increment_reference(refs: &AtomicUsize) -> bool {
    refs.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count|
        (count != 0 && count < isize::MAX as usize).then(|| count + 1)).is_ok()
}
unsafe extern "C" fn acquire(pointer: *mut Looper) {
    let Some(looper) = pointer.as_ref() else { return; };
    // NDK acquire cannot report failure. Abort before overflow could produce
    // premature free. This count belongs to the opaque native handle protocol,
    // not to a fabricated Arc layout or a second Rust owner.
    if !increment_reference(&looper.refs) { libc::abort(); }
}
unsafe extern "C" fn release(pointer: *mut Looper) {
    let Some(looper) = pointer.as_ref() else { return; };
    let previous = looper.refs.fetch_sub(1, Ordering::AcqRel);
    if previous == 0 { libc::abort(); }
    if previous == 1 {
        // Caller/TLS transferred exactly one live reference. At zero no other
        // handle operation may be in flight. Reclaim the sole Box once; OwnedFd
        // closes internal fds, Vec drops registrations, caller fds remain open.
        drop(Box::from_raw(pointer));
    }
}
unsafe fn owned_fd(fd: c_int) -> Result<OwnedFd, c_int> {
    if fd < 0 { Err(errno()) } else { Ok(OwnedFd::from_raw_fd(fd)) }
}
unsafe fn create(opts: c_int) -> Result<Box<Looper>, c_int> {
    let epoll = owned_fd(libc::epoll_create1(libc::EPOLL_CLOEXEC))?;
    let wake = owned_fd(libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC))?;
    let mut event = libc::epoll_event { events: libc::EPOLLIN as u32, u64: 0 };
    if libc::epoll_ctl(epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, wake.as_raw_fd(), &mut event) < 0 { return Err(errno()); }
    // Box::new aborts on OOM. Use the SAME global allocator/layout as Box and
    // initialize before forming its sole owner, preserving fallible NDK prepare.
    // Guest code never frees this address or owns any Rust allocation.
    let pointer = alloc(Layout::new::<Looper>()).cast::<Looper>();
    if pointer.is_null() { return Err(libc::ENOMEM); }
    pointer.write(Looper {
        refs: AtomicUsize::new(1), epoll, wake, allow_non_callbacks: opts & 1 != 0,
        registry: Mutex::new(Registry::default()), pending: RefCell::new(Pending::default()),
    });
    Ok(Box::from_raw(pointer))
}
pub(super) unsafe extern "C" fn prepare(opts: c_int) -> *mut Looper {
    let key = match thread_key() { Ok(key) => key, Err(error) => { fail(error); return ptr::null_mut(); } };
    let existing = libc::pthread_getspecific(key).cast::<Looper>();
    if !existing.is_null() { return existing; }
    let mut owned = match create(opts) { Ok(owned) => owned, Err(error) => { fail(error); return ptr::null_mut(); } };
    let pointer = &mut *owned as *mut Looper;
    let error = libc::pthread_setspecific(key, pointer.cast());
    if error != 0 { drop(owned); fail(error); return ptr::null_mut(); }
    // Native TLS retains the initial reference; its destructor returns it.
    Box::into_raw(owned)
}
unsafe extern "C" fn wake(pointer: *mut Looper) {
    let Some(looper) = pointer.as_ref() else { return; };
    let saved = errno();
    let one = 1u64;
    while libc::write(looper.wake.as_raw_fd(), (&one as *const u64).cast(), 8) < 0 && errno() == libc::EINTR {}
    // EAGAIN means a wake is already pending.
    *libc::__errno_location() = saved;
}
unsafe extern "C" fn add_fd(pointer: *mut Looper, fd: c_int, ident: c_int, events: c_int,
    callback: Option<Callback>, data: *mut c_void) -> c_int {
    let Some(looper) = pointer.as_ref() else { return fail(libc::EINVAL); };
    if fd < 0 || (callback.is_none() && (ident < 0 || !looper.allow_non_callbacks)) { return fail(libc::EINVAL); }
    let mut registry = looper.registry();
    let (index, fresh, generation) = match registry.reserve(fd) { Ok(value) => value, Err(error) => return fail(error) };
    let slot = Slot { generation, fd, ident, next_free: NONE, callback, data };
    let mut event = libc::epoll_event {
        events: ((if events & 1 != 0 { libc::EPOLLIN } else { 0 }) |
                 (if events & 2 != 0 { libc::EPOLLOUT } else { 0 })) as u32,
        u64: slot.token(index),
    };
    let operation = if fresh { libc::EPOLL_CTL_ADD } else { libc::EPOLL_CTL_MOD };
    let mut result = libc::epoll_ctl(looper.epoll.as_raw_fd(), operation, fd, &mut event);
    // Linux forgets a closed descriptor; this number may now name another file.
    if result < 0 && !fresh && errno() == libc::ENOENT {
        result = libc::epoll_ctl(looper.epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut event);
    }
    if result < 0 {
        let error = errno();
        if registry.live == 0 { registry.slots = Vec::new(); }
        drop(registry); return fail(error);
    }
    registry.publish(index, fresh, slot);
    1
}
unsafe fn remove(pointer: *mut Looper, fd: c_int, token: u64) -> c_int {
    let Some(looper) = pointer.as_ref() else { return fail(libc::EINVAL); };
    let mut registry = looper.registry();
    let Some(index) = registry.fd(fd) else { return 0; };
    if token != 0 && registry.slots[index].token(index) != token { return 0; }
    if libc::epoll_ctl(looper.epoll.as_raw_fd(), libc::EPOLL_CTL_DEL, fd, ptr::null_mut()) < 0 &&
        errno() != libc::ENOENT && errno() != libc::EBADF { let error = errno(); drop(registry); return fail(error); }
    registry.retire(index);
    1
}
unsafe extern "C" fn remove_fd(pointer: *mut Looper, fd: c_int) -> c_int { remove(pointer, fd, 0) }

unsafe fn pending_ident(looper: &Looper, fd: *mut c_int, events: *mut c_int, data: *mut *mut c_void) -> c_int {
    loop {
        let ready = {
            let mut pending = looper.pending.borrow_mut();
            if pending.cursor == pending.count { pending.count = 0; pending.cursor = 0; return WAKE; }
            let ready = pending.ready[pending.cursor]; pending.cursor += 1; ready
        };
        let Some(request) = looper.snapshot(ready.token) else { continue; };
        if request.callback.is_some() { continue; }
        if !fd.is_null() { *fd = request.fd; }
        if !events.is_null() { *events = ready.events; }
        if !data.is_null() { *data = request.data; }
        return request.ident;
    }
}
unsafe extern "C" fn poll_once(timeout: c_int, fd: *mut c_int, out_events: *mut c_int, data: *mut *mut c_void) -> c_int {
    if !fd.is_null() { *fd = 0; }
    if !out_events.is_null() { *out_events = 0; }
    if !data.is_null() { *data = ptr::null_mut(); }
    let pointer = for_thread();
    let Some(looper) = pointer.as_ref() else { return ERROR; };
    let result = pending_ident(looper, fd, out_events, data);
    if result >= 0 { return result; }
    let mut events = [libc::epoll_event { events: 0, u64: 0 }; BATCH];
    let count = libc::epoll_wait(looper.epoll.as_raw_fd(), events.as_mut_ptr(), BATCH as c_int, timeout);
    if count < 0 { return if errno() == libc::EINTR { WAKE } else { ERROR }; }
    if count == 0 { return TIMEOUT; }
    let mut callbacks = [Ready::default(); BATCH];
    let mut callback_count = 0;
    // Classify one kernel batch under one short registry lease. The per-callback
    // second lookup below still revalidates replacements/removals before guest
    // execution. No lock survives epoll_wait or a guest callback.
    let registry = looper.registry();
    for event in &events[..count as usize] {
        if event.u64 == 0 {
            let mut value = 0u64;
            while libc::read(looper.wake.as_raw_fd(), (&mut value as *mut u64).cast(), 8) < 0 && errno() == libc::EINTR {}
            continue;
        }
        let mask = event.events as c_int;
        let ready = Ready { token: event.u64, events:
            (if mask & libc::EPOLLIN != 0 { 1 } else { 0 }) |
            (if mask & libc::EPOLLOUT != 0 { 2 } else { 0 }) |
            (if mask & libc::EPOLLERR != 0 { 4 } else { 0 }) |
            (if mask & libc::EPOLLHUP != 0 { 8 } else { 0 }) };
        let Some(request) = registry.snapshot(ready.token) else { continue; };
        if request.callback.is_some() { callbacks[callback_count] = ready; callback_count += 1; }
        else { let mut pending = looper.pending.borrow_mut(); let index = pending.count; pending.ready[index] = ready; pending.count += 1; }
    }
    drop(registry);
    let mut called = false;
    for ready in &callbacks[..callback_count] {
        // Do not keep pointers into Vec, a registry lock, or a pending borrow
        // across guest code. A copied opaque userdata has the NDK caller lifetime.
        let Some(request) = looper.snapshot(ready.token) else { continue; };
        let Some(callback) = request.callback else { continue; };
        called = true;
        if callback(request.fd, ready.events, request.data) == 0 { remove(pointer, request.fd, ready.token); }
    }
    let result = if looper.pending.borrow().count == 0 { WAKE }
        else { pending_ident(looper, fd, out_events, data) };
    if result >= 0 { result } else if called { CALLBACK } else { WAKE }
}
unsafe fn now_ms() -> i64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time);
    time.tv_sec.saturating_mul(1000).saturating_add(time.tv_nsec / 1_000_000)
}
unsafe fn remaining(deadline: i64) -> c_int {
    deadline.saturating_sub(now_ms()).clamp(0, c_int::MAX as i64) as c_int
}
unsafe extern "C" fn poll_all(timeout: c_int, fd: *mut c_int, events: *mut c_int, data: *mut *mut c_void) -> c_int {
    let deadline = if timeout > 0 { now_ms().saturating_add(timeout as i64) } else { 0 };
    loop {
        let result = poll_once(if timeout > 0 { remaining(deadline) } else { timeout }, fd, events, data);
        if result != CALLBACK { return result; }
        if timeout > 0 && remaining(deadline) == 0 { return TIMEOUT; }
    }
}

// No sensor backend. Opaque parameters are never dereferenced; getEvents
// leaves caller storage untouched. Keep exact pointer/scalar/float/ssize ABI.
static SENSOR_MANAGER: u32 = 1;
extern "C" fn sensor_manager() -> *const u32 { &SENSOR_MANAGER }
unsafe extern "C" fn sensor_list(manager: *const u32, list: *mut *const *const c_void) -> c_int {
    if !list.is_null() { *list = ptr::null(); }
    if ptr::eq(manager, &SENSOR_MANAGER) && !list.is_null() { 0 } else { -libc::EINVAL }
}
extern "C" fn default_sensor(_: *const u32, _: c_int) -> *const c_void { ptr::null() }
unsafe extern "C" fn create_queue(_: *const u32, _: *mut Looper, _: c_int, _: Option<Callback>, _: *mut c_void) -> *mut c_void {
    fail(libc::ENODEV); ptr::null_mut()
}
extern "C" fn destroy_queue(_: *const u32, _: *mut c_void) -> c_int { -libc::EINVAL }
extern "C" fn sensor_state(_: *mut c_void, _: *const c_void) -> c_int { -libc::EINVAL }
extern "C" fn event_rate(_: *mut c_void, _: *const c_void, _: i32) -> c_int { -libc::EINVAL }
extern "C" fn has_events(_: *mut c_void) -> c_int { -libc::EINVAL }
extern "C" fn get_events(_: *mut c_void, _: *mut c_void, _: usize) -> isize { -(libc::EINVAL as isize) }
extern "C" fn sensor_text(_: *const c_void) -> *const libc::c_char { ptr::null() }
extern "C" fn sensor_int(_: *const c_void) -> c_int { -libc::EINVAL }
extern "C" fn sensor_resolution(_: *const c_void) -> f32 { 0.0 }

pub(super) fn symbol(name: &[u8]) -> usize {
    match name {
        b"ALooper_acquire" => acquire as usize, b"ALooper_release" => release as usize,
        b"ALooper_forThread" => for_thread as usize, b"ALooper_prepare" => prepare as usize,
        b"ALooper_addFd" => add_fd as usize, b"ALooper_removeFd" => remove_fd as usize,
        b"ALooper_wake" => wake as usize, b"ALooper_pollOnce" => poll_once as usize,
        b"ALooper_pollAll" => poll_all as usize, b"ASensorManager_getInstance" => sensor_manager as usize,
        b"ASensorManager_getSensorList" => sensor_list as usize,
        b"ASensorManager_getDefaultSensor" => default_sensor as usize,
        b"ASensorManager_createEventQueue" => create_queue as usize,
        b"ASensorManager_destroyEventQueue" => destroy_queue as usize,
        b"ASensorEventQueue_enableSensor" | b"ASensorEventQueue_disableSensor" => sensor_state as usize,
        b"ASensorEventQueue_setEventRate" => event_rate as usize,
        b"ASensorEventQueue_hasEvents" => has_events as usize,
        b"ASensorEventQueue_getEvents" => get_events as usize,
        b"ASensor_getName" | b"ASensor_getVendor" => sensor_text as usize,
        b"ASensor_getType" | b"ASensor_getMinDelay" => sensor_int as usize,
        b"ASensor_getResolution" => sensor_resolution as usize, _ => 0,
    }
}

#[cfg(test)]
#[path = "../tests/unit/events.rs"]
mod tests;
