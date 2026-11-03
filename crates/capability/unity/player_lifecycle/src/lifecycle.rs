//! INPUT:  a validated `Spec`, the loader Host API table, shared JNI session and guest module events
//! OUTPUT:  the `prepare`/`bootstrap`/`run` profile callbacks, player callbacks and the spy evidence echo
//! POS:    the one player lifecycle body every Unity engine profile registers; no per-version branches
//!
//! Player/context/Surface objects belong to the shared VM. Optional fixture
//! observation exports do not replace the real registered JNI lifecycle.

use std::ffi::{c_int, c_void, CStr, CString};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rudroid_common::handles::HandleTable;
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native, JNI_VERSION_1_6};
use rudroid_common::registry::{RudroidModuleEvent, HostStr};
use rudroid_common::runtime_policy::{remaining_frame_time, resolve_target_fps, DEFAULT_TARGET_FPS};
use rudroid_common::ModuleKind;

use crate::{clamp_render_sleep, log, spy, CallbackBody, Profile, Spec, COMMON_PLAYER_CALLBACKS};

// Drain framework-owned Java jobs on the same thread as the native player loop.
extern "C" fn execute_main_thread_jobs(env: JniEnv, _player: JObject) {
    let class = env.find_class(c"org/rudroid/runtime/UiThread");
    if let Some(method) = env.static_method(class, c"drain", c"()V") {
        env.call_static_void(class, method, &[]);
    }
    env.delete_local_ref(class);
}
extern "C" fn false_boolean(_env: JniEnv, _player: JObject) -> u8 { 0 }
extern "C" fn unavailable_library(_env: JniEnv, _player: JObject, _name: JObject) -> u8 { 0 }
extern "C" fn no_activity_indicator(_env: JniEnv, _player: JObject) {}
extern "C" fn start_activity_indicator(_env: JniEnv, _player: JObject, _style: i32) {}

impl CallbackBody {
    /// The checked native entry binding `name` to the body matching
    /// [`CallbackBody::signature`].
    pub(crate) fn native(self, name: &'static CStr) -> Native {
        let signature = self.signature();
        match self {
            CallbackBody::DrainMainThreadJobs => Native::new0(name, signature, execute_main_thread_jobs),
            CallbackBody::False => Native::new0(name, signature, false_boolean),
            CallbackBody::LibraryUnavailable => Native::new1(name, signature, unavailable_library),
            CallbackBody::NoActivityIndicator => Native::new0(name, signature, no_activity_indicator),
            CallbackBody::IgnoredActivityIndicator => Native::new1(name, signature, start_activity_indicator),
        }
    }
}

/// The profile session handed prepare → bootstrap → run. Stored in
/// [`SESSIONS`] at prepare and never removed (process-pinned like every live
/// guest mapping, ticket 46): guest code may already hold its module
/// subscription. Its handle is both the profile session and the module `user`.
struct GuestSession {
    spec: &'static Spec,
    api: &'static HostApiV0,
    /// Global handles owned by the process-pinned profile session.
    player_class_handle: usize,
    player: usize,
    context: usize,
    surface: usize,
    /// Load base of the guest main module, captured by the module
    /// subscription (events fire between prepare and bootstrap).
    main_module: Mutex<Option<usize>>,
    /// `unity.context_type` override (reference parity: read in prepare).
    init_jni_int_arg: i32,
    /// `unity.render_sleep_us` clamped (reference parity: read in prepare).
    render_sleep_us: u32,
    target_fps: u32,
    /// `unity.signal_vsync_before_render` (reference parity, default on).
    signal_vsync_before_render: bool,
}

static SESSIONS: HandleTable<GuestSession> = HandleTable::new();

impl GuestSession {
    fn log(&self, message: &str) {
        log(self.api, self.spec.log_tag, message);
    }

    /// The calling thread's env on the shared VM.
    fn env(&self) -> Option<JniEnv> {
        self.api.java_vm()?.get_env(JNI_VERSION_1_6)
    }
}

/// Module subscription: capture the guest main module's load base so
/// bootstrap/run can dispatch into its exec-mapped native bodies. Runs
/// during guest bring-up, between prepare and bootstrap (reference order).
/// `user` is the session handle prepare installed.
extern "C" fn capture_module_event(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let Some(event) = event else {
        return;
    };
    if event.kind != ModuleKind::Main as u32 {
        return;
    }
    let Some(session) = SESSIONS.get(user) else {
        return;
    };
    *session.main_module.lock().unwrap() = Some(event.base);
}

