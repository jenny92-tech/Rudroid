//! INPUT:  explicit process-snapshot policy and Linux CPU identity/topology files
//! OUTPUT:  one bounded sealed cpuinfo owner and exact-path read projection
//! POS:    resource authority optimization; no guest patches, polling or background worker
#![forbid(unsafe_code)]
use std::{ffi::{CStr, CString}, fs::File, io::{self, Read}, os::fd::AsRawFd};

pub(crate) struct CpuInfoView {
    path: CString,
    _file: File,
}
impl CpuInfoView {
    pub(crate) fn prepare() -> io::Result<Self> {
        let before = std::fs::read_to_string("/sys/devices/system/cpu/online")?;
        let mut bytes = Vec::new();
        File::open("/proc/cpuinfo")?.take(65537).read_to_end(&mut bytes)?;
        let after = std::fs::read_to_string("/sys/devices/system/cpu/online")?;
        if before != after || bytes.is_empty() || bytes.len() > 65536 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "CPU topology changed or cpuinfo is empty/oversized"));
        }
        Self::from_bytes(&bytes)
    }
    fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let file = rudroid_common::filesystem::sealed_snapshot(bytes)?;
        Ok(Self { path: CString::new(format!("/proc/self/fd/{}",file.as_raw_fd())).unwrap(), _file: file })
    }
    pub(crate) fn redirect(&self, path: &CStr) -> Option<Vec<u8>> {
        (path == c"/proc/cpuinfo").then(|| self.path.as_bytes_with_nul().to_vec())
    }
}

#[cfg(test)]
#[path="../tests/unit/cpuinfo_view.rs"]
mod tests;
