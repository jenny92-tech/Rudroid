//! INPUT:  host policy, immutable package asset paths and owned File handles
//! OUTPUT:  FileStream and one shared BufferBudget with automatic leases
//! POS:    bounds optional read-ahead without duplicating assets or crossing Rust ownership
#![forbid(unsafe_code)]

use std::ffi::CStr;
use std::io::{Read, Seek};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub(super) struct BufferBudget {
    per_stream: usize,
    limit: usize,
    used: AtomicUsize,
    asset_root: Option<std::path::PathBuf>,
}

impl BufferBudget {
    pub(super) fn disabled() -> Self {
        Self {
            per_stream: 0,
            limit: 0,
            used: AtomicUsize::new(0),
            asset_root: None,
        }
    }
    pub(super) fn from_host(host: &rudroid_common::host_api::HostApiV0) -> Self {
        // Only resolved numeric values cross the existing dynamic ABI.
        let per_stream = host
            .config_i64(c"runtime.stream_buffer_bytes", 8192)
            .clamp(0, 1024 * 1024) as usize;
        let mib = host
            .config_i64(c"runtime.stream_buffer_budget_mib", 1)
            .clamp(0, 1024) as usize;
        // Absent and empty values both mean "no path".
        let config_path = |key: &CStr| {
            host.config_string(key, 4096)
                .filter(|path| !path.is_empty())
                .map(std::path::PathBuf::from)
        };
        let code = config_path(c"paths.android_package_code");
        let asset_root = code
            .and_then(|p| {
                let root = if p.is_file() {
                    p.parent()?.to_owned()
                } else {
                    p
                };
                root.join("assets").canonicalize().ok()
            })
            .or_else(|| {
                config_path(c"paths.game_files")?
                    .join("assets")
                    .canonicalize()
                    .ok()
            });
        Self {
            per_stream,
            limit: mib * 1024 * 1024,
            used: AtomicUsize::new(0),
            asset_root,
        }
    }

    fn reserve(self: &Arc<Self>) -> Option<BufferLease> {
        if self.per_stream == 0 {
            return None;
        }
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(self.per_stream)
                    .filter(|next| *next <= self.limit)
            })
            .ok()?;
        Some(BufferLease {
            budget: self.clone(),
        })
    }
}

struct BufferLease {
    budget: Arc<BufferBudget>,
}
impl Drop for BufferLease {
    fn drop(&mut self) {
        self.budget
            .used
            .fetch_sub(self.budget.per_stream, Ordering::Relaxed);
    }
}

/// One authoritative stream owner. Optional read-ahead never caches another
/// copy of an entire asset; budget/allocator misses keep direct File reads.
pub(super) struct FileStream {
    file: std::fs::File,
    buffer: Vec<u8>,
    position: usize,
    filled: usize,
    optional_budget: Option<Arc<BufferBudget>>,
    // Declared after the buffer, so its allocation is dropped first.
    _lease: Option<BufferLease>,
    #[cfg(test)]
    file_reads: usize,
}

impl FileStream {
    pub(super) fn new(
        file: std::fs::File,
        path: &std::path::Path,
        budget: &Arc<BufferBudget>,
    ) -> Self {
        // Do not prefetch pipes/devices: preserving their blocking semantics
        // is more important than reducing small regular-file reads.
        // Package assets are immutable. Ordinary files (saves/preferences,
        // etc.) keep unbuffered FileInputStream visibility semantics.
        let is_asset = budget.per_stream > 0
            && budget.limit >= budget.per_stream
            && budget
                .asset_root
                .as_ref()
                .is_some_and(|root| path.canonicalize().is_ok_and(|p| p.starts_with(root)));
        let eligible = is_asset
            .then(|| file.metadata().ok())
            .flatten()
            .is_some_and(|m| m.is_file());
        Self {
            file,
            buffer: Vec::new(),
            position: 0,
            filled: 0,
            optional_budget: eligible.then(|| budget.clone()),
            _lease: None,
            #[cfg(test)]
            file_reads: 0,
        }
    }

    pub(super) fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.file.metadata()
    }
    pub(super) fn stream_position(&mut self) -> std::io::Result<u64> {
        self.file
            .stream_position()
            .map(|p| p - (self.filled - self.position) as u64)
    }
}

impl Read for FileStream {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.position == self.filled {
            self.position = 0;
            self.filled = 0;
            // Large reads never allocate an unused read-ahead buffer.
            if self
                .optional_budget
                .as_ref()
                .is_some_and(|b| output.len() < b.per_stream)
            {
                let budget = self.optional_budget.take().unwrap();
                if let Some(lease) = budget.reserve() {
                    if self.buffer.try_reserve_exact(budget.per_stream).is_ok() {
                        self.buffer.resize(budget.per_stream, 0);
                        self._lease = Some(lease);
                    }
                }
            }
            #[cfg(test)]
            {
                self.file_reads += 1;
            }
            // Large reads already amortize I/O; avoid an additional copy.
            if output.len() >= self.buffer.len() {
                return self.file.read(output);
            }
            self.filled = self.file.read(&mut self.buffer)?;
        }
        let count = output.len().min(self.filled - self.position);
        output[..count].copy_from_slice(&self.buffer[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

#[cfg(test)]
#[path = "../tests/unit/stream_buffer.rs"]
mod tests;
