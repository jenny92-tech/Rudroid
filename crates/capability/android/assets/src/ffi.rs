//! INPUT:  trusted NDK buffers/borrows, common handles and the host resource authority
//! OUTPUT:  AAssetManager/AAsset/AAssetDir function addresses and owned fd/map cleanup
//! POS:    the sole native-pointer boundary; handles are lookup identities and never dereferenced
#![deny(unsafe_op_in_unsafe_fn)]
use rudroid_common::{android_assets as query, handles::HandleTable, host_api::HostApiV0};
use std::ffi::{c_char, c_void, CStr, CString};
use std::fs::File;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::sync::{Mutex, OnceLock};

static HOST: OnceLock<HostApiV0> = OnceLock::new();
static MANAGER: u8 = 0;
static ASSETS: HandleTable<Mutex<Asset>> = HandleTable::new();
static DIRECTORIES: HandleTable<Mutex<Directory>> = HandleTable::new();
pub(super) fn initialize(host: &HostApiV0) {
    HOST.get_or_init(|| *host);
}
fn manager() -> *mut c_void {
    (&MANAGER as *const u8).cast_mut().cast()
}

struct Mapping {
    address: usize,
    length: usize,
}
impl Mapping {
    fn open(file: &File, length: u64) -> Option<Self> {
        let length = usize::try_from(length)
            .ok()
            .filter(|n| *n > 0 && *n <= isize::MAX as usize)?;
        // SAFETY: live read-only file fd, validated nonzero size; this creates no Rust reference to file bytes.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            None
        } else {
            Some(Self {
                address: address as usize,
                length,
            })
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this private owner contains exactly one successful mapping; exported borrows end before asset close.
        unsafe {
            libc::munmap(self.address as *mut c_void, self.length);
        }
    }
}
struct Asset {
    file: File,
    length: u64,
    position: u64,
    mapping: Option<Mapping>,
}
struct Directory {
    names: Vec<CString>,
    cursor: usize,
}
fn with_asset<T>(handle: *mut c_void, f: impl FnOnce(&mut Asset) -> T) -> Option<T> {
    let owner = ASSETS.get(handle)?;
    let mut asset = owner.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(&mut asset))
}
extern "C" fn from_java(_env: *mut c_void, _java: *mut c_void) -> *mut c_void {
    if HOST.get().is_some() {
        manager()
    } else {
        std::ptr::null_mut()
    }
}

/// # Safety
/// name is a valid C string. Returned asset is closed once after all calls/buffer borrows end.
unsafe extern "C" fn open(mgr: *mut c_void, name: *const c_char, mode: i32) -> *mut c_void {
    if mgr != manager() || name.is_null() || !(0..=3).contains(&mode) {
        return std::ptr::null_mut();
    }
    let Some(host) = HOST.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: the caller lends a NUL-terminated name for this synchronous query.
    let name = unsafe { CStr::from_ptr(name) };
    let Some(path) = host.asset_path(query::MERGED, name, None, 0).ok().flatten() else {
        return std::ptr::null_mut();
    };
    let Ok(file) = File::open(std::path::Path::new(std::ffi::OsStr::from_bytes(
        path.to_bytes(),
    ))) else {
        return std::ptr::null_mut();
    };
    let Ok(metadata) = file.metadata() else {
        return std::ptr::null_mut();
    };
    if !metadata.is_file() || metadata.len() > i64::MAX as u64 {
        return std::ptr::null_mut();
    }
    ASSETS.insert(Mutex::new(Asset {
        file,
        length: metadata.len(),
        position: 0,
        mapping: None,
    }))
}
extern "C" fn close(asset: *mut c_void) {
    ASSETS.remove(asset);
}
extern "C" fn length(asset: *mut c_void) -> i64 {
    with_asset(asset, |a| a.length as i64).unwrap_or(0)
}
extern "C" fn remaining(asset: *mut c_void) -> i64 {
    with_asset(asset, |a| a.length.saturating_sub(a.position) as i64).unwrap_or(0)
}
extern "C" fn allocated(_asset: *mut c_void) -> i32 {
    0
}

/// # Safety
/// output holds count writable bytes. No output pointer is retained.
unsafe extern "C" fn read(asset: *mut c_void, output: *mut c_void, count: usize) -> i32 {
    if count == 0 {
        return 0;
    }
    if output.is_null() {
        return -1;
    }
    with_asset(asset, |a| {
        let size = count.min(a.length.saturating_sub(a.position).min(i32::MAX as u64) as usize);
        if size == 0 {
            return 0;
        }
        // SAFETY: output holds count bytes, size<=count and i32::MAX; the slice lasts only for the read.
        let bytes = unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), size) };
        match a.file.read_at(bytes, a.position) {
            Ok(n) => {
                a.position += n as u64;
                n as i32
            }
            Err(_) => -1,
        }
    })
    .unwrap_or(-1)
}
extern "C" fn seek(asset: *mut c_void, offset: i64, whence: i32) -> i64 {
    with_asset(asset, |a| {
        let base = match whence {
            libc::SEEK_SET => 0,
            libc::SEEK_CUR => a.position as i128,
            libc::SEEK_END => a.length as i128,
            _ => return -1,
        };
        let position = base + offset as i128;
        if position < 0 || position > i64::MAX as i128 {
            return -1;
        }
        a.position = position as u64;
        position as i64
    })
    .unwrap_or(-1)
}

