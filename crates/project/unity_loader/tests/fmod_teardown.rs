//! INPUT:  rebuilt VM/framework/FMOD DSOs, bounded fake AudioOutput and native owner callbacks
//! OUTPUT:  native-pause drain contrast, owner-stop join and exception-preserving teardown regressions
//! POS:    tests streaming teardown through the real JNI lifecycle without a game or audio driver

use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use rudroid_unity_loader::{host_api, phases::Phase};
use std::{path::Path, sync::atomic::{AtomicBool, AtomicUsize, Ordering}, time::{Duration, Instant}};

static DEVICE: AtomicUsize = AtomicUsize::new(0);
static MIXES: AtomicUsize = AtomicUsize::new(0);
static ALIVE: AtomicBool = AtomicBool::new(true);
static AFTER_FREE: AtomicUsize = AtomicUsize::new(0);
static DRAINED: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicUsize = AtomicUsize::new(0);
static FIRST_ERROR: AtomicUsize = AtomicUsize::new(0);
static PAUSES: AtomicUsize = AtomicUsize::new(0);
static RELEASE_NEEDS_MIXER: AtomicBool = AtomicBool::new(true);

extern "C" fn info(_: JniEnv, _: JObject, index: i32) -> i32 {
    if !ALIVE.load(Ordering::Acquire) { AFTER_FREE.fetch_add(1, Ordering::Relaxed); }
    match index { 0 => 48000, 1 => 64, 3 => 1, 4 => 2, _ => 0 }
}
extern "C" fn mix(_: JniEnv, _: JObject, _: JObject) -> i32 {
    if !ALIVE.load(Ordering::Acquire) { AFTER_FREE.fetch_add(1, Ordering::Relaxed); }
    MIXES.fetch_add(1, Ordering::Release);
    0
}
extern "C" fn open(env: JniEnv, class: JObject, _: i32, _: i32, _: i32) -> JObject {
    env.alloc_object(class)
}
extern "C" fn write(_: JniEnv, _: JObject, _: JObject, frames: i32) -> i32 {
    std::thread::sleep(Duration::from_millis(1));
    frames
}
extern "C" fn close(_: JniEnv, _: JObject) { CLOSED.fetch_add(1, Ordering::Relaxed); }
extern "C" fn pause(env: JniEnv, _: JObject) -> u8 {
    PAUSES.fetch_add(1, Ordering::Relaxed);
    // The verified Unity nativePause suspends its Android audio output before
    // returning. A no-op pause hides the real streamed-release deadlock.
    let class = env.find_class(c"org/fmod/FMODAudioDevice");
    let stop = env.method(class, c"stop", c"()V").unwrap();
    env.call_void(DEVICE.load(Ordering::Acquire) as JObject, stop, &[]);
    env.delete_local_ref(class);
    1
}
extern "C" fn done(env: JniEnv, _: JObject) -> u8 {
    // Model the real FMOD Sound release queue: nativeDone needs additional
    // mixer callbacks before the owner can stop it and release native state.
    let target = MIXES.load(Ordering::Acquire) + 5;
    let deadline = Instant::now() + Duration::from_millis(300);
    while RELEASE_NEEDS_MIXER.load(Ordering::Acquire)
        && MIXES.load(Ordering::Acquire) < target && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    DRAINED.store(!RELEASE_NEEDS_MIXER.load(Ordering::Acquire)
        || MIXES.load(Ordering::Acquire) >= target, Ordering::Release);
    let class = env.find_class(c"org/fmod/FMODAudioDevice");
    let stop = env.method(class, c"stop", c"()V").unwrap();
    env.call_void(DEVICE.load(Ordering::Acquire) as JObject, stop, &[]);
    env.delete_local_ref(class);
    ALIVE.store(false, Ordering::Release); // owner stop MUST join before this.
    1
}
extern "C" fn failure(env: JniEnv, _: JObject) {
    let class = env.find_class(c"java/lang/IllegalStateException");
    env.throw_new(class, c"synthetic shutdown hook failure");
    if FIRST_ERROR.load(Ordering::Acquire) == 0 {
        let error = env.exception_occurred();
        FIRST_ERROR.store(env.new_global_ref(error) as usize, Ordering::Release);
        env.delete_local_ref(error);
    }
    env.delete_local_ref(class);
}

