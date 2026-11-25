//! INPUT: thread attachment, JNI local references and process session identity
//! OUTPUT: per-thread state, attachment checks and deterministic detach cleanup
//! POS: shared JNI session thread ownership and TLS caching
#![forbid(unsafe_code)]
use super::{abi, CachedThread, RefTarget, Session, ThreadState, THREAD_KEY, THREAD_KEY_ALLOC};
use std::{ffi::c_int, sync::{Arc, Mutex}};
#[cfg(test)]
use super::{Pending, SWEEP_PROBE};

impl Session {
    /// The calling thread's attach key — allocated on first JNI use and
    /// stable for the thread's lifetime (until DetachCurrentThread). `None`
    /// once the thread's TLS is being destroyed.
    pub(super) fn thread_key() -> Option<u64> {
        THREAD_KEY
            .try_with(|k| {
                if k.0.get() == 0 {
                    k.0.set(THREAD_KEY_ALLOC.fetch_add(1, std::sync::atomic::Ordering::AcqRel));
                }
                k.0.get()
            })
            .ok()
    }

    /// Attach without creating local references or pending exceptions.
    /// All threads share the immutable env table, but own their state.
    pub(super) fn attach_current_thread(&self) -> bool {
        if self.current_thread_is_attached() { return true; }
        let Some(key) = Self::thread_key() else {
            return false;
        };
        THREAD_KEY.with(|thread| {
            self.cache_thread_state(key, &mut thread.1.borrow_mut());
            thread.2.set(self.id);
        });
        true
    }

    pub(super) fn cache_thread_state(&self, key: u64, cached: &mut Option<CachedThread>) {
        if cached.as_ref().is_some_and(|entry| entry.session == self.id && entry.key == key) {
            return;
        }
        let state = self.threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(ThreadState::default())))
            .clone();
        *cached = Some(CachedThread { session: self.id, key, state });
    }

    /// Read the existing TLS key without allocating one. A key belonging
    /// to another Session is not an attachment to this session.
    pub(super) fn current_thread_is_attached(&self) -> bool {
        THREAD_KEY.try_with(|thread| {
            let key = thread.0.get();
            key != 0 && (thread.2.get() == self.id || self.threads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&key))
        }).unwrap_or(false)
    }

    /// Route one operation through the calling thread's own state
    /// (ticket 88). The closure must not re-enter `with_thread` (its state
    /// lock is held across it). A thread whose TLS is already gone gets a
    /// throwaway state: nothing it creates can outlive the call.
    pub(super) fn with_thread<R>(&self, f: impl FnOnce(&mut ThreadState) -> R) -> R {
        let Some(key) = Self::thread_key() else {
            let mut state = ThreadState::default();
            let result = f(&mut state);
            for local in state.locals.into_values() { self.release_target(local.target); }
            if let Some(pending) = state.pending { self.release_target(RefTarget::Throwable(pending.id)); }
            return result;
        };
        THREAD_KEY.with(|thread| {
            let mut cached = thread.1.borrow_mut();
            self.cache_thread_state(key, &mut cached);
            thread.2.set(self.id);
            let mut state = cached.as_ref().unwrap().state
                .lock().unwrap_or_else(|p| p.into_inner());
            f(&mut state)
        })
    }

    /// DetachCurrentThread (ticket 88): drop the thread's locals and
    /// pending exception and withdraw its attach key.
    pub(super) fn detach_current_thread(&self) -> c_int {
        if self.current_thread_is_attached() && self.with_thread(|s| s.native_floor.is_some()) {
            return abi::JNI_ERR;
        }
        let key = THREAD_KEY.try_with(|thread| {
            let key = thread.0.replace(0);
            thread.2.set(0);
            thread.1.borrow_mut().take();
            key
        }).unwrap_or(0);
        if key != 0 {
            self.detach_key(key);
        }
        0
    }

    /// Detach cleanup for attach key `key`, shared by DetachCurrentThread
    /// and the thread-exit destructor: drops the thread record (locals,
    /// frames, pending), releases the monitors it still holds, and
    /// reclaims entities only its locals were pinning. Never logs and never
    /// touches thread-local storage.
    pub(super) fn detach_key(&self, key: u64) {
        let removed = self.threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        {
            let mut monitors = self.monitors.lock().unwrap_or_else(|p| p.into_inner());
            let before = monitors.len();
            monitors.retain(|_, (owner, _)| *owner != key);
            if monitors.len() != before {
                self.monitor_released.notify_all();
            }
        }
        if let Some(state) = removed {
            let state = std::mem::take(&mut *state.lock().unwrap_or_else(|p| p.into_inner()));
            for local in state.locals.into_values() { self.release_target(local.target); }
            if let Some(pending) = state.pending { self.release_target(RefTarget::Throwable(pending.id)); }
        }
    }

    /// Test-only interleaving point: when armed on the calling thread, runs
    /// detach path another thread's exit would run (a key no thread owns).
    /// Reclamation is now driven by released pins, so an unrelated detach
    /// cannot scan or reclaim an entity being published.
    #[cfg(test)]
    pub(super) fn sweep_probe(&self) {
        if SWEEP_PROBE.with(|armed| armed.get()) {
            self.detach_key(u64::MAX);
        }
    }

    #[cfg(not(test))]
    #[inline(always)]
    pub(super) fn sweep_probe(&self) {}

    /// Attached-thread count (ticket 88 test/diagnostic surface).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn attached_thread_count(&self) -> usize {
        self.threads.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// The current thread's pending exception (test helper).
    #[cfg(test)]
    pub(super) fn current_pending(&self) -> Option<Arc<Pending>> {
        self.with_thread(|s| s.pending.clone())
    }
}
