//! INPUT:  shared JNI session, Unity player object and InputEvent locals
//! OUTPUT:  typed Unity input dispatch and nativeDone-aware player teardown
//! POS:    keeps Unity player JNI helpers out of rudroid-common
#![forbid(unsafe_code)]

use std::ffi::{c_void, CStr};

use rudroid_common::jni_env::{Arg, JObject, JniEnv, Method};

/// Drops a local JNI reference even when the pump returns early or panics.
struct EventLocal {
    env: JniEnv,
    object: JObject,
}

impl Drop for EventLocal {
    fn drop(&mut self) {
        if !self.object.is_null() {
            self.env.delete_local_ref(self.object);
        }
    }
}

/// The two registered Unity input ABIs observed in shipped players. The VM
/// resolves only registered/catalog methods; a miss leaves NoSuchMethodError
/// pending, so probing is confined to these exact descriptors.
#[derive(Clone, Copy)]
pub enum PlayerInputDispatch {
    Legacy(Method),
    Unity2022(Method),
}

impl PlayerInputDispatch {
    /// Resolve a registered player input method without hiding an exception
    /// that predates the probe. Only an actual `NoSuchMethodError` from the
    /// Unity 2022 probe permits the legacy lookup; every other exception is
    /// restored unchanged.
    pub fn resolve(env: JniEnv, class: JObject) -> Result<Self, ()> {
        const NAME: &CStr = c"nativeInjectEvent";
        const LEGACY: &CStr = c"(Landroid/view/InputEvent;)Z";
        const UNITY_2022: &CStr = c"(Landroid/view/InputEvent;I)Z";
        if class.is_null() || env.exception_check() {
            return Err(());
        }
        let missing_method = env.find_class(c"java/lang/NoSuchMethodError");
        if missing_method.is_null() || env.exception_check() {
            if !missing_method.is_null() {
                env.delete_local_ref(missing_method);
            }
            return Err(());
        }
        if let Some(method) = env.method(class, NAME, UNITY_2022) {
            env.delete_local_ref(missing_method);
            return if env.exception_check() {
                Err(())
            } else {
                Ok(Self::Unity2022(method))
            };
        }
        let first = env.exception_occurred();
        if first.is_null() {
            env.delete_local_ref(missing_method);
            return Err(());
        }
        env.exception_clear();
        let first_is_missing = !env.exception_check()
            && env.is_instance_of(first, missing_method)
            && !env.exception_check();
        if !first_is_missing {
            restore_player_input_exception(env, first);
            env.delete_local_ref(first);
            env.delete_local_ref(missing_method);
            return Err(());
        }
        if let Some(method) = env.method(class, NAME, LEGACY) {
            env.delete_local_ref(first);
            env.delete_local_ref(missing_method);
            return if env.exception_check() {
                Err(())
            } else {
                Ok(Self::Legacy(method))
            };
        }
        let second = env.exception_occurred();
        if second.is_null() {
            if !env.exception_check() {
                restore_player_input_exception(env, first);
            }
            env.delete_local_ref(first);
            env.delete_local_ref(missing_method);
            return Err(());
        }
        env.exception_clear();
        let second_is_missing = !env.exception_check()
            && env.is_instance_of(second, missing_method)
            && !env.exception_check();
        let (restored, discarded) = if second_is_missing {
            (first, second)
        } else {
            (second, first)
        };
        restore_player_input_exception(env, restored);
        if !discarded.is_null() {
            env.delete_local_ref(discarded);
        }
        if !restored.is_null() {
            env.delete_local_ref(restored);
        }
        env.delete_local_ref(missing_method);
        Err(())
    }

    /// Dispatch through the resolved shape. Unity 2022's mode is the same zero
    /// used by `injectEvent(InputEvent)`. An exception stays pending on failure.
    pub fn call(self, env: JniEnv, player: JObject, event: JObject) -> Result<bool, ()> {
        if env.exception_check() {
            return Err(());
        }
        let accepted = match self {
            Self::Legacy(method) => env.call_boolean(player, method, &[Arg::object(event)]),
            Self::Unity2022(method) => {
                env.call_boolean(player, method, &[Arg::object(event), Arg::int(0)])
            }
        };
        if env.exception_check() {
            Err(())
        } else {
            Ok(accepted)
        }
    }
}

/// Restore an exception captured before a controlled JNI probe. Any exception
/// raised by classification itself is secondary and must not replace it.
fn restore_player_input_exception(env: JniEnv, exception: JObject) {
    if exception.is_null() {
        return;
    }
    if env.exception_check() {
        env.exception_clear();
    }
    env.throw(exception);
}

