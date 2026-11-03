//! INPUT:  Android LP64 pthread_attr_t fields and native thread/TLS functions
//! OUTPUT:  pthread_create/getattr, attribute, identity and TLS symbol addresses
//! POS:    translates Android thread attributes to the host minimum without extending guest-owned stacks

use std::ffi::{c_int, c_void};
use std::mem::MaybeUninit;
use std::sync::OnceLock;

static HOST_MIN_STACK: OnceLock<usize> = OnceLock::new();

// Android pthread_types.h (LP64): 56 bytes, alignment 8. Not a native attr.
#[repr(C)]
struct Attr {
    flags: u32,
    stack_base: *mut c_void,
    stack_size: usize,
    guard_size: usize,
    sched_policy: c_int,
    sched_priority: c_int,
    reserved: [u8; 16],
}
const _: () = assert!(std::mem::size_of::<Attr>() == 56);

impl Attr {
    unsafe fn default() -> Self {
        Self {
            flags: 0,
            stack_base: std::ptr::null_mut(),
            stack_size: 1024 * 1024 - 32 * 1024,
            guard_size: getpagesize() as usize,
            sched_policy: libc::SCHED_OTHER,
            sched_priority: 0,
            reserved: [0; 16],
        }
    }
    fn inheritsched(&self) -> c_int {
        // Android EXPLICIT=0 / INHERIT=1, opposite to GNU. Before Android P,
        // zero flags infer this setting from the policy instead of a flag.
        if self.flags & 4 != 0 {
            1
        } else if self.flags & 8 != 0 || self.sched_policy != libc::SCHED_OTHER {
            0
        } else {
            1
        }
    }
}

unsafe extern "C" fn attr_init(attr: *mut Attr) -> c_int {
    if attr.is_null() {
        return libc::EINVAL;
    }
    attr.write(Attr::default());
    0
}
unsafe extern "C" fn attr_destroy(attr: *mut Attr) -> c_int {
    if attr.is_null() {
        return libc::EINVAL;
    }
    std::ptr::write_bytes(attr.cast::<u8>(), 0x42, 56);
    0
}
unsafe extern "C" fn attr_getdetachstate(attr: *const Attr, value: *mut c_int) -> c_int {
    if attr.is_null() || value.is_null() {
        return libc::EINVAL;
    }
    value.write(((*attr).flags & 1) as c_int);
    0
}
unsafe extern "C" fn attr_setdetachstate(attr: *mut Attr, value: c_int) -> c_int {
    if attr.is_null() || !(0..=1).contains(&value) {
        return libc::EINVAL;
    }
    (*attr).flags = ((*attr).flags & !1) | value as u32;
    0
}
unsafe extern "C" fn attr_getinheritsched(attr: *const Attr, value: *mut c_int) -> c_int {
    if attr.is_null() || value.is_null() {
        return libc::EINVAL;
    }
    value.write((*attr).inheritsched());
    0
}
unsafe extern "C" fn attr_setinheritsched(attr: *mut Attr, value: c_int) -> c_int {
    if attr.is_null() || !(0..=1).contains(&value) {
        return libc::EINVAL;
    }
    (*attr).flags = ((*attr).flags & !12) | if value == 0 { 8 } else { 4 };
    0
}
macro_rules! scalar_attr {
    ($get:ident, $set:ident, $field:ident, $type:ty, $valid:expr) => {
        unsafe extern "C" fn $get(attr: *const Attr, value: *mut $type) -> c_int {
            if attr.is_null() || value.is_null() {
                return libc::EINVAL;
            }
            value.write((*attr).$field);
            0
        }
        unsafe extern "C" fn $set(attr: *mut Attr, value: $type) -> c_int {
            if attr.is_null() || !($valid)(value) {
                return libc::EINVAL;
            }
            (*attr).$field = value;
            0
        }
    };
}
scalar_attr!(
    attr_getstacksize,
    attr_setstacksize,
    stack_size,
    usize,
    |v| v >= 16384
);
scalar_attr!(
    attr_getguardsize,
    attr_setguardsize,
    guard_size,
    usize,
    |_| true
);
scalar_attr!(
    attr_getschedpolicy,
    attr_setschedpolicy,
    sched_policy,
    c_int,
    |_| true
);
unsafe extern "C" fn attr_getschedparam(attr: *const Attr, value: *mut libc::sched_param) -> c_int {
    if attr.is_null() || value.is_null() {
        return libc::EINVAL;
    }
    (*value).sched_priority = (*attr).sched_priority;
    0
}
unsafe extern "C" fn attr_setschedparam(attr: *mut Attr, value: *const libc::sched_param) -> c_int {
    if attr.is_null() || value.is_null() {
        return libc::EINVAL;
    }
    (*attr).sched_priority = (*value).sched_priority;
    0
}
unsafe extern "C" fn attr_getstack(
    attr: *const Attr,
    base: *mut *mut c_void,
    size: *mut usize,
) -> c_int {
    if attr.is_null() || base.is_null() || size.is_null() {
        return libc::EINVAL;
    }
    base.write((*attr).stack_base);
    size.write((*attr).stack_size);
    0
}
unsafe extern "C" fn attr_setstack(attr: *mut Attr, base: *mut c_void, size: usize) -> c_int {
    let page = getpagesize() as usize;
    if attr.is_null()
        || base.is_null()
        || size < 16384
        || size % page != 0
        || base as usize % page != 0
    {
        return libc::EINVAL;
    }
    (*attr).stack_base = base;
    (*attr).stack_size = size;
    0
}
unsafe extern "C" fn attr_getscope(attr: *const Attr, scope: *mut c_int) -> c_int {
    if attr.is_null() || scope.is_null() {
        return libc::EINVAL;
    }
    scope.write(0);
    0 // Android and GNU PTHREAD_SCOPE_SYSTEM=0.
}
unsafe extern "C" fn attr_setscope(attr: *mut Attr, scope: c_int) -> c_int {
    if attr.is_null() {
        return libc::EINVAL;
    }
    match scope {
        0 => 0,
        1 => libc::ENOTSUP,
        _ => libc::EINVAL,
    }
}

