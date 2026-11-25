//! INPUT:  a validated `Spec`, the loader Host API table, shared JNI session and guest module events
//! OUTPUT:  the `prepare`/`bootstrap`/`run` profile callbacks, player callbacks and the spy evidence echo
//! POS:    the one player lifecycle body every Unity engine profile registers; no per-version branches
//!
//! Player/context/Surface objects belong to the shared VM. Optional fixture
//! observation exports do not replace the real registered JNI lifecycle.

use std::ffi::{c_int, c_void, CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rudroid_common::handles::HandleTable;
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native, JNI_VERSION_1_6};
use rudroid_common::registry::{RudroidModuleEvent, HostStr};
use rudroid_common::runtime_policy::{remaining_frame_time, resolve_target_fps, DEFAULT_TARGET_FPS};
use rudroid_common::ModuleKind;
use rudroid_unity_common::{
    unity_startup_policy_from_api, BootView, UnityRenderThread, UnityStartupPolicy,
};

use crate::{clamp_render_sleep, log, spy, CallbackBody, Profile, Spec, COMMON_PLAYER_CALLBACKS};

// Drain Android UI jobs on their creator, independently of engine callbacks.
// Unity 2022's Java loop never calls executeMainThreadJobs; relying on that
// legacy callback strands Handler/UI work queued during an earlier frame.
// This is the Android UI queue, not UnityPlayer.executeGLThreadJobs.
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
    /// Process-pinned boot overlay owner for non-engine render-thread policy.
    /// Retained so the `/proc/self/fd` replacement stays open for resource queries.
    #[allow(dead_code)]
    boot_view: Option<BootView>,
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

    // The profile provides this player's callback/object model. Declare that
    // actual host type; guest lookup must not create arbitrary absent types.
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        log_on("GetEnv on the shared JavaVM failed; cannot prepare");
        return std::ptr::null_mut();
    };
    let class_name = CString::new(spec.player_class).expect("spec class has no NUL");
    let class = rudroid_common::jni_abi::declare_class(env, &class_name);
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
        let parent = rudroid_common::jni_abi::declare_class(env, c"com/unity3d/player/UnityPlayer");
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
        // These are profile-owned host objects, not guest class queries.
        let class = rudroid_common::jni_abi::declare_class(env, name);
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

    let boot_view = match publish_boot_overlay(api, &log_on) {
        Ok(view) => view,
        Err(reason) => {
            log_on(reason);
            return std::ptr::null_mut();
        }
    };
    if let Some(svc) = crate::mono_service::query(api) {
        if let Some(start) = svc.start_bridge {
            if start(Some(api)) != 0 {
                log_on("unity.mono start_bridge refused");
                return std::ptr::null_mut();
            }
        }
    }

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
        boot_view,
    });
    if !api.on_module_loaded(ModuleKind::Main, capture_module_event, session) {
        log_on(
            "module subscription for Main was rejected; \
             guest dispatch cannot be wired",
        );
        return std::ptr::null_mut();
    }
    log_on(&format!(
        "active engine profile={} player={} (shared JNI object/native dispatch)",
        spec.id, spec.player_class
    ));
    session
}

