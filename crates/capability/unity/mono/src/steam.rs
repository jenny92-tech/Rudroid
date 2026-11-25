//! INPUT: exact symbol names on an explicitly declared offline Steam P/Invoke handle
//! OUTPUT: Steamworks no-client results for startup and shutdown functions
//! POS: the offline handle has no interfaces and unknown exports remain missing
//!
//! No-client behavior adapted from PortMaster-Unity runtime/host/steamstub.c.
//! Copyright (c) 2026 wild1walker (MIT). See THIRD-PARTY-NOTICES.
#![forbid(unsafe_code)]

use std::ffi::{c_char, c_int, c_void, CStr};

extern "C" fn init() -> c_int {
    0
}
extern "C" fn restart(_: u32) -> c_int {
    0
}
extern "C" fn shutdown() {}
extern "C" fn no_handle() -> c_int {
    0
}
extern "C" fn no_interface() -> *mut c_void {
    std::ptr::null_mut()
}
extern "C" fn empty_path() -> *const c_char {
    c"".as_ptr()
}

use std::mem::MaybeUninit;

const NO_CLIENT_MSG: &[u8; 21] = b"Steam is not running\0";

fn no_client(message: Option<&mut [MaybeUninit<u8>; 21]>) -> c_int {
    if let Some(message) = message {
        for (out, byte) in message.iter_mut().zip(NO_CLIENT_MSG) {
            out.write(*byte);
        }
    }
    2 // k_ESteamAPIInitResult_NoSteamClient
}

extern "C" fn init_flat(message: Option<&mut [MaybeUninit<u8>; 21]>) -> c_int {
    no_client(message)
}

extern "C" fn internal_init(_: *const c_char, message: Option<&mut [MaybeUninit<u8>; 21]>) -> c_int {
    no_client(message)
}

pub(super) fn symbol(name: &CStr) -> usize {
    match name.to_bytes() {
        b"SteamAPI_Init" | b"SteamAPI_InitSafe" | b"SteamAPI_IsSteamRunning" => {
            init as *const () as usize
        }
        b"SteamAPI_RestartAppIfNecessary" => restart as *const () as usize,
        b"SteamAPI_InitFlat" => init_flat as *const () as usize,
        b"SteamInternal_SteamAPI_Init" => internal_init as *const () as usize,
        b"SteamAPI_Shutdown"
        | b"SteamAPI_RunCallbacks"
        | b"SteamAPI_ReleaseCurrentThreadMemory" => shutdown as *const () as usize,
        b"SteamAPI_GetHSteamPipe" | b"SteamAPI_GetHSteamUser" => no_handle as *const () as usize,
        b"SteamClient" | b"SteamGameServerClient" => no_interface as *const () as usize,
        b"SteamAPI_GetSteamInstallPath" => empty_path as *const () as usize,
        _ => 0,
    }
}
