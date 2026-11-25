//! INPUT: private executable ARM64 fixture pages and verified policy prefixes
//! OUTPUT: valid patch and rollback behavior
//! POS: Soul Knight startup patch regression
use super::*;

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
#[test]
fn native_offline_optional_policies_preserve_abis_and_checked_rollback() {
    use std::ffi::{c_int, c_void};
    extern "C" {
        fn getpagesize() -> c_int;
        fn mmap(address: *mut c_void, length: usize, protection: c_int, flags: c_int, fd: c_int, offset: i64) -> Ptr;
        fn mprotect(address: Ptr, length: usize, protection: c_int) -> c_int;
        fn munmap(address: Ptr, length: usize) -> c_int;
    }
    struct Page(Ptr, usize);
    impl Drop for Page {
        fn drop(&mut self) {
            // SAFETY: exclusively owned fixture mapping; no code pointer escapes.
            assert_eq!(unsafe { munmap(self.0, self.1) }, 0);
        }
    }
    // Real instructions, same production save/restore/LR/SP shape and tail B.
    // Initialization and lazy creation have independent native scalar counters.
    let mut words = vec![0xd503201f; 75 + 8];
    words[..8].copy_from_slice(&[0xf81c0ffe, 0xa9015ff8, 0xa90257f6, 0xa9034ff4,
        0xaa0003f3, 0xf9400268, 0x91000508, 0xf9000268]);
    for (slot, register) in (8..13).zip(20..25) { words[slot] = 0xd2800000 | register; }
    words[68..75].copy_from_slice(&[0xaa1303e0, 0xa9434ff4, 0xa94257f6, 0xa9415ff8,
        0xf84407fe, 0x14000002, 0xd4200000]);
    let original = words[..75].to_vec();
    let getter = [0xf9400408, 0x91000508, 0xf9000408, 0xd65f03c0,
        0xd503201f, 0xd503201f, 0xd503201f, 0xd503201f];
    words[75..].copy_from_slice(&getter);
    // Native caller checks SP plus x19..24 canaries across each constructor.
    // It saves its own caller state and invokes x1 with x0 still the counter.
    let mut caller = vec![0xa9bc67fe, 0xa9015ff8, 0xa90257f6, 0xa9034ff4, 0x910003f9];
    for register in 19..25 { caller.push(0xd2800000 | register << 5 | register); }
    caller.extend_from_slice(&[0xd63f0020, 0x910003e8, 0xeb19011f]);
    let mut failures = vec![caller.len()]; caller.push(0);
    for register in 19..25 {
        caller.push(0xf100001f | register << 10 | register << 5);
        failures.push(caller.len()); caller.push(0);
    }
    caller.push(0xd2800020); let success = caller.len(); caller.push(0);
    let failed = caller.len(); caller.push(0xd2800000);
    let restore = caller.len();
    caller.extend_from_slice(&[0xa9434ff4, 0xa94257f6, 0xa9415ff8, 0xa8c467fe, 0xd65f03c0]);
    for branch in failures { caller[branch] = 0x54000001 | ((failed - branch) as u32) << 5; }
    caller[success] = 0x14000000 | (restore - success) as u32;
    words.resize(128, 0xd503201f); words.extend_from_slice(&caller);
    // Independent static Boolean/hidden-pointer ABI with an observable
    // initialization counter. Its first two words establish the original frame.
    let notification = [0xa9bf4ffe, 0xaa0003f3, 0xf9400a68, 0x91000508,
        0xf9000a68, 0x52800020, 0xa8c14ffe, 0xd65f03c0];
    words.resize(192, 0xd503201f); words.extend_from_slice(&notification);
    let analytics = [0xa9bf4ffe, 0xaa0003f3, 0xf9400e68, 0x91000508,
        0xf9000e68, 0xd503201f, 0xa8c14ffe, 0xd65f03c0];
    words.resize(256, 0xd503201f); words.extend_from_slice(&analytics);
    // SAFETY: this test owns a fresh page, seals and flushes its bounded code,
    // executes only these known scalar ABIs, and keeps every patch idle.
    unsafe {
        let length = getpagesize() as usize;
        assert!(words.len() * 4 <= length);
        let raw = mmap(std::ptr::null_mut(), length, 3, 0x22, -1, 0);
        assert_ne!(raw as isize, -1); let page = Page(raw, length);
        std::ptr::copy_nonoverlapping(words.as_ptr(), raw.cast::<u32>(), words.len());
        arm64_detour::flush_code(raw.cast(), words.len() * 4);
        assert_eq!(mprotect(raw, length, 5), 0);
        let entry = page.0 as usize;
        let patch = WebViewWarmup { entry, before: original[..73].to_vec(), after: [original[74]], getter };
        let call: extern "C" fn(Ptr, usize) -> u64 = std::mem::transmute(entry + 512);
        let get: extern "C" fn(Ptr) = std::mem::transmute(entry + 300);
        let mut counts = [0u64; 4]; let state = counts.as_mut_ptr().cast();
        assert_eq!(call(state, entry), 1, "original canaries/SP/LR survive");
        assert_eq!(counts, [1, 1, 0, 0]);
        patch.patch(true).unwrap();
        assert_eq!(call(state, entry), 1, "deferred ctor still restores its native caller");
        assert_eq!(counts, [2, 1, 0, 0], "local initialization runs, eager getter is skipped");
        get(state); assert_eq!(counts, [2, 2, 0, 0], "later explicit original getter still executes");
        for guard in 0..3 {
            let mut wrong = WebViewWarmup { entry, before: patch.before.clone(), after: patch.after, getter };
            match guard { 0 => wrong.before[0] ^= 1, 1 => wrong.after[0] ^= 1, _ => wrong.getter[0] ^= 1 }
            assert!(wrong.patch(false).is_err());
            assert_eq!(((entry + 292) as *const u32).read_volatile(), 0xd65f03c0);
        }
        patch.patch(false).unwrap();
        assert_eq!(std::slice::from_raw_parts(entry as *const u32, 75), original.as_slice());
        assert_eq!(call(state, entry), 1);
        assert_eq!(counts, [3, 3, 0, 0], "inverse rollback restores original eager behavior");
        let init: extern "C" fn(Ptr) -> bool = std::mem::transmute(entry + 768);
        let negative = NotificationUnavailable { entry: entry + 768, saved: notification[..2].try_into().unwrap(),
            remaining: notification[2..].try_into().unwrap() };
        assert!(init(state)); assert_eq!(counts[2], 1);
        negative.patch(true).unwrap();
        assert!(!init(state), "the actual Boolean ABI returns false with a nonnull hidden pointer");
        assert_eq!(counts[2], 1, "unsupported initialization must not execute its side effect");
        assert_eq!(call(state, entry + 768), 1, "early false also preserves caller SP/LR/callee-save canaries");
        assert_eq!(counts[2], 1);
        let mut wrong = NotificationUnavailable { entry: negative.entry, saved: negative.saved, remaining: negative.remaining };
        wrong.remaining[0] ^= 1;
        assert!(wrong.patch(false).is_err());
        assert_eq!(std::slice::from_raw_parts(negative.entry as *const u32, 2), &[0x52800000, 0xd65f03c0]);
        negative.patch(false).unwrap();
        assert_eq!(std::slice::from_raw_parts(negative.entry as *const u32, 8), &notification);
        assert!(init(state)); assert_eq!(counts[2], 2);
        let analytics_init: extern "C" fn(Ptr) = std::mem::transmute(entry + 1024);
        let no_analytics = AnalyticsUnavailable { entry: entry + 1024, saved: analytics[0],
            remaining: analytics[1..].try_into().unwrap() };
        analytics_init(state); assert_eq!(counts[3], 1);
        no_analytics.patch(true).unwrap();
        analytics_init(state);
        assert_eq!(counts[3], 1, "Void/nonnull-hidden-pointer return skips telemetry initialization");
        assert_eq!(call(state, no_analytics.entry), 1, "Void early return preserves caller SP/LR/callee-saves");
        let mut wrong = AnalyticsUnavailable { entry: no_analytics.entry, saved: no_analytics.saved, remaining: no_analytics.remaining };
        wrong.remaining[0] ^= 1;
        assert!(wrong.patch(false).is_err());
        assert_eq!((no_analytics.entry as *const u32).read_volatile(), 0xd65f03c0);
        no_analytics.patch(false).unwrap();
        assert_eq!(std::slice::from_raw_parts(no_analytics.entry as *const u32, 8), &analytics);
        analytics_init(state); assert_eq!(counts[3], 2);
        // Exercise the SAME NoNear selector with a real typed wide install,
        // callable relocated original and complete reverse restoration.
        unsafe extern "C-unwind" fn replacement(_: Ptr) {}
        let typed = Original::<unsafe extern "C-unwind" fn(Ptr)>::new();
        let installed = select_no_near(no_analytics.entry,
            Err(arm64_detour::DetourError::NoNearTrampoline { target: no_analytics.entry }),
            || typed.install_quiescent_tracked(no_analytics.entry, replacement).map(SourceDetour::Quiescent)).unwrap();
        let SourceDetour::Quiescent(installed) = installed else { panic!("precise NoNear must select wide"); };
        assert_eq!(typed.address(), installed.trampoline);
        analytics_init(state); assert_eq!(counts[3], 2);
        typed.get().unwrap()(state); assert_eq!(counts[3], 3, "typed original replays the real prologue");
        arm64_detour::restore_quiescent(&installed).unwrap();
        typed.clear(); assert!(typed.get().is_none());
        assert_eq!(std::slice::from_raw_parts(no_analytics.entry as *const u32, 8), &analytics);
        analytics_init(state); assert_eq!(counts[3], 4);
        // The shared pool pins that original trampoline for process life,
        // including its continuation into this one private fixture code page.
        std::mem::forget(page);
    }
}