fn publish_boot_overlay(
    api: &HostApiV0,
    log_on: &dyn Fn(&str),
) -> Result<Option<BootView>, &'static str> {
    let policy = unity_startup_policy_from_api(api)?;
    // Engine keeps HostConfig args untouched. Single/multi publish the
    // command-line override only after BootView succeeds so a rejected
    // prepare cannot leave a stale CONFIG_STRING_OVERRIDES entry.
    if policy.render_thread() == UnityRenderThread::Engine {
        return Ok(None);
    }
    verify_native_player(api, &policy)?;
    let encoded = api
        .asset_path(
            rudroid_common::android_assets::BASE,
            c"bin/Data/boot.config",
            None,
            0,
        )
        .map_err(|_| "cannot resolve extracted Unity boot.config")?
        .ok_or("render-thread policy requires extracted Unity boot.config")?;
    let path = std::path::Path::new(OsStr::from_bytes(encoded.as_bytes()));
    let view = BootView::prepare(path, &policy)?;
    if !api.register_file_overlay(view.source_cstr(), view.replacement_cstr()) {
        return Err("Unity boot overlay registration was refused");
    }
    if let Some(cmd) = policy.command_line() {
        let value = std::ffi::CString::new(cmd.replace('\0', "")).map_err(|_| {
            "Unity render-thread command line contains an interior NUL"
        })?;
        if !api.set_config_string(c"package.mainIntentBundle.unity", &value) {
            return Err("failed to publish Unity render-thread command line");
        }
        api.log_line(
            c"UNITY-STARTUP",
            &format!(
                "render_thread={} (engine keeps original boot; single/multi use a private boot view)",
                policy.render_thread().as_str()
            ),
        );
    }
    log_on(&format!(
        "registered Unity boot overlay for render_thread={}",
        policy.render_thread().as_str()
    ));
    Ok(Some(view))
}

fn verify_native_player(api: &HostApiV0, policy: &UnityStartupPolicy) -> Result<(), &'static str> {
    use std::path::PathBuf;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = api
        .config_string(c"paths.game_files", 4096)
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.clone());
    let root = if root.is_absolute() {
        root
    } else {
        cwd.join(root)
    };
    let native = [
        root.join("lib/arm64-v8a/libunity.so"),
        root.join("gamedata/lib/arm64-v8a/libunity.so"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or("render-thread override requires a verified native Unity player")?;
    let file = std::fs::File::open(native)
        .map_err(|_| "cannot open native Unity player for startup preflight")?;
    let identity = rudroid_common::elf_identity::aarch64_identity(&file)
        .map_err(|_| "cannot validate native Unity player identity")?;
    if !policy.supports_native_player(&identity) {
        return Err(
            "render-thread override is not verified for this native Unity build; retain render_thread_mode=engine",
        );
    }
    Ok(())
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

    if session.api.config_bool(c"unity.separate_player_thread", false) {
        return run_separate_player(session, main_base, env);
    }
    let mut initialized=false;
    let mut engine_stopped=false;
    let result=run_player(&session,main_base,env,&mut initialized,&mut engine_stopped,None);
    finish_run(&session,main_base,env,initialized,engine_stopped,result)
}

fn finish_run(session:&GuestSession,main_base:usize,env:JniEnv,initialized:bool,engine_stopped:bool,result:c_int)->c_int {
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
            return rudroid_common::registry::RUDROID_ENGINE_RUN_PROCESS_EXIT;
        }
        session.log("fast exit refused: preference flush failed; retaining full teardown");
    }
    let clean = rudroid_unity_common::finish_player(
        env, session.player as JObject, session.player_class_handle as JObject, initialized);
    echo_spy_events(&session,main_base);
    if !clean{session.log("player cleanup reported an exception");return -1;}
    result
}

