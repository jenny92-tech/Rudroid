//! INPUT:  Rust values a C callback must find again (sessions, providers, per-registration state)
//! OUTPUT:  `HandleTable<T>`: opaque non-null `*mut c_void` handles that are serial numbers, resolved by lookup
//! POS:    replaces `Box::into_raw` + `&*(user as *const T)` for C `user`/session values; no pointer is ever dereferenced
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// Shared by all tables linked into this library. Different cdylibs still
// have different namespaces: their handles must only return to the library
// that issued them (the C callback contract).
static NEXT: AtomicUsize = AtomicUsize::new(1);

/// Process-wide table of `Arc<T>` keyed by serial. The handle handed to C is
/// the serial cast to a pointer: the host passes it back verbatim, and an
/// unknown, removed or other-table handle resolves to `None` within one
/// linked copy of common. A resolved `Arc` keeps its value alive across removal.
pub struct HandleTable<T> {
    entries: Mutex<BTreeMap<usize, Arc<T>>>,
}

impl<T> HandleTable<T> {
    pub const fn new() -> Self {
        Self { entries: Mutex::new(BTreeMap::new()) }
    }

    /// Store `value`; the returned handle is non-null and never reused.
    pub fn insert(&self, value: T) -> *mut c_void {
        self.insert_arc(Arc::new(value))
    }

    pub fn insert_arc(&self, value: Arc<T>) -> *mut c_void {
        let serial = next_serial(&NEXT);
        self.lock().insert(serial, value);
        serial as *mut c_void
    }

    pub fn get(&self, handle: *mut c_void) -> Option<Arc<T>> {
        self.lock().get(&(handle as usize)).cloned()
    }

    pub fn remove(&self, handle: *mut c_void) -> Option<Arc<T>> {
        self.lock().remove(&(handle as usize))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<usize, Arc<T>>> {
        // Callbacks never panic while holding the lock; recover the map anyway.
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn next_serial(counter: &AtomicUsize) -> usize {
    // Exhaustion stays exhausted even if a Rust caller catches the panic;
    // fetch_add would wrap and eventually overwrite a still-live owner.
    counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |serial| serial.checked_add(1))
        .expect("handle serials exhausted")
}

impl<T> Default for HandleTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../tests/unit/handles.rs"]
mod tests;
