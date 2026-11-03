//! INPUT:  private items of `src/threads.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/threads.rs`; declared there with `#[path]`
use super::*;

unsafe extern "C" fn inspect(argument: *mut c_void) -> *mut c_void {
    let mut attr = MaybeUninit::<Attr>::uninit();
    let result = getattr(libc::pthread_self(), attr.as_mut_ptr());
    if result != 0 {
        return result as usize as *mut c_void;
    }
    let attr = attr.assume_init();
    let output = argument.cast::<usize>();
    output.write(attr.stack_size);
    output.add(1).write(attr.guard_size);
    output.add(2).write(attr.inheritsched() as usize);
    std::ptr::null_mut()
}

#[test]
fn thread_attributes_convert_stack_guards_and_opposite_sched_constants() {
    unsafe {
        let mut attr = Attr::default();
        assert_eq!(attr.stack_size, 1015808);
        assert_eq!(attr_setstacksize(&mut attr, 1), libc::EINVAL);
        assert_eq!(attr_setdetachstate(&mut attr, 2), libc::EINVAL);
        assert_eq!(attr_setscope(&mut attr, 1), libc::ENOTSUP);
        assert_eq!(attr_setinheritsched(&mut attr, 2), libc::EINVAL);
        assert_eq!(attr_setstacksize(&mut attr, 262144), 0);
        assert_eq!(attr_setguardsize(&mut attr, 0), 0);
        for inherit in [0, 1] {
            assert_eq!(attr_setinheritsched(&mut attr, inherit), 0);
            let mut output = [0usize; 3];
            let mut thread = 0;
            assert_eq!(
                create(
                    &mut thread,
                    &attr,
                    Some(inspect),
                    output.as_mut_ptr().cast()
                ),
                0
            );
            let mut result = std::ptr::null_mut();
            assert_eq!(libc::pthread_join(thread, &mut result), 0);
            assert!(result.is_null());
            // POSIX stack size is a minimum; native stack-cache reuse may
            // legitimately provide a larger allocation on another glibc.
            assert!(output[0] >= 262144);
            assert_eq!(&output[1..], &[0, inherit as usize]);
        }
        assert_eq!(
            create(
                std::ptr::null_mut(),
                &attr,
                Some(inspect),
                std::ptr::null_mut()
            ),
            libc::EINVAL
        );
    }
}

#[test]
fn android_audio_sized_stacks_create_real_workers_with_the_host_minimum() {
    // Android permits these sizes; the ARM64 Linux host may require 128KiB.
    // The guest-facing attribute must retain its requested value while the
    // actual native stack meets the host ABI and still joins normally.
    unsafe {
        let minimum = libc::sysconf(libc::_SC_THREAD_STACK_MIN);
        assert!(minimum > 0);
        for requested in [16 * 1024, 64 * 1024, 96 * 1024] {
            let mut attr = Attr::default();
            assert_eq!(attr_setstacksize(&mut attr, requested), 0);
            let mut output = [0usize; 3];
            let mut thread = 0;
            assert_eq!(create(&mut thread, &attr, Some(inspect), output.as_mut_ptr().cast()), 0,
                "valid Android audio stack {requested} must create a native worker");
            let mut result = std::ptr::null_mut();
            assert_eq!(libc::pthread_join(thread, &mut result), 0);
            assert!(result.is_null());
            assert!(output[0] >= requested.max(minimum as usize));
            assert_eq!(attr.stack_size, requested, "translation must not mutate the guest attr");
        }
    }
}

unsafe extern "C" fn inspect_stack_bounds(argument: *mut c_void) -> *mut c_void {
    let mut attr = MaybeUninit::<Attr>::uninit();
    let result = getattr(libc::pthread_self(), attr.as_mut_ptr());
    if result != 0 { return result as usize as *mut c_void; }
    let attr = attr.assume_init();
    let output = argument.cast::<usize>();
    output.write(attr.stack_base as usize);
    output.add(1).write(attr.stack_size);
    std::ptr::null_mut()
}

#[test]
fn borrowed_stack_is_never_extended_and_valid_large_ranges_stay_borrowed() {
    unsafe {
        let minimum = libc::sysconf(libc::_SC_THREAD_STACK_MIN);
        assert!(minimum > 0);
        for requested in [16 * 1024, minimum as usize * 2] {
            let memory = libc::mmap(std::ptr::null_mut(), requested, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
            assert_ne!(memory, libc::MAP_FAILED);
            std::ptr::write_bytes(memory.cast::<u8>(), 0xa5, requested);
            let mut attr = Attr::default();
            assert_eq!(attr_setstack(&mut attr, memory, requested), 0);
            let mut output = [0usize; 2];
            let mut thread = 0;
            assert_eq!(create(&mut thread, &attr, Some(inspect_stack_bounds), output.as_mut_ptr().cast()), 0);
            let mut result = std::ptr::null_mut();
            assert_eq!(libc::pthread_join(thread, &mut result), 0);
            assert!(result.is_null());
            assert_eq!(attr.stack_base, memory);
            assert_eq!(attr.stack_size, requested);
            if requested < minimum as usize {
                assert!(output[0] + output[1] <= memory as usize || output[0] >= memory as usize + requested,
                    "fallback stack must not overlap any caller-owned byte");
                assert!(output[1] >= minimum as usize);
                assert!(std::slice::from_raw_parts(memory.cast::<u8>(), requested).iter().all(|byte| *byte == 0xa5),
                    "small caller-owned stack must remain untouched");
            } else {
                assert_eq!(output, [memory as usize, requested], "a valid borrowed stack retains its exact bounds");
            }
            assert_eq!(libc::munmap(memory, requested), 0);
        }
    }
}
