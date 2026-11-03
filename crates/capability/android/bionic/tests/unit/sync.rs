//! INPUT:  private items of `src/sync.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/sync.rs`; declared there with `#[path]`
use super::*;

#[test]
fn barrier_cycles_have_one_serial_participant_and_preserve_32_bit_attributes() {
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    let _serial=crate::LIFECYCLE_TEST_LOCK.lock().unwrap();
    #[repr(C,align(8))] struct Barrier {before:u64,words:[u64;4],after:u64}
    let mut guest=Barrier {before:91,words:[0;4],after:92};let pointer=guest.words.as_mut_ptr().cast();
    // SAFETY: exact LP64 facades and scalar attributes; worker threads join before destroy/free.
    unsafe {
        let mut attr=[0x77777777i32;3];assert_eq!(barrierattr_init(attr.as_mut_ptr()),0);assert_eq!(attr,[0,0x77777777,0x77777777]);
        assert_eq!(barrier_init(pointer,attr.as_ptr(),0),libc::EINVAL);
        assert_eq!(barrierattr_setpshared(attr.as_mut_ptr(),1),0);
        let mut shared=[-1,81];assert_eq!(barrierattr_getpshared(attr.as_ptr(),shared.as_mut_ptr()),0);assert_eq!(shared,[1,81]);
        assert_eq!(barrier_init(pointer,attr.as_ptr(),5),libc::ENOTSUP);assert_eq!(guest.words,[0;4]);
        assert_eq!(barrierattr_setpshared(attr.as_mut_ptr(),2),libc::EINVAL);assert_eq!(attr[0],1);
        assert_eq!(barrierattr_setpshared(attr.as_mut_ptr(),0),0);
        FAIL.with(|fail|fail.set(true));assert_eq!(barrier_init(pointer,attr.as_ptr(),5),libc::ENOMEM);
        FAIL_RECORD.with(|fail|fail.set(true));assert_eq!(barrier_init(pointer,attr.as_ptr(),5),libc::ENOMEM);
        assert_eq!(LIVE.load(Ordering::Relaxed),0);assert!(OWNED.lock().unwrap().is_empty());
        assert_eq!(barrier_init(pointer,attr.as_ptr(),5),0);
        let serials=Arc::new(AtomicUsize::new(0));let values=Arc::new((0..5).map(|_|AtomicUsize::new(0)).collect::<Vec<_>>());
        let address=pointer as usize;
        let workers:Vec<_>=(0..5).map(|id| {let values=Arc::clone(&values);let serials=Arc::clone(&serials);
            std::thread::spawn(move|| {for round in 1..=32 {
                values[id].store(round,Ordering::Relaxed);
                let r=barrier_wait(address as *mut c_void);assert!(r==0 || r==-1);if r==-1 {serials.fetch_add(1,Ordering::Relaxed);}
                assert!(values.iter().all(|v|v.load(Ordering::Relaxed)==round));
                let r=barrier_wait(address as *mut c_void);assert!(r==0 || r==-1);if r==-1 {serials.fetch_add(1,Ordering::Relaxed);}
            }})
        }).collect();for w in workers {w.join().unwrap();}
        assert_eq!(serials.load(Ordering::Relaxed),64);assert_eq!((guest.before,guest.after),(91,92));
        assert_eq!(barrier_destroy(pointer),0);assert_eq!(barrier_wait(pointer),libc::EINVAL);assert_eq!(barrier_destroy(pointer),libc::EINVAL);
        assert_eq!(barrierattr_destroy(attr.as_mut_ptr()),0);assert_eq!(attr,[0,0x77777777,0x77777777]);
        assert_eq!(barrier_init(pointer,std::ptr::null(),1),0);assert_eq!(barrier_wait(pointer),-1);
        finalize_range(pointer as usize,32);assert_eq!(barrier_wait(pointer),libc::EINVAL);assert_eq!(LIVE.load(Ordering::Relaxed),0);
    }
}

#[test]
fn spin_facades_serialize_native_threads_and_recover_failed_ownership() {
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    let _serial=crate::LIFECYCLE_TEST_LOCK.lock().unwrap();
    #[repr(C,align(8))] struct Spin {before:u64,word:u64,after:u64}
    let mut guest=Spin {before:51,word:0,after:52};let p=(&mut guest.word as *mut u64).cast();
    // SAFETY: initialized one-word LP64 facade remains live until all native users join.
    unsafe {
        assert_eq!(spin_lock(p),libc::EINVAL);assert_eq!(spin_init(p,1),libc::ENOTSUP);assert_eq!(spin_init(p,2),libc::EINVAL);
        FAIL.with(|fail|fail.set(true));assert_eq!(spin_init(p,0),libc::ENOMEM);
        FAIL_RECORD.with(|fail|fail.set(true));assert_eq!(spin_init(p,0),libc::ENOMEM);
        assert_eq!(LIVE.load(Ordering::Relaxed),0);assert_eq!(guest.word,0);
        assert_eq!(spin_init(p,0),0);assert_eq!(spin_lock(p),0);assert_eq!(spin_trylock(p),libc::EBUSY);assert_eq!(spin_unlock(p),0);
        let count=Arc::new(AtomicUsize::new(0));let address=p as usize;
        let workers:Vec<_>=(0..8).map(|_| {let count=Arc::clone(&count);std::thread::spawn(move|| {for _ in 0..1000 {
            assert_eq!(spin_lock(address as *mut c_void),0);
            let old=count.load(Ordering::Relaxed);count.store(old+1,Ordering::Relaxed);
            assert_eq!(spin_unlock(address as *mut c_void),0);
        }})}).collect();for w in workers {w.join().unwrap();}
        assert_eq!(count.load(Ordering::Relaxed),8000);assert_eq!((guest.before,guest.after),(51,52));
        assert_eq!(spin_destroy(p),0);assert_eq!(spin_lock(p),libc::EINVAL);
        for _ in 0..500 {assert_eq!(spin_init(p,0),0);assert_eq!(spin_trylock(p),0);assert_eq!(spin_unlock(p),0);finalize_range(p as usize,8);}
        assert_eq!(LIVE.load(Ordering::Relaxed),0);assert!(OWNED.lock().unwrap().is_empty());
        assert_eq!((guest.before,guest.after),(51,52));
    }
}

