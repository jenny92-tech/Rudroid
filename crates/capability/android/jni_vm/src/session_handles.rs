//! INPUT: shared session state and JNI handles
//! OUTPUT: validated operations on the shared VM
//! POS: strong and local JNI reference ownership
#![forbid(unsafe_code)]

use super::*;

// Retain small, warmed frame buckets. A large/deep burst releases its
// storage on pop without disturbing the outer frames.
const WARM_DEPTH: usize = 32;
const LARGE_BUCKET: usize = 4096;

impl ThreadState {
    pub(super) fn insert_local(&mut self, serial: u64, target: RefTarget) {
        let position = if self.frame == 0 {
            0
        } else {
            let entries = self.pushed_locals.entry(self.frame).or_default();
            entries.push(serial);
            entries.len() - 1
        };
        self.locals.insert(
            serial,
            Local {
                target,
                frame: self.frame,
                position,
            },
        );
    }

    fn remove_local(&mut self, serial: u64) -> Option<RefTarget> {
        let local = self.locals.remove(&serial)?;
        if local.frame != 0 {
            let entries = self
                .pushed_locals
                .get_mut(&local.frame)
                .expect("live local frame");
            entries.swap_remove(local.position);
            if let Some(moved) = entries.get(local.position) {
                self.locals.get_mut(moved).expect("live local").position = local.position;
            }
            if entries.is_empty() && (local.frame > WARM_DEPTH || entries.capacity() > LARGE_BUCKET) {
                self.pushed_locals.remove(&local.frame);
            }
        }
        Some(local.target)
    }

    fn release_frames(&mut self, from: usize, session: &Session) {
        if from == 0 {
            for (_, local) in self.locals.drain() {
                session.release_target(local.target);
            }
            for entries in self.pushed_locals.values_mut() { entries.clear(); }
        } else {
            for (_, serials) in self.pushed_locals.range_mut(from..) {
                for serial in serials.drain(..) {
                    let local = self.locals.remove(&serial).expect("indexed live local");
                    session.release_target(local.target);
                }
            }
        }
        self.pushed_locals.retain(|depth, entries| {
            *depth < from || (*depth <= WARM_DEPTH && entries.capacity() <= LARGE_BUCKET)
        });
    }
}

impl Session {
    // -- handle <-> entity resolution

    pub(super) fn resolve(&self, handle: *mut c_void) -> Option<RefTarget> {
        if handle.is_null() {
            return None;
        }
        let (tag, serial) = handle_parts(handle);
        match tag {
            TAG_GLOBAL => self.globals.lock().unwrap().get(&serial).copied(),
            TAG_WEAK => {
                let target = self.weaks.lock().unwrap().get(&serial).copied()?;
                self.target_alive(target).then_some(target)
            }
            TAG_LOCAL => self.with_thread(|s| s.locals.get(&serial).map(|local| local.target)),
            TAG_METHOD => {
                let methods = self.methods.lock().unwrap();
                methods
                    .by_id
                    .get(&serial)
                    .map(|m| RefTarget::Class(m.class))
            }
            _ => None,
        }
    }