fn run_separate_player(session:std::sync::Arc<GuestSession>,main_base:usize,env:JniEnv)->c_int {
    // A returning/joined worker cannot preserve UnityMain TLS for _exit.
    // Keep the validated process-pinned fast-exit path on its existing owner.
    if session.api.config_bool(c"unity.fast_exit",false) {
        session.log("separate_player_thread cannot be combined with fast_exit");
        return -1;
    }
    let mut initialized=false;
    if !initialize_player(&session,env,&mut initialized) {
        return finish_run(&session,main_base,env,initialized,false,-1);
    }
    let Some(mut input)=rudroid_unity_common::PlayerInput::new(env,session.api.config_bool(c"unity.log_input_events",false)) else {
        return finish_run(&session,main_base,env,true,false,-1);
    };
    let Some(vm)=session.api.java_vm() else {return finish_run(&session,main_base,env,true,false,-1);};
    let sync_snapshot=if rudroid_common::logging::diagnostics_enabled()
        && std::env::var_os("RUDROID_DIAGNOSTIC_SYNC").is_some_and(|v|v=="1")
    {
        let class=env.find_class(c"org/rudroid/runtime/SyncDiagnostics");
        let method=env.static_method(class,c"snapshot",c"()V");
        let Some(method)=method.filter(|_|!env.exception_check()) else {
            env.delete_local_ref(class);
            session.log("requested native sync observer is unavailable");
            return finish_run(&session,main_base,env,true,false,-1);
        };
        Some((class,method))
    } else {None};
    let stopping=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let native_stopping=std::sync::Arc::clone(&stopping);
    let native_session=std::sync::Arc::clone(&session);
    let mut creator_error:JObject=std::ptr::null_mut();
    let mut observed_at=Instant::now();
    let result=super::player_thread::run(move |pump| {
        let Some(env)=vm.attach_current_thread() else {return None;};
        native_session.log(&format!("native player owner attached: thread={:?}",std::thread::current().id()));
        let caller=env.find_class(c"org/rudroid/runtime/CallerLooper");
        let begin=env.static_method(caller,c"begin",c"()Z");
        let drain=env.static_method(caller,c"drain",c"()V");
        let finish=env.static_method(caller,c"finish",c"()V");
        let looper=match (begin,drain,finish) {
            (Some(begin),Some(drain),Some(finish)) if !env.exception_check()
                && env.call_static_boolean(caller,begin,&[]) && !env.exception_check() => Some((drain,finish)),
            _=>None,
        };
        let mut initialized=true;
        let mut engine_stopped=false;
        let mut input=|| {
            if let Some((drain,_))=looper {env.call_static_void(caller,drain,&[]);}
            if env.exception_check() {Err(())} else {pump.poll()}
        };
        let result=if looper.is_some() {
            run_player(&native_session,main_base,env,&mut initialized,&mut engine_stopped,Some(&mut input))
        } else {
            native_session.log("native player owner could not prepare its Looper");-1
        };
        native_stopping.store(true,std::sync::atomic::Ordering::Release);
        // The creator may already be inside nativeInjectEvent. Stop future
        // calls AND wait for that dispatch to return before freeing its owner.
        let input_result=pump.quiesce().unwrap_or(Err(()));
        // Input can fail after the final smoke/terminal frame, with no next
        // frame request to observe it. Preserve that failure through cleanup.
        let result=if input_result.is_err() {-1} else {result};
        let result=finish_run(&native_session,main_base,env,true,engine_stopped,result);
        // A pending exception/local belongs to this worker. Promote only the
        // throwable root, then detach; the creator restores/deletes it once.
        let error=env.exception_occurred();
        let mut error_root=env.new_global_ref(error) as usize;
        env.delete_local_ref(error);
        if let Some((_,finish))=looper {
            // The queue outlives nativeDone and service shutdown. Clear only
            // after retaining the original throwable; finish owns no guest call.
            env.exception_clear();env.call_static_void(caller,finish,&[]);
            if env.exception_check() && error_root==0 {
                let error=env.exception_occurred();
                error_root=env.new_global_ref(error) as usize;env.delete_local_ref(error);
            }
        }
        env.delete_local_ref(caller);
        vm.detach_current_thread();
        Some((if error_root==0 {result} else {-1},error_root))
    }, |input_enabled| {
        execute_main_thread_jobs(env,session.player as JObject);
        let mut result=if env.exception_check() {Err(())}
            else if input_enabled && creator_error.is_null() && !stopping.load(std::sync::atomic::Ordering::Acquire) {
                input.pump_with(session.player as JObject,session.player_class_handle as JObject,
                    |event|super::soft_input::consume(env,session.player as JObject,event))
            } else {Ok(false)};
        if let Some((class,method))=&sync_snapshot {
            if !env.exception_check() && observed_at.elapsed()>=Duration::from_secs(1) {
                observed_at=Instant::now();
                env.call_static_void(*class,*method,&[]);
            }
        }
        // Capture the same tick's diagnostic JNI failure too: the owner can
        // finish before another UI tick. Preserve the first actual throwable.
        if env.exception_check() {
            result=Err(());
            let error=env.exception_occurred();env.exception_clear();
            if creator_error.is_null() {creator_error=error;} else {env.delete_local_ref(error);}
        }
        result
    });
    if let Some((class,_))=sync_snapshot {env.delete_local_ref(class);}
    let worker_result=match result {
        Ok(Ok(Some((result,error)))) => {
            if error!=0 {
                if creator_error.is_null() {env.throw(error as JObject);}
                env.delete_global_ref(error as JObject);
            }
            result
        }
        Ok(Ok(None)) | Err(_) => {
            session.log("player worker could not start/attach; cleaning up creator initialization");
            finish_run(&session,main_base,env,true,false,-1)
        }
        Ok(Err(_)) => {
            rudroid_common::error!("player thread or creator input pump panicked");
            -1
        }
    };
    if !creator_error.is_null() {
        env.throw(creator_error);env.delete_local_ref(creator_error);return -1;
    }
    worker_result
}