unsafe extern "C" {
    fn getpagesize() -> c_int;
    fn pthread_attr_getdetachstate(attr: *const libc::pthread_attr_t, state: *mut c_int) -> c_int;
    fn pthread_create(
        thread: *mut libc::pthread_t,
        attr: *const libc::pthread_attr_t,
        entry: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        argument: *mut c_void,
    ) -> c_int;
}
unsafe extern "C" fn create(
    thread: *mut libc::pthread_t,
    attr: *const Attr,
    entry: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    argument: *mut c_void,
) -> c_int {
    if thread.is_null() || entry.is_none() {
        return libc::EINVAL;
    }
    let default = Attr::default();
    let guest = if attr.is_null() { &default } else { &*attr };
    if guest.flags & !13 != 0 {
        return libc::EINVAL;
    }
    let mut host = MaybeUninit::<libc::pthread_attr_t>::uninit();
    let host = host.as_mut_ptr();
    let result = libc::pthread_attr_init(host);
    if result != 0 {
        return result;
    }
    let configure = || {
        let steps = [
            libc::pthread_attr_setdetachstate(host, (guest.flags & 1) as c_int),
            libc::pthread_attr_setguardsize(host, guest.guard_size),
            libc::pthread_attr_setschedpolicy(host, guest.sched_policy),
            libc::pthread_attr_setschedparam(
                host,
                &libc::sched_param {
                    sched_priority: guest.sched_priority,
                },
            ),
            libc::pthread_attr_setinheritsched(
                host,
                if guest.inheritsched() == 1 {
                    libc::PTHREAD_INHERIT_SCHED
                } else {
                    libc::PTHREAD_EXPLICIT_SCHED
                },
            ),
        ];
        for result in steps {
            if result != 0 {
                return result;
            }
        }
        let minimum = *HOST_MIN_STACK.get_or_init(|| {
            // Native ABI query, once per process. The running firmware's
            // minimum can exceed Android's accepted 16KiB attribute minimum.
            // SAFETY: sysconf takes a scalar selector and retains no pointers.
            let value = libc::sysconf(libc::_SC_THREAD_STACK_MIN);
            if value > 0 { value as usize } else { libc::PTHREAD_STACK_MIN }
        });
        if guest.stack_size == 0 {
            if guest.stack_base.is_null() { 0 } else { libc::EINVAL }
        } else if !guest.stack_base.is_null() && guest.stack_size >= minimum {
            libc::pthread_attr_setstack(host, guest.stack_base, guest.stack_size)
        } else {
            // Never enlarge a caller-owned allocation in place. A valid
            // Android stack below the host minimum gets a host-owned stack;
            // the original guest range remains untouched and guest-owned.
            libc::pthread_attr_setstacksize(host, guest.stack_size.max(minimum))
        }
    };
    let mut result = configure();
    if result == 0 {
        result = pthread_create(thread, host, entry.unwrap(), argument);
    }
    libc::pthread_attr_destroy(host);
    result
}

unsafe extern "C" fn getattr(thread: libc::pthread_t, attr: *mut Attr) -> c_int {
    if attr.is_null() {
        return libc::EINVAL;
    }
    let mut host = MaybeUninit::<libc::pthread_attr_t>::uninit();
    let host = host.as_mut_ptr();
    let result = libc::pthread_getattr_np(thread, host);
    if result != 0 {
        return result;
    }
    let mut guest = Attr::default();
    let mut detach = 0;
    let mut inherit = 0;
    let mut param = libc::sched_param { sched_priority: 0 };
    let steps = [
        pthread_attr_getdetachstate(host, &mut detach),
        libc::pthread_attr_getguardsize(host, &mut guest.guard_size),
        libc::pthread_attr_getschedpolicy(host, &mut guest.sched_policy),
        libc::pthread_attr_getschedparam(host, &mut param),
        libc::pthread_attr_getinheritsched(host, &mut inherit),
        libc::pthread_attr_getstack(host, &mut guest.stack_base, &mut guest.stack_size),
    ];
    libc::pthread_attr_destroy(host);
    for result in steps {
        if result != 0 {
            return result;
        }
    }
    guest.flags = detach as u32
        | if inherit == libc::PTHREAD_INHERIT_SCHED {
            4
        } else {
            8
        };
    guest.sched_priority = param.sched_priority;
    attr.write(guest);
    0
}

