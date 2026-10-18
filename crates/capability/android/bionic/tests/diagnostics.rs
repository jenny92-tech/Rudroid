//! INPUT:  production diagnostic addresses, mutable caller text and isolated child processes
//! OUTPUT:  first-message ownership, assertion/stack diagnostics and real SIGABRT checks
//! POS:    verifies fatal paths without aborting the parent suite or producing core files

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
mod linux {
    use std::ffi::{c_char, CStr};
    use std::os::unix::process::ExitStatusExt;

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