/// The reference `prepare` for profile `P`: engine-side setup, null on any
/// failure (the loader turns a null into a hard, named error).
pub(crate) extern "C" fn prepare<P: Profile>(api: Option<&'static HostApiV0>) -> *mut c_void {
    prepare_spec(P::SPEC, api)
}

fn prepare_spec(spec: &'static Spec, api: Option<&'static HostApiV0>) -> *mut c_void {
    let Some(api) = api else {
        return std::ptr::null_mut();
    };
    let log_on = |message: &str| log(api, spec.log_tag, message);
    if api.abi_version != HOST_API_VERSION {
        log_on("host api version mismatch; profile prepare refused");
        return std::ptr::null_mut();
    }
    // Reference parity: prepare requires the JNI runtime.
    let Some(vm) = api.java_vm() else {
        log_on("no JNI session registered; the engine profile cannot prepare");
        return std::ptr::null_mut();
    };

    // GetEnv on the shared VM, then FindClass of the player class — both
    // real session calls; any failure is a hard null with a named log.
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        log_on("GetEnv on the shared JavaVM failed; cannot prepare");
        return std::ptr::null_mut();
    };
    let class_name = CString::new(spec.player_class).expect("spec class has no NUL");
    let class = env.find_class(&class_name);
    if class.is_null() {
        log_on(&format!("profile {} class not registered: {}", spec.id, spec.player_class));
        return std::ptr::null_mut();
    }

    if let Some(register_services) = spec.register_java_services {
        if register_services(env,api) != 0 {
            log_on("Unity Java service registration failed");
            return std::ptr::null_mut();
        }
    }

    let callbacks: Vec<Native> = COMMON_PLAYER_CALLBACKS
        .iter()
        .chain(spec.extra_player_callbacks)
        .map(|callback| callback.body.native(callback.name))
        .collect();
    if env.register_class(&class_name, &callbacks) != 0 {
        log_on("player Java callback registration failed");
        return std::ptr::null_mut();
    }

    // Only the shared VM creates object identities. The profile retains global
    // handles because native code and attached Unity threads may retain them.
    let types = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let set_parent = env.static_method(types, c"setSuperclass",
        c"(Ljava/lang/Class;Ljava/lang/Class;)V");
    if spec.player_class != "com/unity3d/player/UnityPlayer" {
        let parent = env.find_class(c"com/unity3d/player/UnityPlayer");
        if let Some(set_parent) = set_parent {
            env.call_static_void(types, set_parent, &[Arg::object(class), Arg::object(parent)]);
        }
        env.delete_local_ref(parent);
    }
    env.delete_local_ref(types);
    let player_local = env.alloc_object(class);
    let player = env.new_global_ref(player_local);
    env.delete_local_ref(player_local);
    let make_object = |name: &CStr| {
        let class = env.find_class(name);
        let local = env.alloc_object(class);
        let object = env.new_global_ref(local);
        env.delete_local_ref(local);
        env.delete_local_ref(class);
        object
    };
    // A real Unity Activity owns this player; managed JNI reflection and
    // native init must see the same standard class and object graph.
    let activity_name = c"com/unity3d/player/UnityPlayerActivity";
    if rudroid_common::jni_abi::declare_superclass(env, activity_name, c"android/app/Activity") != 0
        || rudroid_common::jni_abi::declare_field(env, activity_name, c"mUnityPlayer", c"Lcom/unity3d/player/UnityPlayer;", false) != 0 {
        return std::ptr::null_mut();
    }
    let context = make_object(activity_name);
    let activity_class = env.get_object_class(context);
    let player_field = env.get_field_id(activity_class,c"mUnityPlayer",c"Lcom/unity3d/player/UnityPlayer;");
    env.set_object_field(context,player_field,player);
    env.delete_local_ref(activity_class);
    if spec.publishes_current_activity {
        // Native bootstrap and managed AndroidJavaObject calls share one Activity.
        let unity = env.find_class(c"com/unity3d/player/UnityPlayer");
        if rudroid_common::jni_abi::declare_field(env, c"com/unity3d/player/UnityPlayer", c"currentActivity", c"Landroid/app/Activity;", true) != 0 {
            return std::ptr::null_mut();
        }
        let field = env.get_static_field_id(unity, c"currentActivity", c"Landroid/app/Activity;");
        env.set_static_object_field(unity, field, context);
        env.delete_local_ref(unity);
    }
    let surface = make_object(c"android/view/Surface");
    let player_class = env.new_global_ref(class);
    env.delete_local_ref(class);
    if player.is_null() || context.is_null() || surface.is_null() || env.exception_check() {
        log_on("shared JNI object preparation failed");
        return std::ptr::null_mut();
    }

    // Reference-parity config reads (the same keys the C++ runner reads).
    let init_jni_int_arg = api.config_i64(c"unity.context_type", i64::from(spec.init_jni_int_arg)) as i32;
    let render_sleep_us = clamp_render_sleep(
        api.config_i64(c"unity.render_sleep_us", i64::from(spec.render_sleep_us)),
        spec.render_sleep_us,
    );
    let target_fps = resolve_target_fps(api.config_i64(c"unity.target_fps", i64::from(DEFAULT_TARGET_FPS)));
    let signal_vsync_before_render = api.config_bool(c"unity.signal_vsync_before_render", true);

    // Process-pinned session (ticket 46): never removed from the table, the
    // loader passes its handle back to bootstrap/run.
    let session = SESSIONS.insert(GuestSession {
        spec,
        api,
        player_class_handle: player_class as usize,
        player: player as usize,
        context: context as usize,
        surface: surface as usize,
        main_module: Mutex::new(None),
        init_jni_int_arg,
        render_sleep_us,
        target_fps,
        signal_vsync_before_render,
    });
    for kind in [ModuleKind::Main, ModuleKind::Il2cpp] {
        if !api.on_module_loaded(kind, capture_module_event, session) {
            log_on(&format!(
                "module subscription for {kind:?} was rejected; \
                 guest dispatch cannot be wired"
            ));
            return std::ptr::null_mut();
        }
    }
    log_on(&format!(
        "active engine profile={} player={} (shared JNI object/native dispatch)",
        spec.id, spec.player_class
    ));
    session
}