fn run_case(kit: &Path, case: &str) {
    let expects_error = matches!(case, "early-error" | "late-error" | "both-errors");
    let compatible_pause = case == "pause-compatible";
    RELEASE_NEEDS_MIXER.store(!compatible_pause, Ordering::Release);
    host_api::install_config(Some(rudroid_host_config::HostConfig::parse("[paths]\nandroid_files='conf'\n").unwrap())).unwrap();
    let host = host_api::table();
    // SAFETY: compiler-built test DSOs remain pinned through every VM callback.
    let libraries: Vec<_> = unsafe {
        ["libreply_catalog.so", "libandroid_jni_vm.so", "libandroid_base.so", "libsdk_fmod.so"]
            .into_iter().map(|name| std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap())).collect()
    };
    for phase in [Phase::EarlyInit, Phase::RegisterJni, Phase::Init] {
        for library in &libraries {
            // SAFETY: the pinned library exports the phase's same-kit ABI.
            match unsafe { host_api::run_phase_entry(phase, library) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {},
                Err(error) => panic!("phase failed: {error:?}"),
            }
        }
    }
    let env = host.java_vm().unwrap().get_env(0x10006).unwrap();
    assert_eq!(env.register_class(c"org/rudroid/runtime/AudioOutput", &[
        Native::new3(c"open", c"(III)Lorg/rudroid/runtime/AudioOutput;", open),
        Native::new2(c"write", c"(Ljava/nio/ByteBuffer;I)I", write),
        Native::new0(c"close", c"()V", close),
    ]), 0);
    assert_eq!(env.register_class(c"org/fmod/FMODAudioDevice", &[
        Native::new1(c"fmodGetInfo", c"(I)I", info),
        Native::new1(c"fmodProcess", c"(Ljava/nio/ByteBuffer;)I", mix),
    ]), 0);
    let class = env.find_class(c"org/fmod/FMODAudioDevice");
    let device = env.alloc_object(class);
    let root = env.new_global_ref(device);
    DEVICE.store(root as usize, Ordering::Release);
    let start = env.method(class, c"start", c"()V").unwrap();
    env.call_void(device, start, &[]);
    let deadline = Instant::now() + Duration::from_secs(1);
    while MIXES.load(Ordering::Acquire) < 3 && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(1)); }
    assert!(MIXES.load(Ordering::Acquire) >= 3, "real producer must start");
    if expects_error {
        assert_eq!(rudroid_common::jni_abi::declare_superclass(env, c"rudroid/FailingShutdown", c"java/lang/Runnable"), 0);
        assert_eq!(env.register_class(c"rudroid/FailingShutdown", &[Native::new0(c"run", c"()V", failure)]), 0);
        let hook_class = env.find_class(c"rudroid/FailingShutdown");
        let hook = env.alloc_object(hook_class);
        let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
        for (enabled, name) in [
            (matches!(case, "early-error" | "both-errors"), c"registerShutdown"),
            (matches!(case, "late-error" | "both-errors"), c"registerAfterNativeShutdown"),
        ] {
            if enabled {
                let register = env.static_method(lifecycle, name, c"(Ljava/lang/Runnable;)V").unwrap();
                env.call_static_void(lifecycle, register, &[Arg::object(hook)]);
            }
        }
        for local in [hook_class, hook, lifecycle] { env.delete_local_ref(local); }
    }
    assert_eq!(env.register_class(c"rudroid/AudioOwner", &[
        Native::new0(c"nativePause", c"()Z", pause), Native::new0(c"nativeDone", c"()Z", done),
    ]), 0);
    let owner_class = env.find_class(c"rudroid/AudioOwner");
    let owner = env.alloc_object(owner_class);
    if compatible_pause {
        // An ordinary, separate pause still stops its producer. Terminal
        // cleanup itself must not inject a suspend into a live native drain.
        env.call_boolean(owner, env.method(owner_class, c"nativePause", c"()Z").unwrap(), &[]);
    }
    let ok = rudroid_common::jni_abi::finish_player(env, owner, owner_class, true);
    assert_eq!(PAUSES.load(Ordering::Acquire), usize::from(compatible_pause),
        "terminal close must not inject an extra nativePause");
    assert_eq!(ok, !expects_error);
    assert_eq!(env.exception_check(), expects_error, "preserve shutdown exceptions");
    let error = env.exception_occurred();
    env.exception_clear();
    if expects_error {
        assert!(env.is_same_object(error, FIRST_ERROR.load(Ordering::Acquire) as JObject), "the original exception identity must survive every cleanup phase");
        env.delete_global_ref(FIRST_ERROR.load(Ordering::Acquire) as JObject);
    }
    env.delete_local_ref(error);
    assert!(DRAINED.load(Ordering::Acquire), "stream release requires mixer progress during nativeDone");
    assert_eq!(CLOSED.load(Ordering::Acquire), 1);
    let mixed = MIXES.load(Ordering::Acquire);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(MIXES.load(Ordering::Acquire), mixed, "owner stop must synchronously join");
    assert_eq!(AFTER_FREE.load(Ordering::Acquire), 0, "no native state access after owner release");
    let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
    env.call_static_void(lifecycle, env.static_method(lifecycle, c"finish", c"()V").unwrap(), &[]);
    env.delete_local_ref(lifecycle);
    assert_eq!(CLOSED.load(Ordering::Acquire), 1, "repeated finish must not close a producer twice");
    env.call_void(device, start, &[]);
    assert!(!env.call_boolean(device, env.method(class, c"isRunning", c"()Z").unwrap(), &[]), "shutdown must refuse new producers");
    env.delete_global_ref(root);
    for local in [owner, owner_class, device, class] { env.delete_local_ref(local); }
    assert!(!env.exception_check());
}

