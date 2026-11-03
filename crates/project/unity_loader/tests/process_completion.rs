//! INPUT: loader process completion boundary and an isolated native atexit callback
//! OUTPUT: child exit-status, buffered diagnostics and no post-teardown destructor regressions
//! POS: verifies executable termination without bypassing the existing player lifecycle tests
#[cfg(unix)]
mod unix {
    use std::{path::PathBuf, process::Command};
    const MARKER: &str = "RUDROID_COMPLETION_TEST_MARKER";
    const STATUS: &str = "RUDROID_COMPLETION_TEST_STATUS";
    extern "C" fn retired_guest_destructor() {
        std::fs::write(std::env::var_os(MARKER).unwrap(), b"invalid post-teardown callback").unwrap();
    }
    #[test]
    #[ignore = "invoked only as an isolated child by completed_guest_flushes_output_without_replaying_finalizers"]
    fn completion_child() {
        let _: PathBuf = std::env::var_os(MARKER).expect("isolated child marker required").into();
        extern "C" { fn atexit(callback: extern "C" fn()) -> i32; }
        // SAFETY: static callback and no borrowed state. Parent proves it is not
        // replayed after explicit completion; a normal exit would create its marker.
        assert_eq!(unsafe { atexit(retired_guest_destructor) }, 0);
        print!("completed-player buffered-output");
        let status = std::env::var(STATUS).unwrap().parse().unwrap();
        rudroid_unity_loader::terminate_completed_guest(&[], status);
    }
    #[test]
    fn completed_guest_flushes_output_without_replaying_finalizers() {
        let root = std::env::temp_dir().join(format!("rudroid-completion-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        for status in [0, 1] {
            let marker = root.join(format!("finalizer-{status}"));
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["unix::completion_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
                .env(MARKER, &marker).env(STATUS, status.to_string()).output().unwrap();
            assert_eq!(output.status.code(), Some(status), "{:?}", output);
            assert!(String::from_utf8_lossy(&output.stdout).contains("completed-player buffered-output"));
            assert!(!marker.exists(), "retired guest data must not be revisited after completion");
        }
        std::fs::remove_dir(root).unwrap();
    }
}
