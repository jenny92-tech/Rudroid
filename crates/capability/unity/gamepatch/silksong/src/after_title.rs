//! INPUT:  typed IL2CPP metadata, player-frame/stop events and a title-delay setting
//! OUTPUT:  once-only title-scene bundle reclamation with cancellation and truthful outcomes
//! POS:    owns the title unload policy and confines all managed calls to its player thread
#![deny(unsafe_op_in_unsafe_fn)]

use rudroid_common::host_api::HostApiV0;
use rudroid_common::il2cpp::{Il2cpp, Method, TYPE_BOOLEAN, TYPE_STRING, TYPE_VOID};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::ThreadId;

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Waiting,
    Counting,
    Cancelled,
    Stopped,
    Unload,
}

#[derive(Default)]
struct Gate {
    remaining: u32,
    seen_title: bool,
    finished: bool,
}

impl Gate {
    fn frame(&mut self, in_title: bool, delay: u32) -> Step {
        if self.finished {
            return Step::Waiting;
        }
        if !in_title {
            if self.seen_title {
                self.finished = true;
                return Step::Cancelled;
            }
            return Step::Waiting;
        }
        if !self.seen_title {
            self.seen_title = true;
            self.remaining = delay;
        }
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            // Consume BEFORE entering guest code: reentry, managed failure
            // and returning to the title must never retry a bulk unload.
            self.finished = true;
            Step::Unload
        } else {
            Step::Counting
        }
    }
}

struct Engine {
    api: Il2cpp,
    scene_name: Method,
    unload: Method,
    gate: Gate,
    delay: u32,
    // Created and used only on Runtime's owning player thread. No managed
    // object is stored between frames; this engine itself is !Send/!Sync.
    _thread: PhantomData<*const ()>,
}

impl Engine {
    fn discover(api: Il2cpp, delay: u32) -> Result<Self, &'static str> {
        if !api.thread_attached() {
            return Err("player thread is not attached to IL2CPP");
        }
        if !api.has("il2cpp_string_length") || !api.has("il2cpp_string_chars") {
            return Err("IL2CPP string API unavailable");
        }
        let application = api
            .find_class(c"UnityEngine", c"Application")
            .ok_or("Application metadata unavailable")?;
        // Unity 6 implements this getter as GetActiveScene().name. Calling
        // it through runtime_invoke avoids guessing the native Scene layout.
        let scene_name = api
            .method(application, c"get_loadedLevelName", 0)
            .filter(|m| api.method_signature(*m, true, &[], TYPE_STRING))
            .ok_or("active scene getter signature unavailable")?;
        let bundle = api
            .find_class(c"UnityEngine", c"AssetBundle")
            .ok_or("AssetBundle metadata unavailable")?;
        let unload = api
            .method(bundle, c"UnloadAllAssetBundles", 1)
            .filter(|m| api.method_signature(*m, true, &[(TYPE_BOOLEAN, false)], TYPE_VOID))
            .ok_or("UnloadAllAssetBundles(bool) signature unavailable")?;
        Ok(Self {
            api,
            scene_name,
            unload,
            gate: Gate::default(),
            delay,
            _thread: PhantomData,
        })
    }

    fn frame(&mut self, stopped: &AtomicBool) -> Result<Step, &'static str> {
        if stopped.load(Ordering::Acquire) {
            return Ok(Step::Stopped);
        }
        if !self.api.thread_attached() {
            return Err("player thread detached from IL2CPP");
        }
        // SAFETY: metadata-validated static zero-argument String getter on
        // the attached owning player thread; code/metadata stay mapped.
        let name = unsafe {
            self.api
                .invoke(self.scene_name, std::ptr::null_mut(), std::ptr::null_mut())
        }
        .map_err(|()| "active scene getter threw a managed exception")?;
        if stopped.load(Ordering::Acquire) {
            return Ok(Step::Stopped);
        }
        if name.is_null() {
            return Err("active scene getter returned null");
        }
        // SAFETY: the validated getter just returned this live managed
        // String. string_eq keeps its native-stack owner through the copy;
        // no managed pointer is retained after this frame.
        let in_title = unsafe { self.api.string_eq(name, "Menu_Title") };
        if stopped.load(Ordering::Acquire) {
            return Ok(Step::Stopped);
        }
        let step = self.gate.frame(in_title, self.delay);
        if step == Step::Unload {
            let mut keep_loaded_objects = false;
            let mut args = [&mut keep_loaded_objects as *mut bool as *mut c_void];
            // SAFETY: validated static void(bool) method; runtime_invoke
            // receives one native bool by address on the player thread.
            // Unity enumerates its bundles; Rust never reads array offsets,
            // owns their pointers or frees guest allocations directly.
            unsafe {
                self.api
                    .invoke(self.unload, std::ptr::null_mut(), args.as_mut_ptr())
            }
            .map_err(|()| "UnloadAllAssetBundles(false) threw a managed exception")?;
        }
        Ok(step)
    }
}