fn initialize_player(session:&GuestSession,env:JniEnv,initialized:&mut bool)->bool {
    let (player, player_class) = (session.player as JObject, session.player_class_handle as JObject);
    let resolve = |name: &CStr, signature: &CStr| env.method(player_class, name, signature);
    let init_sig = CString::new(session.spec.init_jni_signature).expect("profile signature");
    let Some(init) = resolve(c"initJni", &init_sig).filter(|_| !env.exception_check()) else {
        session.log("required registered initJni method is unavailable");
        return false;
    };
    let init_args = [Arg::object(session.context as JObject),
        Arg::int(session.init_jni_int_arg)];
    session.log(&format!("calling initJni ({})", session.spec.init_jni_signature));
    session.log(&format!("initJni owner: thread={:?}",std::thread::current().id()));
    *initialized=true;
    // Both initJni shapes take a prefix of (Context, int).
    env.call_void(player, init, &init_args[..init.arity()]);
    if env.exception_check() {
        session.log("a pending exception interrupted initJni");
        return false;
    }
    true
}

fn run_player(session:&GuestSession,main_base:usize,env:JniEnv,initialized:&mut bool,engine_stopped:&mut bool,
    mut external_pump:Option<&mut dyn FnMut()->Result<bool,()>>)->c_int {
    if !*initialized && !initialize_player(session,env,initialized) { return -1; }
    let (player, player_class) = (session.player as JObject, session.player_class_handle as JObject);
    let resolve = |name: &CStr, signature: &CStr| env.method(player_class, name, signature);
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
    let log_progress = session.api.config_bool(c"unity.log_frame_progress", false);
    let phase_diagnostics = log_progress && rudroid_common::logging::diagnostics_enabled();
    let mut render_calls = 0u8;
    let mut render = |env: JniEnv, object| {
        let started = if phase_diagnostics && render_calls < 3 {
            render_calls += 1;
            session.log(&format!("render phase: nativeRender begin call={render_calls}"));
            Some(Instant::now())
        } else { None };
        let result = env.call_boolean(object, render_method, &[]);
        if let Some(started) = started {
            session.log(&format!("render phase: nativeRender end call={render_calls} returned={result} elapsed_ms={}",started.elapsed().as_millis()));
        }
        result
    };
    let inline_player = external_pump.is_none();
    let mut input = if inline_player {
        let Some(input) = rudroid_unity_common::PlayerInput::new(env, session.api.config_bool(c"unity.log_input_events", false))
        else {session.log("input source unavailable");return -1;};
        Some(input)
    } else { None };
    let mut pump_calls = 0u8;
    let mut pump=|| {
        let started = if phase_diagnostics && pump_calls < 3 {
            pump_calls += 1;
            session.log(&format!("render phase: input pump begin call={pump_calls}"));
            Some(Instant::now())
        } else { None };
        let result = if let Some(external) = external_pump.as_mut() {
            external()
        } else {
            execute_main_thread_jobs(env, player);
            if env.exception_check() {
                session.log("a pending exception interrupted UI job dispatch");
                Err(())
            } else {
                input.as_mut().expect("creator input").pump_with(player,player_class,|event|super::soft_input::consume(env,player,event))
            }
        };
        if let Some(started)=started {
            session.log(&format!("render phase: input pump end call={pump_calls} result={result:?} elapsed_ms={}",started.elapsed().as_millis()));
        }
        result
    };
    {
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
        // This timing control leaves native workers running after the real
        // lifecycle calls, before ANY nativeRender/input/vsync dispatch. A
        // failure during the hold therefore cannot require the first render
        // call; surviving it does not establish the cause of a later failure.
        // It never skips initialization, manufactures a frame or changes a
        // guest return value, and ordinary production runs cannot enable it.
        diagnostic_pre_render_hold(session);
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
        #[cfg(feature = "diag-probes")]
        let mut phase_trace = inline_player.then(super::phase_timing::Trace::configured).flatten();
        let mut frames: i64 = 1;
        loop {
            let frame_started = Instant::now();
            // The first startup render has already completed. At this point
            // frames is also the one-based index of the new steady iteration.
            #[cfg(feature = "diag-probes")]
            let mut phase_sample = phase_trace.as_mut().and_then(|trace| trace.begin(frames as u64));
            match pump() {Ok(true)=>break,Ok(false)=>{},Err(())=>{session.log("input dispatch failed");return -1;}}
            signal_vsync_fn();
            #[cfg(feature = "diag-probes")]
            if let (Some(trace), Some(sample)) = (&mut phase_trace, &mut phase_sample) { trace.before_render(sample); }
            let result = render(env, player);
            #[cfg(feature = "diag-probes")]
            if let (Some(trace), Some(sample)) = (&mut phase_trace, &mut phase_sample) { trace.after_render(sample); }
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
            // Count input, nativeRender and driver/vsync waiting toward the
            // cap. Legacy render_sleep_us remains an explicit extra-delay
            // override; its default is zero for every engine profile.
            let reached_limit = frame_limit > 0 && frames >= frame_limit;
            let delay = if reached_limit {
                Duration::ZERO
            } else {
                remaining_frame_time(session.target_fps, frame_started.elapsed())
                    .max(Duration::from_micros(u64::from(session.render_sleep_us)))
            };
            #[cfg(feature = "diag-probes")]
            let sleep_start = if !delay.is_zero() {
                let start = phase_sample.as_ref().map(|_| rudroid_common::clock::monotonic_nanos());
                std::thread::sleep(delay);
                start
            } else {
                None
            };
            #[cfg(feature = "diag-probes")]
            if let (Some(trace), Some(sample)) = (&mut phase_trace, phase_sample) {
                trace.finish(sample, delay, sleep_start);
            }
            #[cfg(not(feature = "diag-probes"))]
            if !delay.is_zero() { std::thread::sleep(delay); }
            if reached_limit { break; }
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

fn diagnostic_pre_render_hold(session: &GuestSession) {
    if !rudroid_common::logging::diagnostics_enabled() { return; }
    let Ok(value) = std::env::var("RUDROID_DIAGNOSTIC_PRE_RENDER_HOLD_MS") else { return; };
    let Some(milliseconds) = value.parse::<u64>().ok().filter(|value| *value <= 30_000) else {
        session.log("diagnostic pre-render hold rejected: expected 0..=30000 milliseconds");
        return;
    };
    if milliseconds == 0 { return; }
    session.log(&format!("diagnostic pre-render hold begin: milliseconds={milliseconds} nativeRender_calls=0"));
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(milliseconds));
    session.log(&format!("diagnostic pre-render hold complete: elapsed_ms={} original render path resumes", started.elapsed().as_millis()));
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
