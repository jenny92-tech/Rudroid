//! INPUT:  a caller-supplied virtual address and caller-owned output buffer
//! OUTPUT: bounded read-only bytes or an I/O error; never a pointer dereference
//! POS:    shared kernel-mediated evidence for mapped-image ABI diagnostics
#![forbid(unsafe_code)]

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

/// Bounded evidence for a single preflight. Callers keep the image pinned.
pub struct MappedRegions(String);

impl MappedRegions {
    pub fn snapshot() -> io::Result<Self> {
        use std::io::Read;
        const LIMIT: usize = 4 * 1024 * 1024;
        let mut text = String::new();
        File::open("/proc/self/maps")?.take(LIMIT as u64 + 1).read_to_string(&mut text)?;
        if text.len() > LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "mapping snapshot exceeds limit"));
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str { &self.0 }

    pub fn contains(&self, address: usize, bytes: usize, executable: bool) -> bool {
        Self::contains_in(&self.0, address, bytes, executable)
    }

    pub fn contains_in(snapshot: &str, address: usize, bytes: usize, executable: bool) -> bool {
        let Some(limit) = address.checked_add(bytes) else { return false; };
        if bytes == 0 { return false; }
        snapshot.lines().any(|line| {
            let mut fields = line.split_whitespace();
            let Some((start, end)) = fields.next().and_then(|range| range.split_once('-')) else { return false; };
            let Some(permissions) = fields.next() else { return false; };
            let (Ok(start), Ok(end)) = (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16)) else { return false; };
            start <= address && limit <= end && permissions.as_bytes().first() == Some(&b'r')
                && (!executable || permissions.as_bytes().get(2) == Some(&b'x'))
        })
    }
}

/// Read-only access to this process's mapped memory through `/proc/self/mem`.
///
/// The reader copies only into the caller's initialized buffer. It performs no
/// raw dereference, mapping change, retention of the target range, or write.
/// Callers remain responsible for proving that an address is owned and readable
/// before treating a successful copy as evidence.
#[derive(Debug)]
pub struct MappedMemory {
    memory: File,
}

impl MappedMemory {
    pub fn open() -> io::Result<Self> {
        File::open("/proc/self/mem").map(|memory| Self { memory })
    }

    pub fn read_exact(&self, address: usize, output: &mut [u8]) -> io::Result<()> {
        if output.is_empty() {
            return Ok(());
        }
        let offset = u64::try_from(address)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "address exceeds u64"))?;
        let len = u64::try_from(output.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "length exceeds u64"))?;
        offset.checked_add(len).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "address range overflows")
        })?;
        self.memory.read_exact_at(output, offset)
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/unit/mapped_memory.rs"]
mod tests;
