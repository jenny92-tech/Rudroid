//! INPUT: dotted configuration keys and caller-supplied defaults
//! OUTPUT: typed settings from either a parsed config or the capability ABI
//! POS: shared gamepatch configuration contract without a host-config dependency
#![forbid(unsafe_code)]
use crate::host_api::HostApiV0;
use std::ffi::CString;

pub trait ConfigReader {
    fn boolean(&self, key: &str, fallback: bool) -> bool;
    fn integer(&self, key: &str, fallback: i64) -> i64;
    fn number(&self, key: &str, fallback: f64) -> f64;
    fn text(&self, key: &str, fallback: &str) -> String;
}

impl ConfigReader for HostApiV0 {
    fn boolean(&self, key: &str, fallback: bool) -> bool {
        CString::new(key).map_or(fallback, |key| self.config_bool(&key, fallback))
    }
    fn integer(&self, key: &str, fallback: i64) -> i64 {
        CString::new(key).map_or(fallback, |key| self.config_i64(&key, fallback))
    }
    fn number(&self, key: &str, fallback: f64) -> f64 {
        CString::new(key).map_or(fallback, |key| self.config_f64(&key, fallback))
    }
    fn text(&self, key: &str, fallback: &str) -> String {
        CString::new(key)
            .ok()
            .and_then(|key| self.config_string(&key, 512))
            .unwrap_or_else(|| fallback.to_owned())
    }
}