unsafe extern "C" fn sigmask(how: c_int, set: *const u64, old: *mut u64) -> c_int {
    // Android ARM64 sigset_t is one unsigned long; GNU sigset_t is 128 bytes.
    // Use the native public bit API rather than assuming its private layout.
    let saved_errno = *libc::__errno_location();
    let mut host_set = MaybeUninit::<libc::sigset_t>::uninit();
    let mut host_old = MaybeUninit::<libc::sigset_t>::uninit();
    let pointer = if set.is_null() {
        std::ptr::null()
    } else {
        libc::sigemptyset(host_set.as_mut_ptr());
        let bits = set.read();
        for signal in 1..=64 {
            if bits & (1 << (signal - 1)) != 0 {
                libc::sigaddset(host_set.as_mut_ptr(), signal);
            }
        }
        host_set.as_ptr()
    };
    let old_pointer = if old.is_null() {
        std::ptr::null_mut()
    } else {
        host_old.as_mut_ptr()
    };
    let result = libc::pthread_sigmask(how, pointer, old_pointer);
    if result == 0 && !old.is_null() {
        let mut bits = 0;
        for signal in 1..=64 {
            if libc::sigismember(host_old.as_ptr(), signal) == 1 {
                bits |= 1 << (signal - 1);
            }
        }
        old.write(bits);
    }
    *libc::__errno_location() = saved_errno;
    result
}

symbol_provider! {
    b"pthread_create" => create as *const () as usize,
    b"pthread_getattr_np" => getattr as *const () as usize,
    b"pthread_attr_init" => attr_init as *const () as usize,
    b"pthread_attr_destroy" => attr_destroy as *const () as usize,
    b"pthread_attr_getdetachstate" => attr_getdetachstate as *const () as usize,
    b"pthread_attr_setdetachstate" => attr_setdetachstate as *const () as usize,
    b"pthread_attr_getinheritsched" => attr_getinheritsched as *const () as usize,
    b"pthread_attr_setinheritsched" => attr_setinheritsched as *const () as usize,
    b"pthread_attr_getstacksize" => attr_getstacksize as *const () as usize,
    b"pthread_attr_setstacksize" => attr_setstacksize as *const () as usize,
    b"pthread_attr_getguardsize" => attr_getguardsize as *const () as usize,
    b"pthread_attr_setguardsize" => attr_setguardsize as *const () as usize,
    b"pthread_attr_getschedpolicy" => attr_getschedpolicy as *const () as usize,
    b"pthread_attr_setschedpolicy" => attr_setschedpolicy as *const () as usize,
    b"pthread_attr_getschedparam" => attr_getschedparam as *const () as usize,
    b"pthread_attr_setschedparam" => attr_setschedparam as *const () as usize,
    b"pthread_attr_getstack" => attr_getstack as *const () as usize,
    b"pthread_attr_setstack" => attr_setstack as *const () as usize,
    b"pthread_attr_getscope" => attr_getscope as *const () as usize,
    b"pthread_attr_setscope" => attr_setscope as *const () as usize,
    // ARM64 pthread_t is an opaque 64-bit token; pthread_key_t is 32 bits.
    // Tokens originate from this same native service, including Rust/native
    // callers. No guest pthread_internal_t is exposed or dereferenced.
    b"pthread_self" => libc::pthread_self as *const () as usize,
    b"pthread_equal" => libc::pthread_equal as *const () as usize,
    b"pthread_join" => libc::pthread_join as *const () as usize,
    b"pthread_detach" => libc::pthread_detach as *const () as usize,
    b"pthread_exit" => libc::pthread_exit as *const () as usize,
    b"pthread_key_create" => libc::pthread_key_create as *const () as usize,
    b"pthread_key_delete" => libc::pthread_key_delete as *const () as usize,
    b"pthread_getspecific" => libc::pthread_getspecific as *const () as usize,
    b"pthread_setspecific" => libc::pthread_setspecific as *const () as usize,
    b"pthread_setname_np" => libc::pthread_setname_np as *const () as usize,
    b"pthread_getname_np" => libc::pthread_getname_np as *const () as usize,
    b"pthread_getcpuclockid" => libc::pthread_getcpuclockid as *const () as usize,
    b"pthread_getschedparam" => libc::pthread_getschedparam as *const () as usize,
    b"pthread_setschedparam" => libc::pthread_setschedparam as *const () as usize,
    b"pthread_kill" => libc::pthread_kill as *const () as usize,
    b"pthread_sigmask" => sigmask as *const () as usize,
}

#[cfg(test)]
#[path = "../tests/unit/threads.rs"]
mod tests;
