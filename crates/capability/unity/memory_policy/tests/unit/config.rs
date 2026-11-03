//! INPUT: default and explicitly enabled memory config through the safe Host API
//! OUTPUT: notification-only default and explicit-unload/GC preservation assertions
//! POS: guards against silently adding managed collection during plugin installation
#![forbid(unsafe_code)]
use super::*;
use rudroid_common::registry::HostStr;

#[test]
fn installation_defaults_to_notification_without_extra_unload_or_gc() {
    extern "C" fn boolean(_: HostStr<'_>, fallback: i32) -> i32 {
        fallback
    }
    extern "C" fn integer(_: HostStr<'_>, fallback: i64) -> i64 {
        fallback
    }
    let mut host = HostApiV0::stubbed();
    host.config_get_bool = boolean;
    host.config_get_i64 = integer;
    let defaults = Config::read(&host).unwrap();
    assert!(defaults.notify);
    assert!(!defaults.unload);
    assert!(!defaults.collect_managed);
    extern "C" fn explicit(key: HostStr<'_>, _: i32) -> i32 {
        i32::from(key.to_cstr() != Some(c"unity_memory.diagnostic_only"))
    }
    host.config_get_bool = explicit;
    let opt_in = Config::read(&host).unwrap();
    assert!(opt_in.unload && opt_in.collect_managed);
    assert!(!defaults.diagnostic_only && !opt_in.diagnostic_only);
}

#[test]
fn diagnostic_mode_forces_all_actions_off_and_bounds_sampling() {
    extern "C" fn boolean(_: HostStr<'_>, _: i32) -> i32 { 1 }
    extern "C" fn integer(_: HostStr<'_>, fallback: i64) -> i64 { fallback }
    let mut host = HostApiV0::stubbed();
    host.config_get_bool = boolean;
    host.config_get_i64 = integer;
    let config = Config::read(&host).unwrap();
    assert!(config.diagnostic_only);
    assert!(!config.notify && !config.unload && !config.collect_managed);
    assert_eq!(config.diagnostic_samples, 32);
    extern "C" fn invalid(key: HostStr<'_>, fallback: i64) -> i64 {
        if key.to_cstr() == Some(c"unity_memory.diagnostic_interval_ms") { 999 } else { fallback }
    }
    host.config_get_i64 = invalid;
    assert!(Config::read(&host).is_err());
    extern "C" fn invalid_count(key: HostStr<'_>, fallback: i64) -> i64 {
        if key.to_cstr() == Some(c"unity_memory.diagnostic_samples") { 121 } else { fallback }
    }
    host.config_get_i64 = invalid_count;
    assert!(Config::read(&host).is_err());
}