#[test]
fn streamed_audio_drains_before_native_owner_release() {
    if let Ok(case) = std::env::var("RUDROID_FMOD_TEARDOWN_CHILD") {
        run_case(Path::new(&std::env::var("RUDROID_FMOD_TEARDOWN_KIT").unwrap()), &case);
        return;
    }
    let kit = std::env::temp_dir().join(format!("rudroid-fmod-teardown-{}", std::process::id()));
    std::fs::create_dir_all(&kit).unwrap();
    for (package, stem, file) in [
        ("rudroid-android-reply-catalog", "rudroid_android_reply_catalog", "libreply_catalog.so"),
        ("rudroid-android-jni-vm", "android_jni_vm", "libandroid_jni_vm.so"),
        ("rudroid-android-framework", "android_base", "libandroid_base.so"),
        ("rudroid-android-addon-fmod", "sdk_fmod", "libsdk_fmod.so"),
    ] {
        let source = rudroid_test_support::cargo::ensure_workspace_cdylib(package, stem);
        std::fs::copy(source, kit.join(file)).unwrap();
    }
    for case in ["normal", "early-error", "late-error", "both-errors", "pause-compatible"] {
        let result = std::process::Command::new("timeout").arg("20")
            .arg(std::env::current_exe().unwrap())
            .args(["streamed_audio_drains_before_native_owner_release", "--exact", "--nocapture"])
            .env("RUDROID_FMOD_TEARDOWN_CHILD", case).env("RUDROID_FMOD_TEARDOWN_KIT", &kit)
            .current_dir(&kit).output().unwrap();
        assert!(result.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        println!("native-owner teardown verified: {case}; drained, joined before free, no late native call, first exception preserved");
    }
    std::fs::remove_dir_all(kit).unwrap();
}