    pub(super) fn target_alive(&self, target: RefTarget) -> bool {
        match target {
            RefTarget::Class(_) | RefTarget::ReflectedMethod(_) | RefTarget::ReflectedField(_) => {
                true
            }
            RefTarget::Str(id) => self.strings.lock().unwrap().contains_key(&id),
            RefTarget::Throwable(id) => self.throwables.lock().unwrap().contains_key(&id),
            RefTarget::Object(id) => self.objects.lock().unwrap().contains_key(&id),
            RefTarget::Array(id) => self.arrays.lock().unwrap().contains_key(&id),
        }
    }
    /// Resolve and retain in one critical section. A numeric handle snapshot
    /// alone cannot prevent the last global owner from being deleted.
    pub(super) fn root_reference(&self, handle: *mut c_void) -> Option<ReferenceRoot<'_>> {
        let (tag, id) = handle_parts(handle);
        let target = match tag {
            TAG_LOCAL => self.with_thread(|state| {
                let target = state.locals.get(&id)?.target;
                self.pin_target(target);
                Some(target)
            }),
            TAG_GLOBAL => {
                let globals = self.globals.lock().unwrap();
                let target = globals.get(&id).copied()?;
                // DeleteGlobalRef cannot remove the source until it is pinned.
                self.pin_target(target);
                Some(target)
            }
            TAG_WEAK => {
                let mut live = self.liveness.lock().unwrap();
                let target = self.weaks.lock().unwrap().get(&id).copied()?;
                if !self.target_alive(target) {
                    return None;
                }
                if let RefTarget::Str(id)
                | RefTarget::Throwable(id)
                | RefTarget::Object(id)
                | RefTarget::Array(id) = target
                {
                    let count = live.entry(id).or_default();
                    *count = count.checked_add(1).expect("JNI pin overflow");
                }
                Some(target)
            }
            TAG_METHOD => self.resolve(handle),
            _ => None,
        }?;
        Some(ReferenceRoot {
            session: self,
            target,
        })
    }

    pub(super) fn promote_reference(&self, handle: *mut c_void, global: bool) -> *mut c_void {
        if handle.is_null() {
            return std::ptr::null_mut();
        }
        let Some(root) = self.root_reference(handle) else {
            if handle_parts(handle).0 != TAG_WEAK {
                self.set_pending("java/lang/IllegalArgumentException", "invalid reference");
            }
            return std::ptr::null_mut();
        };
        self.new_ref(root.target, global)
    }

    /// Resolve a handle to the CLASS identity it references (null +
    /// pending on any mismatch — explicit, never a guessed default).
    pub(super) fn resolve_class(&self, handle: *mut c_void, entry: &str) -> Option<u64> {
        match self.resolve(handle) {
            Some(RefTarget::Class(id)) => Some(id),
            other => {
                self.log(&format!(
                    "{entry}: argument is not a live class reference (got {other:?})"
                ));
                self.set_pending(
                    "java/lang/NoClassDefFoundError",
                    "not a live class reference",
                );
                None
            }
        }
    }

    pub(super) fn new_ref(&self, target: RefTarget, global: bool) -> *mut c_void {
        if !global && Self::thread_key().is_none() {
            return std::ptr::null_mut();
        }
        let Some(serial) = self.next_serial() else {
            self.serial_exhausted(if global {
                "NewGlobalRef"
            } else {
                "NewLocalRef"
            });
            return std::ptr::null_mut();
        };
        self.pin_target(target);
        if global {
            self.globals.lock().unwrap().insert(serial, target);
        } else {
            // Ticket 88: locals are owned by the creating thread, in its
            // innermost local frame.
            self.with_thread(|s| s.insert_local(serial, target));
        }
        make_handle(if global { TAG_GLOBAL } else { TAG_LOCAL }, serial)
    }

    pub(super) fn delete_ref(&self, handle: *mut c_void, global: bool, entry: &str) {
        if handle.is_null() {
            return;
        }
        let (tag, serial) = handle_parts(handle);
        let expected = if global { TAG_GLOBAL } else { TAG_LOCAL };
        let removed = if tag == expected {
            if global {
                let mut globals = self.globals.lock().unwrap();
                let removed = globals.remove(&serial);
                trim_reference_table(&mut globals);
                removed
            } else {
                // Ticket 88: locals resolve against the calling thread.
                self.with_thread(|s| {
                    let removed = s.remove_local(serial);
                    trim_reference_table(&mut s.locals);
                    removed
                })
            }
        } else {
            None
        };
        if removed.is_none() {
            self.log(&format!(
                "{entry}: deleting an unknown or already-deleted reference"
            ));
            self.set_pending(
                "java/lang/NoSuchFieldError",
                "deleting an unknown or already-deleted reference",
            );
            return;
        }
        // Ticket 88 Phase B: deleting the last reference of a string or
        // throwable entity reclaims it.
        if let Some(target) = removed {
            self.release_target(target);
        }
    }

    // -- local frames (libjnivm PushLocalFrame/PopLocalFrame parity)

    pub(super) fn push_local_frame(&self, capacity: c_int) -> c_int {
        if capacity < 0 {
            self.log(&format!("PushLocalFrame: negative capacity {capacity}"));
            self.set_pending(
                "java/lang/OutOfMemoryError",
                "negative local frame capacity",
            );
            return abi::JNI_ERR;
        }
        let pushed =
            self.with_thread(|s| s.frame.checked_add(1).map(|next| s.frame = next).is_some());
        if pushed {
            abi::JNI_OK
        } else {
            self.set_pending("java/lang/OutOfMemoryError", "local frame depth exhausted");
            abi::JNI_ERR
        }
    }

    /// Free every local of the innermost frame and hand `result` back as a
    /// fresh local of the now-current frame. With no pushed frame the base
    /// frame is cleared (the reference recreates its top-level frame).
    pub(super) fn pop_local_frame(&self, result: *mut c_void) -> *mut c_void {
        let (depth, floor) = self.with_thread(|s| (s.frame, s.native_floor));
        if floor.is_some_and(|floor| depth <= floor) {
            self.set_pending(
                "java/lang/IllegalStateException",
                "cannot pop the automatic native frame",
            );
            return std::ptr::null_mut();
        }
        let output = self.pop_frames_from(depth, depth.saturating_sub(1), result, None);
        if depth == 0 {
            self.log("PopLocalFrame: no pushed frame — cleared the base local frame");
        }
        output
    }

    /// Shared release/promotion path: ordinary Pop removes its top frame;
    /// native return removes all nested native-owned frames and restores the
    /// caller's boundary. The result gains its new pin before any old release.
    pub(super) fn pop_frames_from(
        &self,
        from: usize,
        caller: usize,
        result: *mut c_void,
        restore_floor: Option<Option<usize>>,
    ) -> *mut c_void {
        let root = if result.is_null() {
            None
        } else {
            match self.root_reference(result) {
                Some(root) => Some(root),
                None => {
                    self.log("PopLocalFrame: result is not a live reference; returning null");
                    None
                }
            }
        };
        // Source promotion retains under the local/global/weak authority
        // lock, closing the concurrent last-global deletion window. This
        // temporary pin survives all frame releases and drops after the
        // destination local has its own pin (or the failed transfer finishes).
        let kept = root.as_ref().map(|root| root.target);
        let serial = match kept {
            Some(_) => match self.next_serial() {
                Some(serial) => Some(serial),
                None => {
                    self.serial_exhausted("PopLocalFrame");
                    None
                }
            },
            None => None,
        };
        // The result gains its outer-frame pin before any removed local
        // releases its pin, including when both reference the same entity.
        self.with_thread(|s| {
            if let (Some(target), Some(_)) = (kept, serial) {
                self.pin_target(target);
            }
            // Lock order is thread state -> liveness -> entity storage.
            // Reclaim never enters thread state or invokes callbacks, so
            // pins can be released directly without a temporary Vec.
            s.release_frames(from, self);
            s.frame = caller;
            if let Some(floor) = restore_floor {
                s.native_floor = floor;
            }
            if let (Some(target), Some(serial)) = (kept, serial) {
                s.insert_local(serial, target);
            }
            trim_reference_table(&mut s.locals);
        });
        self.sweep_probe();
        match serial {
            Some(serial) => make_handle(TAG_LOCAL, serial),
            None => std::ptr::null_mut(),
        }
    }

    pub(super) fn ensure_local_capacity(&self, capacity: c_int) -> c_int {
        if capacity < 0 {
            self.log(&format!(
                "EnsureLocalCapacity: negative capacity {capacity}"
            ));
            self.set_pending("java/lang/OutOfMemoryError", "negative local capacity");
            return abi::JNI_ERR;
        }
        // The local table grows on demand; nothing to reserve.
        abi::JNI_OK
    }
}
