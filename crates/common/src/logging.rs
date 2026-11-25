//! INPUT: actual Cargo profile, explicit RUDROID_DIAGNOSTICS process flag and diagnostic/error callers
//! OUTPUT: cached diagnostics policy, explicit errors, lazy diagnostics and per-image build profile exports
//! POS: keeps ordinary production runs quiet without hiding failures or formatting disabled traces
#![forbid(unsafe_code)]

use std::sync::OnceLock;

pub const PRODUCTION: bool = cfg!(all(rudroid_release_profile, not(debug_assertions)));

pub fn diagnostics_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("RUDROID_DIAGNOSTICS").is_some_and(|v| v == "1"))
}

pub fn is_error_tag(tag: &str) -> bool {
    matches!(tag, "ERROR" | "FATAL")
}

pub fn stderr(message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::logging::stderr(format_args!("[ERROR] {}", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! diagnostic {
    ($($arg:tt)*) => {
        if $crate::logging::diagnostics_enabled() { $crate::logging::stderr(format_args!($($arg)*)); }
    };
}

#[cfg(rudroid_release_profile)]
#[macro_export]
macro_rules! export_build_profile {
    () => {
        #[cfg(debug_assertions)]
        #[no_mangle]
        pub extern "C" fn rudroid_debug_build_marker() {}
        #[cfg(not(debug_assertions))]
        #[no_mangle]
        pub extern "C" fn rudroid_release_build_marker() {}
    };
}

#[cfg(not(rudroid_release_profile))]
#[macro_export]
macro_rules! export_build_profile {
    () => {
        #[no_mangle]
        pub extern "C" fn rudroid_debug_build_marker() {}
    };
}

#[cfg(test)]
#[path = "../tests/unit/logging.rs"]
mod tests;
