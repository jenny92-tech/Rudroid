//! INPUT: shared per-thread local frames and typed native return values
//! OUTPUT: automatic native scopes, rooted return promotion and unwind cleanup
//! POS: one reusable owner for every descriptor-dispatched native call
#![forbid(unsafe_code)]
use super::Session;
use std::ffi::c_void;

struct NativeFrame<'a> {
    session: &'a Session,
    caller: usize,
    previous_floor: Option<usize>,
    armed: bool,
}
impl<'a> NativeFrame<'a> {
    fn enter(session: &'a Session) -> Option<Self> {
        let state = session.with_thread(|thread| {
            let next = thread.frame.checked_add(1)?;
            // JNI guarantees at least sixteen local slots at native entry.
            // Reserve before publishing the boundary; warm calls reuse it.
            if thread.locals.try_reserve(16).is_err() {
                return None;
            }
            let saved = (thread.frame, thread.native_floor);
            thread.frame = next;
            thread.native_floor = Some(next);
            Some(saved)
        });
        match state {
            Some((caller, previous_floor)) => Some(Self {
                session,
                caller,
                previous_floor,
                armed: true,
            }),
            None => {
                session.set_pending(
                    "java/lang/OutOfMemoryError",
                    "native frame capacity/depth exhausted",
                );
                None
            }
        }
    }
    fn finish(mut self, result: *mut c_void) -> *mut c_void {
        let result = self.session.pop_frames_from(
            self.caller + 1,
            self.caller,
            result,
            Some(self.previous_floor),
        );
        self.armed = false;
        result
    }
}
impl Drop for NativeFrame<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.session.pop_frames_from(
                self.caller + 1,
                self.caller,
                std::ptr::null_mut(),
                Some(self.previous_floor),
            );
        }
    }
}

pub(super) trait NativeResult: Default {
    fn object(&self) -> *mut c_void {
        std::ptr::null_mut()
    }
    fn promoted(self, _object: *mut c_void) -> Self {
        self
    }
}
macro_rules! scalar_result {($($ty:ty),*) => {$(impl NativeResult for $ty {})*};}
scalar_result!((), u8, i8, u16, i16, i32, i64, f32, f64);
impl NativeResult for *mut c_void {
    fn object(&self) -> *mut c_void {
        *self
    }
    fn promoted(self, object: *mut c_void) -> Self {
        object
    }
}
pub(super) trait NativeArgument: Copy {
    fn local(self, session: &Session) -> Result<Self, ()>;
}
macro_rules! scalar_argument {($($ty:ty),*) => {$(
    impl NativeArgument for $ty {
        fn local(self, _session: &Session) -> Result<Self, ()> {Ok(self)}
    }
)*};}
scalar_argument!(u8, i8, u16, i16, i32, i64, f32, f64);
impl NativeArgument for *mut c_void {
    fn local(self, session: &Session) -> Result<Self, ()> {
        if self.is_null() {
            return Ok(self);
        }
        let local = session.promote_reference(self, false);
        if local.is_null() {
            // NewLocalRef of a cleared weak is legitimately null. Invalid
            // strong handles/failed promotion already set a pending error;
            // preserve it and never call the body after that failure.
            if session.with_thread(|thread| thread.pending.is_some()) {
                Err(())
            } else {
                Ok(std::ptr::null_mut())
            }
        } else {
            Ok(local)
        }
    }
}
impl Session {
    pub(super) fn native_argument<T: NativeArgument>(&self, value: T) -> Result<T, ()> {
        value.local(self)
    }
    pub(super) fn with_native_frame<R: NativeResult>(
        &self,
        call: impl FnOnce(&Self) -> Result<R, ()>,
    ) -> R {
        let Some(frame) = NativeFrame::enter(self) else {
            return R::default();
        };
        let result = match call(self) {
            Ok(result) => result,
            Err(()) => return R::default(),
        };
        let promoted = frame.finish(result.object());
        result.promoted(promoted)
    }
}
