//! INPUT:  production diagnostic addresses, mutable caller text and isolated child processes
//! OUTPUT:  first-message ownership, assertion/stack diagnostics and real SIGABRT checks
//! POS:    verifies fatal paths without aborting the parent suite or producing core files

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {
    use std::ffi::{c_char, CStr};
    use std::os::unix::process::ExitStatusExt;

    // Independent caller ABI probe, restoring its original platform register.
    std::arch::global_asm!(r#"
        .text
        .p2align 2
        .global rudroid_signal_syscall_register_probe
        .hidden rudroid_signal_syscall_register_probe
        .type rudroid_signal_syscall_register_probe, %function
    rudroid_signal_syscall_register_probe:
        .cfi_startproc
        hint #34
        stp x29, x30, [sp, #-48]!
        .cfi_def_cfa_offset 48
        .cfi_offset x29, -48
        .cfi_offset x30, -40
        stp x18, x19, [sp, #16]
        .cfi_offset x18, -32
        .cfi_offset x19, -24
        mov x29, sp
        mov x19, x0
        movz x18, #0x1234, lsl #48
        movk x18, #0x5678
        mov x0, #134
        mov x1, #6
        mov x2, #0
        mov x3, #0
        mov x4, #8
        blr x19
        movz x9, #0x1234, lsl #48
        movk x9, #0x5678
        cmp x18, x9
        cset w0, eq
        ldp x18, x19, [sp, #16]
        ldp x29, x30, [sp], #48
        .cfi_restore x18
        .cfi_restore x19
        .cfi_restore x29
        .cfi_restore x30
        .cfi_def_cfa_offset 0
        ret
        .cfi_endproc
        .size rudroid_signal_syscall_register_probe, .-rudroid_signal_syscall_register_probe
        .section .note.GNU-stack,"",%progbits
    "#);

    unsafe fn address(name: &CStr) -> usize {
        let result = android_bionic::rudroid_bionic_symbol(
            name.as_ptr(),
            c"libc.so".as_ptr(),
            c"LIBC".as_ptr(),
            std::ptr::null_mut(),
        );
        assert_ne!(result, 0);
        result
    }
    #[test]
    fn signal_interface_metadata_is_bounded_quiet_by_default_and_preserves_kernel_state() {
        const NAME: &str = "linux::signal_interface_metadata_is_bounded_quiet_by_default_and_preserves_kernel_state";
        if let Some(case) = std::env::var_os("RUDROID_SIGNAL_METADATA_TEST") {
            // Independent Android LP64 declaration, guarded on both sides.
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct Action { flags: i32, callback: usize, mask: u64, restorer: usize }
            #[repr(C)]
            struct Guarded { before: u64, action: Action, after: u64 }
            // AOSP ARM64 __kernel_sigaction: handler first, unlike the
            // flags-first Android libc structure above. Native libc below
            // independently checks the state installed through this raw ABI.
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct KernelAction { callback: usize, flags: usize, restorer: usize, mask: u64 }
            #[repr(C)]
            struct GuardedKernel { before: u64, action: KernelAction, after: u64 }
            extern "C" fn handler(_: i32, _: *mut libc::siginfo_t, _: *mut std::ffi::c_void) {}
            unsafe extern "C" { fn rudroid_bionic_install_signal_diagnostics(enabled: i32); }
            unsafe extern "C" { fn rudroid_signal_syscall_register_probe(entry: usize) -> i32; }
            // SAFETY: private child process, valid independent Android action
            // and native sigaction storage. No signal is raised; every original
            // disposition is restored before exit. The setter enables metadata
            // only and is not an Android-visible symbol.
            unsafe {
                let no_core=libc::rlimit { rlim_cur:0, rlim_max:0 };
                assert_eq!(libc::setrlimit(libc::RLIMIT_CORE,&no_core),0);
                if case != "quiet" { rudroid_bionic_install_signal_diagnostics(1); }
                let action: unsafe extern "C" fn(i32, *const Action, *mut Action) -> i32 =
                    std::mem::transmute(address(c"sigaction"));
                let signal: unsafe extern "C" fn(i32, usize) -> usize =
                    std::mem::transmute(address(c"signal"));
                let empty = Action { flags: 0, callback: 0, mask: 0, restorer: 0 };
                let mut previous = Guarded { before: 73, action: empty, after: 91 };
                let next = Action { flags: libc::SA_SIGINFO | libc::SA_RESTART,
                    callback: handler as *const () as usize, mask: 1 << (libc::SIGUSR1 - 1), restorer: 0 };
                *libc::__errno_location() = 37;
                assert_eq!(action(libc::SIGBUS, &next, &mut previous.action), 0);
                assert_eq!(*libc::__errno_location(), 37);
                let mut native: libc::sigaction = std::mem::zeroed();
                assert_eq!(libc::sigaction(libc::SIGBUS, std::ptr::null(), &mut native), 0);
                assert_eq!(native.sa_sigaction, next.callback);
                assert_eq!(native.sa_flags & next.flags, next.flags);
                assert_eq!(libc::sigismember(&native.sa_mask, libc::SIGUSR1), 1);
                let mut queried = Guarded { before: 17, action: empty, after: 29 };
                assert_eq!(action(libc::SIGBUS, std::ptr::null(), &mut queried.action), 0);
                assert_eq!(queried.action.callback, next.callback);
                assert_eq!(queried.action.mask, next.mask);
                assert_eq!(queried.action.flags & next.flags, next.flags);
                assert_eq!((previous.before, previous.after, queried.before, queried.after), (73, 91, 17, 29));
                assert_eq!(action(libc::SIGBUS, &previous.action, std::ptr::null_mut()), 0);
                let mut original_native: libc::sigaction = std::mem::zeroed();
                assert_eq!(libc::sigaction(libc::SIGBUS, std::ptr::null(), &mut original_native), 0);
                let old = signal(libc::SIGBUS, libc::SIG_IGN);
                assert_ne!(old, libc::SIG_ERR);
                assert_eq!(libc::sigaction(libc::SIGBUS, std::ptr::null(), &mut native), 0);
                assert_eq!(native.sa_sigaction, libc::SIG_IGN);
                assert_eq!(signal(libc::SIGBUS, old), libc::SIG_IGN);
                assert_eq!(libc::sigaction(libc::SIGBUS, &original_native, std::ptr::null_mut()), 0);
                if case == "limit" {
                    for _ in 0..100 { assert_eq!(action(libc::SIGABRT, std::ptr::null(), std::ptr::null_mut()), 0); }
                }
                let syscall: unsafe extern "C" fn(libc::c_long, ...) -> libc::c_long =
                    std::mem::transmute(address(c"syscall"));
                // A no-argument native call uses the tail path. The signal
                // path keeps all four kernel arguments and the native errno;
                // a wrong mask size gives a real EINVAL without touching state.
                assert_eq!(syscall(libc::SYS_getpid), libc::getpid() as libc::c_long);
                *libc::__errno_location() = 71;
                assert_eq!(syscall(libc::SYS_rt_sigaction, libc::SIGABRT as libc::c_long,
                    0 as libc::c_long, 0 as libc::c_long, 8 as libc::c_long), 0);
                assert_eq!(*libc::__errno_location(), 71);
                assert_eq!(syscall(libc::SYS_rt_sigaction, libc::SIGBUS as libc::c_long,
                    0 as libc::c_long, 0 as libc::c_long, 16 as libc::c_long), -1);
                assert_eq!(*libc::__errno_location(), libc::EINVAL);
                let raw_next=KernelAction { callback:handler as *const () as usize,
                    flags:libc::SA_SIGINFO as usize, restorer:0, mask:1 << (libc::SIGUSR2-1) };
                let mut raw_previous=GuardedKernel { before:41, action:raw_next, after:43 };
                assert_eq!(syscall(libc::SYS_rt_sigaction, libc::SIGBUS as libc::c_long,
                    &raw_next, &mut raw_previous.action, 8 as libc::c_long),0);
                assert_eq!(libc::sigaction(libc::SIGBUS, std::ptr::null(), &mut native),0);
                assert_eq!(native.sa_sigaction,raw_next.callback);
                assert_eq!(native.sa_flags & libc::SA_SIGINFO,libc::SA_SIGINFO);
                assert_eq!(libc::sigismember(&native.sa_mask,libc::SIGUSR2),1);
                let mut raw_query=GuardedKernel { before:47,
                    action:KernelAction { callback:usize::MAX, flags:usize::MAX, restorer:usize::MAX, mask:u64::MAX }, after:53 };
                assert_eq!(syscall(libc::SYS_rt_sigaction, libc::SIGBUS as libc::c_long,
                    0 as libc::c_long, &mut raw_query.action, 8 as libc::c_long),0);
                assert_eq!((raw_query.action.callback,raw_query.action.mask),(raw_next.callback,raw_next.mask));
                assert_eq!((raw_previous.before,raw_previous.after,raw_query.before,raw_query.after),(41,43,47,53));
                assert_eq!(syscall(libc::SYS_rt_sigaction, libc::SIGBUS as libc::c_long,
                    &raw_previous.action, 0 as libc::c_long, 8 as libc::c_long),0);
                assert_eq!(libc::sigaction(libc::SIGBUS,std::ptr::null(),&mut native),0);
                assert_eq!((native.sa_sigaction,native.sa_flags),(original_native.sa_sigaction,original_native.sa_flags));
                for signo in 1..=64 {
                    if signo!=32 && signo!=33 {
                        assert_eq!(libc::sigismember(&native.sa_mask,signo),libc::sigismember(&original_native.sa_mask,signo));
                    }
                }
                assert_eq!(rudroid_signal_syscall_register_probe(address(c"syscall")),1);
                // Six-argument non-target call and an actual pointer-sized
                // return: touch only this child's newly allocated page.
                let mapped=syscall(libc::SYS_mmap,0 as libc::c_long,4096 as libc::c_long,
                    (libc::PROT_READ|libc::PROT_WRITE) as libc::c_long,
                    (libc::MAP_PRIVATE|libc::MAP_ANONYMOUS) as libc::c_long,
                    -1 as libc::c_long,0 as libc::c_long);
                assert_ne!(mapped,-1);
                (mapped as *mut u8).write(73);
                assert_eq!((mapped as *const u8).read(),73);
                assert_eq!(libc::munmap(mapped as *mut std::ffi::c_void,4096),0);
                *libc::__errno_location() = 0;
                assert_eq!(action(0, std::ptr::null(), std::ptr::null_mut()), -1);
                assert_eq!(*libc::__errno_location(), libc::EINVAL);
            }
            return;
        }
        for (case, count) in [("quiet", 0), ("enabled", 11), ("limit", 64)] {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env("RUDROID_SIGNAL_METADATA_TEST", case)
                .output().unwrap();
            assert!(child.status.success(), "{case}: {}", String::from_utf8_lossy(&child.stderr));
            let stderr = String::from_utf8(child.stderr).unwrap();
            let records: Vec<_> = stderr.lines().filter(|line| line.starts_with("[BIONIC signal_action]")).collect();
            assert_eq!(records.len(), count, "{case}: {stderr}");
            for line in records {
                assert!(line.contains(" tid=0x") && line.contains(" errno=0x"), "{line}");
                assert!(!line.contains(" sig=0x0 "), "{line}");
            }
            if case != "quiet" {
                assert!(stderr.contains("api=sigaction op=set sig=0x7 handler=0x"), "{stderr}");
                assert!(stderr.contains("api=signal op=set sig=0x7 handler=0x1 flags=none"), "{stderr}");
            }
            if case == "enabled" {
                assert!(stderr.contains("api=syscall op=query sig=0x6 handler=none flags=none rc=0 errno=0x47"), "{stderr}");
                assert!(stderr.contains("api=syscall op=query sig=0x7 handler=none flags=none rc=-1 errno=0x16"), "{stderr}");
            }
        }
    }
    #[test]
    fn syslog_identity_is_owned_and_closelog_keeps_native_semantics() {
        if std::env::var_os("RUDROID_BIONIC_SYSLOG_TEST").is_some() {
            unsafe {
                let open: unsafe extern "C" fn(*const c_char, i32, i32) =
                    std::mem::transmute(address(c"openlog"));
                let log: unsafe extern "C" fn(i32, *const c_char, ...) =
                    std::mem::transmute(address(c"syslog"));
                let close: unsafe extern "C" fn() = std::mem::transmute(address(c"closelog"));
                let mut tag = b"owned-log\0".to_vec();
                open(tag.as_ptr().cast(), 0x20, 8); // LOG_PERROR / LOG_USER.
                tag.fill(b'x');
                drop(tag);
                log(6, c"value=%d".as_ptr(), 42);
                close();
                log(6, c"after-close=%d".as_ptr(), 73);
                close();
            }
            return;
        }
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::syslog_identity_is_owned_and_closelog_keeps_native_semantics",
                "--nocapture",
            ])
            .env("RUDROID_BIONIC_SYSLOG_TEST", "1")
            .output()
            .unwrap();
        assert!(child.status.success());
        let stderr = String::from_utf8(child.stderr).unwrap();
        assert!(stderr.contains("owned-log: value=42"), "{stderr}");
        assert!(stderr.contains(": after-close=73"), "{stderr}");
    }
    #[test]
    fn fatal_diagnostics_copy_the_first_message_and_really_abort() {
        if let Some(case) = std::env::var_os("RUDROID_BIONIC_FATAL_TEST") {
            unsafe {
                let limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                assert_eq!(libc::setrlimit(libc::RLIMIT_CORE, &limit), 0);
                let set: unsafe extern "C" fn(*const c_char) =
                    std::mem::transmute(address(c"android_set_abort_message"));
                let mut buffer = b"first message copied\0".to_vec();
                set(buffer.as_ptr().cast());
                buffer.fill(b'x');
                drop(buffer);
                set(c"replacement must be ignored".as_ptr());
                if case == "assert" {
                    let assertion: unsafe extern "C" fn(
                        *const c_char,
                        i32,
                        *const c_char,
                        *const c_char,
                    ) -> ! = std::mem::transmute(address(c"__assert2"));
                    assertion(
                        c"guest.c".as_ptr(),
                        42,
                        c"worker".as_ptr(),
                        c"state != 0".as_ptr(),
                    );
                }
                let name = if case == "stack" {
                    c"__stack_chk_fail"
                } else {
                    c"abort"
                };
                let abort: unsafe extern "C" fn() -> ! = std::mem::transmute(address(name));
                abort();
            }
        }
        for case in ["abort", "assert", "stack"] {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "linux::fatal_diagnostics_copy_the_first_message_and_really_abort",
                    "--nocapture",
                ])
                .env("RUDROID_BIONIC_FATAL_TEST", case)
                .output()
                .unwrap();
            assert_eq!(child.status.signal(), Some(libc::SIGABRT));
            let stderr = String::from_utf8(child.stderr).unwrap();
            assert!(
                stderr.contains("[BIONIC] abort: first message copied"),
                "{stderr}"
            );
            assert!(!stderr.contains("replacement must be ignored"));
            if case == "assert" {
                assert!(stderr.contains("guest.c worker state != 0 (line 42)"));
            }
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU Bionic diagnostics; use docker-rust.sh test"]
fn fatal_diagnostics_copy_the_first_message_and_really_abort() {
    panic!("run on Linux ARM64 GNU");
}