/// # Safety
/// The returned read-only bytes are borrowed until close; backing files stay
/// untruncated/unmodified. Caller must end all borrows before closing the asset.
unsafe extern "C" fn buffer(asset: *mut c_void) -> *const c_void {
    with_asset(asset, |a| {
        if a.mapping.is_none() {
            a.mapping = Mapping::open(&a.file, a.length);
        }
        a.mapping
            .as_ref()
            .map_or(std::ptr::null(), |m| m.address as *const c_void)
    })
    .unwrap_or(std::ptr::null())
}
/// # Safety
/// start/size point to writable i64 values. The caller owns and closes the returned fd.
unsafe extern "C" fn descriptor(asset: *mut c_void, start: *mut i64, size: *mut i64) -> i32 {
    if start.is_null() || size.is_null() {
        return -1;
    }
    with_asset(asset, |a| match a.file.try_clone() {
        Ok(file) => {
            // SAFETY: caller lends two writable outputs; neither is retained.
            unsafe {
                *start = 0;
                *size = a.length as i64;
            }
            file.into_raw_fd()
        }
        Err(_) => -1,
    })
    .unwrap_or(-1)
}
/// # Safety
/// name is a valid directory C string; returned names are borrowed until dir close.
unsafe extern "C" fn open_directory(mgr: *mut c_void, name: *const c_char) -> *mut c_void {
    if mgr != manager() || name.is_null() {
        return std::ptr::null_mut();
    }
    let Some(host) = HOST.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: name is caller-owned and live for this query.
    let name = unsafe { CStr::from_ptr(name) };
    let Some(bytes) = host
        .asset_query(&query::AssetRequest::new(query::DIRECTORY, name, None, 0))
        .ok()
        .flatten()
    else {
        return std::ptr::null_mut();
    };
    let mut names = Vec::new();
    for part in bytes.split(|b| *b == 0) {
        if part.is_empty() {
            break;
        }
        names.push(CString::new(part).expect("split excludes NUL"));
    }
    DIRECTORIES.insert(Mutex::new(Directory { names, cursor: 0 }))
}
extern "C" fn close_directory(dir: *mut c_void) {
    DIRECTORIES.remove(dir);
}
extern "C" fn rewind_directory(dir: *mut c_void) {
    if let Some(owner) = DIRECTORIES.get(dir) {
        owner.lock().unwrap_or_else(|e| e.into_inner()).cursor = 0;
    }
}
/// # Safety
/// The returned name is borrowed; dir remains open while the caller uses it.
unsafe extern "C" fn next_name(dir: *mut c_void) -> *const c_char {
    let Some(owner) = DIRECTORIES.get(dir) else {
        return std::ptr::null();
    };
    let mut dir = owner.lock().unwrap_or_else(|e| e.into_inner());
    let Some(name) = dir.names.get(dir.cursor) else {
        return std::ptr::null();
    };
    let name = name.as_ptr();
    dir.cursor += 1;
    name
}

/// # Safety
/// Query strings are valid for the call; returned functions require the NDK
/// ARM64 signatures, caller-owned buffers and quiescent handle close contracts.
#[no_mangle]
pub unsafe extern "C" fn rudroid_assets_symbol(
    name: *const c_char,
    library: *const c_char,
    version: *const c_char,
    _user: *mut c_void,
) -> usize {
    if name.is_null() || !library.is_null() || !version.is_null() {
        return 0;
    }
    // SAFETY: the ELF source lends a valid NUL-terminated query name.
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"AAssetManager_fromJava" => from_java as usize,
        b"AAssetManager_open" => open as usize,
        b"AAsset_close" => close as usize,
        b"AAsset_getBuffer" => buffer as usize,
        b"AAsset_getLength" | b"AAsset_getLength64" => length as usize,
        b"AAsset_getRemainingLength" | b"AAsset_getRemainingLength64" => remaining as usize,
        b"AAsset_read" => read as usize,
        b"AAsset_seek" | b"AAsset_seek64" => seek as usize,
        b"AAsset_isAllocated" => allocated as usize,
        b"AAsset_openFileDescriptor" | b"AAsset_openFileDescriptor64" => descriptor as usize,
        b"AAssetManager_openDir" => open_directory as usize,
        b"AAssetDir_close" => close_directory as usize,
        b"AAssetDir_getNextFileName" => next_name as usize,
        b"AAssetDir_rewind" => rewind_directory as usize,
        _ => 0,
    }
}