/// The reference `bootstrap`: `NativeLoader.load(native_library_dir)` —
/// class lookup through the shared session, native body through the
/// exec-mapped guest module. Non-zero on any failure; NEVER reports success
/// before the guest code actually ran. `session` is the handle from prepare;
/// `native_library_dir` is lent by the host for the call (null when absent).
pub(crate) extern "C" fn bootstrap(session: *mut c_void, native_library_dir: HostStr<'_>) -> c_int {
    let Some(session) = SESSIONS.get(session) else {
        return -1;
    };
    let dir = native_library_dir
        .to_cstr()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();
    if dir.is_empty() {
        session.log("bootstrap called without a native library dir");
        return -1;
    }
    let Some(main_base) = *session.main_module.lock().unwrap() else {
        session.log(
            "bootstrap ran before the guest main module was mapped; guest \
             native bodies are unavailable",
        );
        return -1;
    };
    let Some(env) = session.env() else {
        session.log("GetEnv on the shared JavaVM failed; cannot bootstrap");
        return -1;
    };
    let native_loader = env.find_class(c"com/unity3d/player/NativeLoader");
    if native_loader.is_null() {
        session.log("NativeLoader class was not registered by libmain");
        return -1;
    }

    // Resolve the method registered by real libmain in the same shared JNI
    // method authority. No fixture symbol or separate native-address registry.
    let Some(load_method) = env
        .static_method(native_loader, c"load", c"(Ljava/lang/String;)Z")
        .filter(|_| !env.exception_check())
    else {
        env.delete_local_ref(native_loader);
        session.log("NativeLoader.load(String)Z was not bound by libmain");
        return -1;
    };
    session.log(&format!("calling NativeLoader.load({dir})"));
    let c_dir = CString::new(dir.clone()).expect("dir came from a C string");
    let jstring = env.new_string_utf(&c_dir);
    if jstring.is_null() {
        env.delete_local_ref(native_loader);
        session.log("NewStringUTF for the native library dir failed");
        return -1;
    }
    let returned = env.call_static_boolean(native_loader, load_method, &[Arg::object(jstring)]);
    env.delete_local_ref(jstring);
    env.delete_local_ref(native_loader);
    if env.exception_check() {
        session.log("a pending exception interrupted NativeLoader.load");
        echo_spy_events(&session, main_base);
        return -1;
    }
    if !returned {
        session.log("NativeLoader.load returned false");
        // Evidence echo on the failure path too: the guest RAN and answered
        // false — the recorded buffer proves the dispatch happened.
        echo_spy_events(&session, main_base);
        return -1;
    }
    session.log(&format!("NativeLoader.load({dir}) returned true"));
    0
}

