//! INPUT:  bounded snapshot bytes or borrowed paths/files and Linux filesystem/fd APIs
//! OUTPUT:  FilesystemStats and bounded kernel-sealed snapshot File owners
//! POS:    central native filesystem queries and immutable fd ownership for loader/Android services
use std::{fs::File, io, path::Path};

/// A small immutable file owner. Each /proc/self/fd reopen gets its own cursor;
/// kernel seals reject mutation even for privileged callers and writable reopens.
pub fn sealed_snapshot(bytes: &[u8]) -> io::Result<File> {
    if bytes.len() > 65536 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "snapshot exceeds 64 KiB"));
    }
    #[cfg(target_os = "linux")]
    {
        use std::{io::{Seek, Write}, os::fd::{AsRawFd, FromRawFd}};
        // SAFETY: fixed terminated name/flags; a successful fd is immediately
        // adopted exactly once. All subsequent failures drop that File owner.
        let mut file = unsafe {
            let fd = libc::memfd_create(c"rudroid-snapshot".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING);
            if fd < 0 { return Err(io::Error::last_os_error()); }
            File::from_raw_fd(fd)
        };
        file.write_all(bytes)?;
        file.rewind()?;
        // SAFETY: the live private File owns fd; fcntl takes only scalar flags.
        // No writable mapping exists; failure never publishes an unsealed file.
        let result = unsafe {
            libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS,
                libc::F_SEAL_WRITE | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL)
        };
        if result != 0 { return Err(io::Error::last_os_error()); }
        Ok(file)
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::new(io::ErrorKind::Unsupported, "sealed snapshots require Linux"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilesystemStats {
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub noexec: bool,
}
impl FilesystemStats {
    /// Snapshot the filesystem containing an existing path; do not open the
    /// file for reading or create missing paths to make a failed query succeed.
    pub fn for_path(path: &Path) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "filesystem path contains NUL")
            })?;
            query(Query::Path(&name))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "filesystem statistics require Linux",
            ))
        }
    }
    /// Borrow the already-open owner, avoiding a pathname race or another open.
    pub fn for_file(file: &File) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            query(Query::File(file))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = file;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "filesystem statistics require Linux",
            ))
        }
    }
}
#[cfg(target_os = "linux")]
enum Query<'a> {
    Path(&'a std::ffi::CStr),
    File(&'a File),
}
#[cfg(target_os = "linux")]
fn query(target: Query<'_>) -> io::Result<FilesystemStats> {
    use std::os::fd::AsRawFd;
    // SAFETY: libc::statvfs contains integer fields, so zero is valid even for
    // reserved fields not written by libc. The borrowed CStr is terminated,
    // the borrowed File keeps its fd live, and native output points to our
    // writable stack object. Read output only after success; retain no pointer.
    unsafe {
        let mut raw: libc::statvfs = std::mem::zeroed();
        let status = match target {
            Query::Path(name) => libc::statvfs(name.as_ptr(), &mut raw),
            Query::File(file) => libc::fstatvfs(file.as_raw_fd(), &mut raw),
        };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(FilesystemStats {
            block_size: raw.f_frsize as u64,
            blocks: raw.f_blocks as u64,
            free_blocks: raw.f_bfree as u64,
            available_blocks: raw.f_bavail as u64,
            noexec: raw.f_flag & libc::ST_NOEXEC != 0,
        })
    }
}
#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/unit/filesystem.rs"]
mod tests;
