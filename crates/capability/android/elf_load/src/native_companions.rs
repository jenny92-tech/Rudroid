//! INPUT:  loader-approved directory, exact Steamworks companion basenames and GNU libdl
//! OUTPUT:  process-pinned native code, independent revocable guest handles and scoped native symbol lookup
//! POS:    loads only two approved host companions outside registry locks and rejects constructor reentry
#![deny(unsafe_op_in_unsafe_fn)]
use rudroid_common::{
    handles::HandleTable,
    host_api::{HostApiV0, HOST_API_VERSION},
};
use std::{
    ffi::{c_void, CStr, CString},
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
enum State {
    Empty,
    Loading,
    Ready(Arc<NativeModule>),
}
struct NativeModule {
    handle: usize,
}
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static MODULES: [Mutex<State>; 2] = [const { Mutex::new(State::Empty) }; 2];
static HANDLES: HandleTable<NativeModule> = HandleTable::new();

pub(super) fn approved(name: &CStr) -> Option<(usize, &'static str)> {
    match name.to_bytes() {
        b"steam_api64" | b"steam_api64.so" | b"libsteam_api64" | b"libsteam_api64.so" => {
            Some((0, "libsteam_api64.so"))
        }
        b"sdkencryptedappticket64"
        | b"sdkencryptedappticket64.so"
        | b"libsdkencryptedappticket64"
        | b"libsdkencryptedappticket64.so" => Some((1, "libsdkencryptedappticket64.so")),
        _ => None,
    }
}
fn lock(index: usize) -> std::sync::MutexGuard<'static, State> {
    MODULES[index].lock().unwrap_or_else(|e| e.into_inner())
}
fn directory(path: PathBuf) -> Result<(), String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("native companion directory: {e}"))?;
    if !path.is_dir() {
        return Err("native companion directory is not a directory".into());
    }
    if let Some(old) = DIRECTORY.get() {
        return if *old == path {
            Ok(())
        } else {
            Err("native companion directory is already pinned".into())
        };
    }
    DIRECTORY
        .set(path)
        .map_err(|_| "native companion directory publication conflict".into())
}

#[no_mangle]
extern "C" fn rudroid_plugin_init(host: Option<&HostApiV0>) -> i32 {
    let Some(host) = host else {
        return -1;
    };
    if host.abi_version != HOST_API_VERSION {
        return -2;
    }
    let path = host
        .config_string(c"paths.native_companion_dir", 4096)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(std::env::current_dir);
    match path.map_err(|e| e.to_string()).and_then(directory) {
        Ok(()) => 0,
        Err(reason) => {
            host.log_line(c"ELF", &reason);
            -3
        }
    }
}

pub(super) fn open(name: &CStr, no_load: bool) -> Result<*mut c_void, String> {
    let (index, basename) = approved(name).ok_or("unapproved native companion")?;
    {
        let mut state = lock(index);
        match &*state {
            State::Ready(module) => return Ok(HANDLES.insert_arc(Arc::clone(module))),
            State::Loading => return Err(
                "native companion load is already in progress; recursive/concurrent open refused"
                    .into(),
            ),
            State::Empty if no_load => {
                return Err("native companion is not loaded (RTLD_NOLOAD)".into())
            }
            State::Empty => *state = State::Loading,
        }
    }
    // Constructors and transitive host loading occur outside every registry/module/handle lock.
    let result = load(basename);
    let mut state = lock(index);
    match result {
        Ok(module) => {
            let module = Arc::new(module);
            *state = State::Ready(Arc::clone(&module));
            Ok(HANDLES.insert_arc(module))
        }
        Err(reason) => {
            *state = State::Empty;
            Err(reason)
        }
    }
}
fn load(name: &str) -> Result<NativeModule, String> {
    let root = DIRECTORY
        .get()
        .ok_or("native companion directory not configured")?;
    let path = root
        .join(name)
        .canonicalize()
        .map_err(|e| format!("{name}: {e}"))?;
    if !path.starts_with(root) {
        return Err("native companion symlink escapes approved directory".into());
    }
    let mut header = [0u8; 64];
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| e.to_string())?;
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || u16::from_le_bytes([header[16], header[17]]) != 3
        || u16::from_le_bytes([header[18], header[19]]) != 183
    {
        return Err("native companion must be ELF64 LE AArch64 shared code".into());
    }
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| "native companion path contains NUL")?;
    // SAFETY: loader authorizes trusted immutable host code under one directory; exact native host ABI,
    // RTLD_NOW|LOCAL, constructors outside locks. Successful handles intentionally remain process-pinned.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        // SAFETY: GNU libdl owns this thread-local terminated error until the next libdl operation.
        let reason = unsafe {
            let error = libc::dlerror();
            if error.is_null() {
                "native dlopen failed".into()
            } else {
                CStr::from_ptr(error).to_string_lossy().into_owned()
            }
        };
        Err(format!("{name}: {reason}"))
    } else {
        Ok(NativeModule {
            handle: handle as usize,
        })
    }
}
pub(super) fn is_handle(handle: *mut c_void) -> bool {
    HANDLES.get(handle).is_some()
}
pub(super) fn close(handle: *mut c_void) -> bool {
    HANDLES.remove(handle).is_some()
}
pub(super) fn symbol(handle: *mut c_void, name: &CStr) -> Option<usize> {
    let module = HANDLES.get(handle)?;
    // SAFETY: module authority pins host code for the process; name is terminated for this call.
    let address = unsafe { libc::dlsym(module.handle as *mut c_void, name.as_ptr()) };
    (!address.is_null()).then_some(address as usize)
}

#[cfg(test)]
#[path = "../tests/unit/native_companions.rs"]
mod tests;