#[repr(C, align(8))]
struct Guarded {
    prefix: u32,
    guest: [u32; 14],
    suffix: u32,
}

#[test]
fn lazy_races_oom_busy_destroy_and_native_payload_recovery() {
    let _serial = crate::LIFECYCLE_TEST_LOCK.lock().unwrap();
    unsafe {
        let mut object = Guarded {
            prefix: 71,
            guest: [0; 14],
            suffix: 82,
        };
        let pointer = object.guest.as_mut_ptr().cast::<c_void>();
        assert_eq!(pointer as usize % 8, 4);
        FAIL.with(|fail| fail.set(true));
        assert_eq!(mutex_lock(pointer), libc::ENOMEM);
        assert_eq!(slot(pointer).load(Ordering::Acquire), 0);
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        FAIL_RECORD.with(|fail| fail.set(true));
        assert_eq!(mutex_lock(pointer), libc::ENOMEM);
        assert_eq!(slot(pointer).load(Ordering::Acquire), 0);
        assert!(OWNED.lock().unwrap().is_empty());
        // Concurrent first touch must publish only one native object.
        let address = pointer as usize;
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let pointer = address as *mut c_void;
                    for _ in 0..1000 {
                        assert_eq!(mutex_lock(pointer), 0);
                        assert_eq!(mutex_unlock(pointer), 0);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            LIVE.load(Ordering::Relaxed),
            size_of::<libc::pthread_mutex_t>() as isize
        );
        assert_eq!(mutex_lock(pointer), 0);
        assert_eq!(mutex_destroy(pointer), libc::EBUSY);
        assert_eq!(mutex_unlock(pointer), 0);
        assert_eq!(mutex_destroy(pointer), 0);
        assert_eq!(mutex_lock(pointer), libc::EINVAL);
        assert_eq!((object.prefix, object.suffix), (71, 82));
        // Every explicit init is valid after the previous destroy; all
        // allocated C payload bytes must be reclaimed, including errors.
        for _ in 0..2000 {
            for kind in [Kind::Mutex, Kind::Cond, Kind::Rwlock] {
                assert_eq!(initialize(pointer, 0, kind), 0);
                assert_eq!(destroy(pointer, kind), 0);
            }
        }
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        assert!(OWNED.lock().unwrap().is_empty());
        assert_eq!(OWNED.lock().unwrap().capacity(), 0);
        assert_eq!((object.prefix, object.suffix), (71, 82));
        object.guest.fill(0);
        object.guest[0] = 2; // Android monotonic static condition initializer.
        let cond = ensure(pointer, Kind::Cond).unwrap();
        // A zero absolute monotonic deadline is expired; verify the mutex
        // is re-acquired by the native timed wait before returning.
        let mut mutex = [0u32; 10];
        let m = mutex.as_mut_ptr().cast();
        assert_eq!(mutex_lock(m), 0);
        let past = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        assert_eq!(cond_timedwait(pointer, m, &past), libc::ETIMEDOUT);
        assert_eq!(ensure(pointer, Kind::Cond).unwrap(), cond);
        assert_eq!(mutex_unlock(m), 0);
        assert_eq!(mutex_destroy(m), 0);
        assert_eq!(cond_destroy(pointer), 0);
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        object.guest.fill(0);
        assert_eq!(mutex_lock(pointer), 0);
        assert_eq!(mutex_unlock(pointer), 0);
        finalize_range(pointer as usize + 1, 1); // Other ranges cannot reclaim it.
        assert_ne!(LIVE.load(Ordering::Relaxed), 0);
        finalize_range(pointer as usize, 1);
        finalize_range(pointer as usize, 1); // Exactly once.
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        assert_eq!(mutex_lock(pointer), libc::EINVAL);
        assert_eq!(OWNED.lock().unwrap().capacity(), 0);
    }
}

#[test]
fn semaphore_oom_rollback_alignment_and_range_cleanup() {
    let _serial = crate::LIFECYCLE_TEST_LOCK.lock().unwrap();
    unsafe {
        #[repr(C, align(8))] struct SemGuard { before: u32, bytes: [u32;4], after: u32 }
        let mut object = SemGuard { before: 41, bytes: [0;4], after: 42 };
        let pointer = object.bytes.as_mut_ptr().cast();
        FAIL.with(|fail| fail.set(true));
        assert_eq!(semaphore_init(pointer,0,1),-1);
        assert_eq!(*libc::__errno_location(),libc::ENOMEM);
        FAIL_RECORD.with(|fail| fail.set(true));
        assert_eq!(semaphore_init(pointer,0,1),-1);
        assert_eq!(LIVE.load(Ordering::Relaxed),0);
        for _ in 0..2000 {
            assert_eq!(semaphore_init(pointer,0,1),0);
            assert_eq!(semaphore_wait(pointer),0);
            assert_eq!(semaphore_trywait(pointer),-1);
            assert_eq!(*libc::__errno_location(),libc::EAGAIN);
            assert_eq!(semaphore_post(pointer),0);
            finalize_range(pointer as usize,16);
            assert_eq!(LIVE.load(Ordering::Relaxed),0);
            assert_eq!(OWNED.lock().unwrap().capacity(),0);
        }
        assert_eq!((object.before,object.after),(41,42));
    }
}