/// Resolve on the current player object and dispatch one event. This is the
/// uncached path used by the Java-to-native soft-input passthrough; the main
/// pump caches the same typed choice.
pub fn dispatch_player_input_event(
    env: JniEnv,
    player: JObject,
    event: JObject,
) -> Result<bool, ()> {
    if env.exception_check() {
        return Err(());
    }
    let class = env.get_object_class(player);
    if class.is_null() {
        return Err(());
    }
    let result = PlayerInputDispatch::resolve(env, class)
        .and_then(|dispatch| dispatch.call(env, player, event));
    env.delete_local_ref(class);
    result
}

/// Cached input dispatch on the player/SDL creator thread. The VM owns events;
/// this scoped adapter retains only one local class handle and typed method ID.
pub struct PlayerInput {
    env: JniEnv,
    source: *mut c_void,
    poll: Method,
    quit: Method,
    inject: Option<Option<PlayerInputDispatch>>,
    trace: bool,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl PlayerInput {
    /// `env` is the current thread's env; the adapter stays on this thread.
    pub fn new(env: JniEnv, trace: bool) -> Option<Self> {
        let source = env.find_class(c"org/rudroid/runtime/InputSource");
        let poll = env.static_method(source, c"poll", c"()Ljava/lang/Object;");
        let quit = env.static_method(source, c"quitRequested", c"()Z");
        let (false, Some(poll), Some(quit)) = (source.is_null(), poll, quit) else {
            env.delete_local_ref(source);
            return None;
        };
        Some(Self {
            env,
            source,
            poll,
            quit,
            inject: None,
            trace,
            _thread: std::marker::PhantomData,
        })
    }

    /// Pump bounded input before rendering. `Ok(true)` requests a clean exit.
    pub fn pump(&mut self, player: *mut c_void, class: *mut c_void) -> Result<bool, ()> {
        self.pump_with(player, class, |_| Ok(false))
    }

    /// Allow a player-owned text-input session to consume an event before native
    /// dispatch. The event stays local to this call and drops on error or unwind.
    pub fn pump_with(
        &mut self,
        player: *mut c_void,
        class: *mut c_void,
        mut consume: impl FnMut(*mut c_void) -> Result<bool, ()>,
    ) -> Result<bool, ()> {
        let env = self.env;
        if env.exception_check() {
            return Err(());
        }
        for _ in 0..64 {
            let event = env.call_static_object(self.source, self.poll, &[]);
            let _event = EventLocal { env, object: event };
            if env.exception_check() {
                return Err(());
            }
            if event.is_null() {
                break;
            }
            if consume(event)? {
                continue;
            }
            let inject = *self
                .inject
                .get_or_insert_with(|| PlayerInputDispatch::resolve(env, class).ok());
            let Some(inject) = inject else {
                return Err(());
            };
            let accepted = u8::from(inject.call(env, player, event)?);
            if self.trace {
                rudroid_common::diagnostic!("[UnityInput] nativeInjectEvent accepted={accepted}");
            }
        }
        let quit = env.call_static_boolean(self.source, self.quit, &[]);
        if env.exception_check() {
            Err(())
        } else {
            Ok(quit)
        }
    }
}

impl Drop for PlayerInput {
    fn drop(&mut self) {
        self.env.delete_local_ref(self.source);
    }
}

/// Quiesce independent services, let nativeDone own terminal producer stop,
/// then join final services/core workers. Preserve the first exception.
/// Runs on the player thread. `initialized` means initJni was attempted;
/// otherwise player/class may be null and only services are closed.
pub fn finish_player(
    env: JniEnv,
    player: *mut c_void,
    class: *mut c_void,
    initialized: bool,
) -> bool {
    let mut first = env.exception_occurred();
    env.exception_clear();
    let capture = |first: &mut *mut c_void| {
        let error = env.exception_occurred();
        env.exception_clear();
        if first.is_null() {
            *first = error;
        } else {
            env.delete_local_ref(error);
        }
    };
    if initialized {
        rudroid_common::diagnostic!("[UnityLifecycle] native owner controls terminal stop order");
    }
    let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
    capture(&mut first);
    rudroid_common::diagnostic!("[UnityLifecycle] quiescing service producers");
    if let Some(stop) = env.static_method(lifecycle, c"shutdown", c"()V") {
        env.call_static_void(lifecycle, stop, &[]);
    }
    capture(&mut first);
    if initialized {
        rudroid_common::diagnostic!("[UnityLifecycle] destroying native player");
        if let Some(done) = env.method(class, c"nativeDone", c"()Z") {
            env.call_boolean(player, done, &[]);
        }
        capture(&mut first);
    }
    rudroid_common::diagnostic!("[UnityLifecycle] joining core workers");
    if let Some(finish) = env.static_method(lifecycle, c"finish", c"()V") {
        env.call_static_void(lifecycle, finish, &[]);
    }
    capture(&mut first);
    env.delete_local_ref(lifecycle);
    rudroid_common::diagnostic!("[UnityLifecycle] cleanup completed");
    if first.is_null() {
        true
    } else {
        env.throw(first);
        env.delete_local_ref(first);
        false
    }
}