/// The reference `run`: initJni dispatch, the lifecycle sequence, then the
/// render loop with vsync signalling and elapsed-aware 90 FPS pacing. Returns
/// 0 after complete teardown; the distinct process-exit result follows only
/// an engine stop with the opted-in policy and a checked preference save. Bootstrap
/// refusal, failure to start rendering and pending exceptions remain failures.
/// `session` is the handle from prepare.
pub(crate) extern "C" fn run(session: *mut c_void) -> c_int {
    let Some(session) = SESSIONS.get(session) else {
        return -1;
    };
    let Some(main_base) = *session.main_module.lock().unwrap() else {
        session.log("run called before the guest main module was mapped");
        return -1;
    };
    let Some(env) = session.env() else {
        session.log("GetEnv on the shared JavaVM failed; cannot run");
        return -1;
    };

    let mut initialized=false;
    let mut engine_stopped=false;
    let result=run_player(&session,main_base,env,&mut initialized,&mut engine_stopped);
    super::soft_input::reset(env,session.player as JObject);
    // Release optional managed operation roots while IL2CPP is still alive.
    session.api.fire_event(ModuleKind::UnityStopping);
    // Only the engine's terminal render result follows the game's own quit/
    // save flow. Startup failures, frame limits and input shortcuts retain
    // complete teardown. Never exit from a managed callback or free state
    // that still has live native/service workers in this process-exit path.
    if result == 0 && engine_stopped && !env.exception_check()
        && session.api.config_bool(c"unity.fast_exit", false)
    {
        let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
        let saved = env.static_method(lifecycle, c"flushPreferences", c"()Z")
            .is_some_and(|flush| env.call_static_boolean(lifecycle, flush, &[]));
        env.delete_local_ref(lifecycle);
        if saved && !env.exception_check() {
            session.log("engine stop saved preferences; requesting immediate process exit");
            echo_spy_events(&session,main_base);
            return rudroid_common::registry::RUDROID_UNITY_RUN_PROCESS_EXIT;
        }
        session.log("fast exit refused: preference flush failed; retaining full teardown");
    }
    let clean = rudroid_common::jni_abi::finish_player(
        env, session.player as JObject, session.player_class_handle as JObject, initialized);
    echo_spy_events(&session,main_base);
    if !clean{session.log("player cleanup reported an exception");return -1;}
    result
}

