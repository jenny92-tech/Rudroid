//! INPUT:  typed CinematicSequence metadata, unavailable Android video backend and live callbacks
//! OUTPUT:  original Begin followed by the game's normal Skip path
//! POS:    narrow unsupported-video boundary; no decoder, managed object cache or layout guesses
#![deny(unsafe_op_in_unsafe_fn)]
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::{atomic::{AtomicBool, Ordering}, Mutex};
use rudroid_common::arm64_detour::Original;
use rudroid_common::il2cpp::{Il2cpp, Method};
use super::{InstallEvent, SilksongFeatures, UpdateFn};

static ORIGINAL: Original<UpdateFn> = Original::new();
static CONTEXT: Mutex<Option<(Il2cpp, Method)>> = Mutex::new(None);
static STOPPED: AtomicBool = AtomicBool::new(true);
thread_local! { static BUSY: Cell<bool> = const { Cell::new(false) }; }
struct Guard;
impl Drop for Guard { fn drop(&mut self) { BUSY.with(|v| v.set(false)); } }
pub(super) fn stop() { STOPPED.store(true, Ordering::Release); }

unsafe extern "C-unwind" fn begin(receiver: *mut c_void, metadata: *mut c_void) {
    if let Some(original) = ORIGINAL.get() {
        // SAFETY: validated instance void Begin(), live callback receiver.
        unsafe { original(receiver, metadata) };
    }
    if receiver.is_null() || STOPPED.load(Ordering::Acquire) { return; }
    let Some((api, skip)) = *super::lock(&CONTEXT) else { return; };
    if !api.thread_attached() || BUSY.with(|v| v.replace(true)) { return; }
    let _guard = Guard;
    // SAFETY: instance void Skip() belongs to the same validated class;
    // receiver is kept reachable on this active attached callback's stack.
    let result = unsafe {
        let result = api.invoke(skip, receiver, std::ptr::null_mut());
        std::ptr::read_volatile(std::ptr::addr_of!(receiver));
        result
    };
    match result {
        Ok(_) => super::host_log_raw("unsupported cinematic: normal Skip requested after Begin"),
        Err(_) => { stop(); super::host_log_raw("unsupported cinematic Skip threw; further requests disabled"); }
    }
}

/// # Safety
/// The host provides quiescent targets and process-pinned code/metadata.
pub(super) unsafe fn install(api: Il2cpp, features: &SilksongFeatures) -> Vec<InstallEvent> {
    stop(); *super::lock(&CONTEXT) = None;
    if !features.skip_unsupported_cinematics { return vec![InstallEvent::Skipped { behavior: "skip_unsupported_cinematics", reason: "switch off" }]; }
    let a = super::method_entry(&api, "", "CinematicSequence", "Begin", 0);
    let b = super::method_entry(&api, "", "CinematicSequence", "Skip", 0);
    let (Some((entry, _)), Some((_, skip))) = (a, b) else {
        return vec![InstallEvent::TargetMissing { behavior: "skip_unsupported_cinematics", target: "CinematicSequence.Begin/Skip signatures".to_owned() }];
    };
    // SAFETY: both callbacks have complete matching instance void() metadata;
    // the host's install window keeps the Begin target idle.
    match unsafe { super::detour("skip_unsupported_cinematics", "CinematicSequence.Begin", entry, &ORIGINAL, begin) } {
        Ok(hook) => {
            *super::lock(&CONTEXT) = Some((api, skip)); STOPPED.store(false, Ordering::Release);
            vec![InstallEvent::Installed { behavior: "skip_unsupported_cinematics", target: "CinematicSequence.Begin".to_owned(), shape: hook.shape }]
        }
        Err(e) => vec![e],
    }
}
