//! INPUT: real Host API default sink and child processes with explicit diagnostics settings
//! OUTPUT: quiet-production and opt-in diagnostic stderr assertions
//! POS: proves ordinary output is suppressed while error details remain visible
use std::process::Command;

#[test]
fn production_keeps_errors_and_diagnostics_require_explicit_opt_in() {
    for enabled in [false, true] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--ignored", "--exact", "logging_child", "--nocapture"]);
        command.env_remove("RUDROID_DIAGNOSTICS");
        if enabled { command.env("RUDROID_DIAGNOSTICS", "1"); }
        let result = command.output().unwrap();
        assert!(result.status.success(), "{result:?}");
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.contains("[ERROR] [SDL] operational failure"), "{stderr}");
        assert!(stderr.contains("malformed \u{fffd} context"), "{stderr}");
        assert_eq!(stderr.contains("normal initialization"), enabled, "{stderr}");
        assert_eq!(stderr.contains("diagnostic counter"), enabled, "{stderr}");
    }
}

#[test]
#[ignore = "isolated child invoked by production_keeps_errors_and_diagnostics_require_explicit_opt_in"]
fn logging_child() {
    let api = rudroid_unity_loader::host_api::table();
    api.log_line(c"SDL", "normal initialization");
    api.log_error(c"SDL", "operational failure");
    let invalid = std::ffi::CStr::from_bytes_with_nul(b"malformed \xff context\0").unwrap();
    (api.log)(rudroid_common::registry::HostStr::new(c"ERROR"), rudroid_common::registry::HostStr::new(invalid));
    rudroid_common::diagnostic!("diagnostic counter");
}