fn run_player(session:&GuestSession,main_base:usize,env:JniEnv,initialized:&mut bool,engine_stopped:&mut bool)->c_int {
    let (player, player_class) = (session.player as JObject, session.player_class_handle as JObject);
    let resolve = |name: &CStr, signature: &CStr| env.method(player_class, name, signature);
    let init_sig = CString::new(session.spec.init_jni_signature).expect("profile signature");
    let Some(init) = resolve(c"initJni", &init_sig).filter(|_| !env.exception_check()) else {
        session.log("required registered initJni method is unavailable");
        return -1;
    };
    let init_args = [Arg::object(session.context as JObject),
        Arg::int(session.init_jni_int_arg)];
    session.log(&format!("calling initJni ({})", session.spec.init_jni_signature));
    *initialized=true;
    // Both initJni shapes take a prefix of (Context, int).
    env.call_void(player, init, &init_args[..init.arity()]);
    if env.exception_check() {
        session.log("a pending exception interrupted initJni");
        return -1;
    }
    let recreate = resolve(c"nativeRecreateGfxState", c"(ILandroid/view/Surface;)V");
    let changed = resolve(c"nativeSendSurfaceChangedEvent", c"()V");
    let focus = resolve(c"nativeFocusChanged", c"(Z)V");
    let resume = resolve(c"nativeResume", c"()V");
    let render_method = resolve(c"nativeRender", c"()Z");
    let (recreate, changed, focus, resume, render_method) = match (recreate, changed, focus, resume, render_method) {
        (Some(recreate), Some(changed), Some(focus), Some(resume), Some(render_method)) if !env.exception_check() =>
            (recreate, changed, focus, resume, render_method),
        _ => {
            session.log("required lifecycle JNI method unavailable");
            return -1;
        }
    };
    let restart = resolve(c"nativeRestartActivityIndicator", c"()V");
    if restart.is_none() {
        // This optional method is absent on some Unity versions.
        env.exception_clear();
    }
    env.call_void(player, recreate, &[Arg::int(0), Arg::object(session.surface as JObject)]);
    session.log("calling nativeRecreateGfxState");
    if let Some(restart) = restart {
        env.call_void(player, restart, &[]);
        session.log("calling nativeRestartActivityIndicator");
    }
    env.call_void(player, changed, &[]);
    session.log("calling nativeSendSurfaceChangedEvent");
    env.call_void(player, resume, &[]);
    session.log("calling nativeResume");
    env.call_void(player, focus, &[Arg::bool(true)]);
    session.log("calling nativeFocusChanged(true)");
    if env.exception_check() {
        session.log("a pending exception interrupted the lifecycle sequence");
        return -1;
    }
    // Optional fixture observation; a real game never needs spy exports.
    let signal: Option<extern "C" fn()> = if session.signal_vsync_before_render {
        // SAFETY: the spy fixture exports `void rudroid_spy_signal_vsync(void)`.
        session.api.guest_symbol(main_base, spy::SIGNAL_VSYNC).map(|a| unsafe { std::mem::transmute::<usize, extern "C" fn()>(a) })
    } else { None };
    let render = |env: JniEnv, object| env.call_boolean(object, render_method, &[]);
    let Some(mut input) = rudroid_common::jni_abi::PlayerInput::new(env, session.api.config_bool(c"unity.log_input_events", false))
    else {session.log("input source unavailable");return -1;};
    let mut pump=||input.pump_with(player,player_class,|event|super::soft_input::consume(env,player,event));
    {
        let log_progress = session.api.config_bool(c"unity.log_frame_progress", false);
        let frame_limit = session.api.config_i64(c"unity.smoke_frame_limit", 0);
        let retry_cap = session.api.config_i64(c"unity.render_false_retry_cap", 20_000);
        let retry_sleep_us = session.api.config_i64(c"unity.render_false_retry_sleep_us", 100_000);
        session.log(&format!(
            "render loop entering: frames={} render_sleep_us={} target_fps={} retry_cap={retry_cap} \
             retry_sleep_us={retry_sleep_us}",
            if frame_limit == 0 {
                "unlimited".to_owned()
            } else {
                frame_limit.to_string()
            },
            session.render_sleep_us,
            session.target_fps
        ));

        let signal_vsync_fn = || {
            if let Some(signal) = signal {
                signal();
            }
        };
        // First render with the transient-false retry (reference: pump +
        // 100 ms sleep, up to 20000 attempts — config-tunable for the
        // fixture).
        let first_frame_started = Instant::now();
        match pump() {Ok(true)=>return 0,Ok(false)=>{},Err(())=>{session.log("input dispatch failed");return -1;}}
        signal_vsync_fn();
        let mut rendered = render(env, player);
        if env.exception_check() {
            session.log("a pending exception interrupted the first nativeRender");
            return -1;
        }
        let mut false_count: i64 = 0;
        while !rendered && false_count < retry_cap {
            match pump() {Ok(true)=>return 0,Ok(false)=>{},Err(())=>{session.log("input dispatch failed");return -1;}}
            signal_vsync_fn();
            std::thread::sleep(Duration::from_micros(retry_sleep_us.max(0) as u64));
            rendered = render(env, player);
            if env.exception_check() { session.log("a pending exception interrupted nativeRender retry");return -1; }
            false_count += 1;
        }
        if !rendered {
            session.log("first nativeRender still false");
            return -1;
        }
        session.log("nativeRender returned true; entering render loop");
        session.api.fire_event(ModuleKind::UnityFrame);
        let mut maintenance_at = Instant::now();
        let mut max_work_us = first_frame_started.elapsed().as_micros();
        let mut slow_frames = u64::from(max_work_us >= 50_000);
        let mut frame_work = log_progress.then(|| Box::new(rudroid_common::frame_timing::FrameWorkTiming::default()));
        if let Some(stats) = frame_work.as_mut() { stats.record(0, max_work_us); }

        let delay = remaining_frame_time(session.target_fps, first_frame_started.elapsed())
            .max(Duration::from_micros(u64::from(session.render_sleep_us)));
        if !delay.is_zero() { std::thread::sleep(delay); }
        let mut frames: i64 = 1;
        loop {
            let frame_started = Instant::now();
            match pump() {Ok(true)=>break,Ok(false)=>{},Err(())=>{session.log("input dispatch failed");return -1;}}
            signal_vsync_fn();
            let result = render(env, player);
            if env.exception_check() {
                session.log("a pending exception interrupted nativeRender");
                return -1;
            }
            if !result {
                // Once the player has rendered, false is its terminal loop
                // result (Application.Quit), not another startup wait. Retrying
                // it 20,000 times delayed exit and misreported a clean stop as
                // failure. Keep transient-false recovery before the first frame.
                session.log("nativeRender requested player stop");
                *engine_stopped = true;
                break;
            }
            frames += 1;
            if log_progress && frames%60==0{session.log(&format!("render progress: native_frames={frames}"));}
            // The managed frame has unwound. The host skips this event
            // without a listener; snapshots never allocate per frame.
            session.api.fire_event(ModuleKind::UnityFrame);
            // Memory-pressure maintenance remains throttled to once/sec.
            let now = Instant::now();
            if now.duration_since(maintenance_at) >= Duration::from_secs(1) {
                maintenance_at = now;
                session.api.fire_event(ModuleKind::UnityMaintenance);
            }
            if log_progress {
                let work_us = frame_started.elapsed().as_micros();
                max_work_us = max_work_us.max(work_us);
                slow_frames += u64::from(work_us >= 50_000);
                if let Some(stats) = frame_work.as_mut() {
                    stats.record(frame_started.duration_since(first_frame_started).as_millis(), work_us);
                }
            }
            if frame_limit > 0 && frames >= frame_limit {
                break;
            }
            // Count input, nativeRender and driver/vsync waiting toward the
            // cap. Legacy render_sleep_us remains an explicit extra-delay
            // override; its default is zero for every engine profile.
            let delay = remaining_frame_time(session.target_fps, frame_started.elapsed())
                .max(Duration::from_micros(u64::from(session.render_sleep_us)));
            if !delay.is_zero() { std::thread::sleep(delay); }
        }
        session.log(&format!(
            "render loop complete: {frames} frames rendered (player stopped)"
        ));
        if log_progress {
            session.log(&format!("render timing: native_calls={frames} elapsed_ms={} max_frame_work_us={max_work_us} frames_over_50ms={slow_frames}",
                first_frame_started.elapsed().as_millis()));
            if let Some(stats) = frame_work.as_ref() {
                for report in stats.reports() { session.log(&report); }
            }
        }
    }
    0
}