enum Slot {
    New,
    Active(Engine),
    Finished,
}

thread_local! {
    static SLOT: RefCell<Option<(usize, Slot)>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.with(|busy| busy.set(false));
    }
}

pub(super) struct Runtime {
    host: HostApiV0,
    delay: u32,
    binding: Mutex<Option<Il2cpp>>,
    owner: Mutex<Option<ThreadId>>,
    stopped: AtomicBool,
}

impl Runtime {
    pub(super) fn new(host: HostApiV0, delay: u32) -> Self {
        Self {
            host,
            delay,
            binding: Mutex::new(None),
            owner: Mutex::new(None),
            stopped: AtomicBool::new(false),
        }
    }

    pub(super) fn ready(&self, api: Il2cpp) {
        if !self.stopped.load(Ordering::Acquire) {
            *super::lock(&self.binding) = Some(api);
        }
    }

    fn player_thread(&self) -> bool {
        let current = std::thread::current().id();
        let mut owner = super::lock(&self.owner);
        match *owner {
            Some(id) => id == current,
            None => {
                *owner = Some(current);
                true
            }
        }
    }

    fn log(&self, message: &str) {
        self.host.log_line(c"SILK", message);
    }

    pub(super) fn stop(&self, identity: usize) {
        if !self.player_thread() {
            return;
        }
        self.stopped.store(true, Ordering::Release);
        *super::lock(&self.binding) = None;
        SLOT.with(|slot| {
            // A reentrant stop must not borrow the active engine. The frame
            // callback keeps only a Finished sentinel here during guest calls.
            if slot
                .borrow()
                .as_ref()
                .is_some_and(|(id, _)| *id == identity)
            {
                slot.replace(Some((identity, Slot::Finished)));
            }
        });
    }

    pub(super) fn frame(&self, identity: usize) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let Some(api) = *super::lock(&self.binding) else {
            return;
        };
        if !self.player_thread() || BUSY.with(|busy| busy.replace(true)) {
            return;
        }
        let _busy = BusyGuard;
        // Release both locks and the RefCell borrow before invoking Unity.
        // An unwinding/reentrant path sees Finished, never a fresh attempt.
        let previous = SLOT.with(|slot| slot.replace(Some((identity, Slot::Finished))));
        let mut slot = match previous {
            Some((id, slot)) if id == identity => slot,
            _ => Slot::New,
        };
        if matches!(slot, Slot::New) {
            slot = match Engine::discover(api, self.delay) {
                Ok(engine) => {
                    self.log(&format!(
                        "after-title bundle unload ready: Menu_Title delay={} rendered frames",
                        self.delay
                    ));
                    Slot::Active(engine)
                }
                Err(reason) => {
                    self.log(&format!("after-title bundle unload unavailable: {reason}"));
                    Slot::Finished
                }
            };
        }
        if let Slot::Active(engine) = &mut slot {
            match engine.frame(&self.stopped) {
                Ok(Step::Waiting | Step::Counting) => {}
                Ok(Step::Stopped) => {
                    slot = Slot::Finished;
                }
                Ok(Step::Cancelled) => {
                    self.log(
                        "after-title bundle unload cancelled: left Menu_Title before delay elapsed",
                    );
                    slot = Slot::Finished;
                }
                Ok(Step::Unload) => {
                    self.log(
                        "AssetBundle.UnloadAllAssetBundles(false) completed once in Menu_Title",
                    );
                    slot = Slot::Finished;
                }
                Err(reason) => {
                    self.log(&format!(
                        "after-title bundle unload stopped without retry: {reason}"
                    ));
                    slot = Slot::Finished;
                }
            }
        }
        if self.stopped.load(Ordering::Acquire) {
            slot = Slot::Finished;
        }
        SLOT.with(|cell| {
            cell.replace(Some((identity, slot)));
        });
    }
}

#[cfg(test)]
#[path = "../tests/unit/after_title.rs"]
mod tests;