/// Echo the spy's recorded event buffer verbatim into the host log (the
/// call-sequence + arguments the acceptance asserts on). No-op for a real
/// guest without the fixture exports.
fn echo_spy_events(session: &GuestSession, main_base: usize) {
    let Some(count_address) = session.api.guest_symbol(main_base, spy::EVENT_COUNT) else {
        return;
    };
    let Some(at_address) = session.api.guest_symbol(main_base, spy::EVENT_AT) else {
        session.log("spy evidence readback failed: rudroid_spy_event_at missing");
        return;
    };
    // SAFETY: the spy fixture exports `int rudroid_spy_event_count(void)`.
    let count_fn: extern "C" fn() -> c_int = unsafe { std::mem::transmute(count_address) };
    // SAFETY: the spy fixture exports `int rudroid_spy_event_at(int, RudroidSpyEvent *)`;
    // it writes only through the event pointer.
    let at_fn: extern "C" fn(c_int, &mut spy::Event) -> c_int = unsafe { std::mem::transmute(at_address) };
    let count = count_fn();
    for index in 0..count {
        let mut event = spy::Event {
            kind: 0,
            a: 0,
            b: 0,
            text: [0; 48],
        };
        if at_fn(index, &mut event) == 1 {
            session.log(&format!(
                "spy event[{}] kind={}({}) a={:#x} b={:#x} text={:?}",
                index,
                spy::kind_name(event.kind),
                event.kind,
                event.a,
                event.b,
                event.text_lossy()
            ));
        }
    }
}
