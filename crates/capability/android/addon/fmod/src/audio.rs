//! INPUT:  FMODAudioDevice lifecycle calls and the engine's registered mixer JNI functions
//! OUTPUT:  one producer per device, native-owner stop joins and final lifecycle cleanup
//! POS:    FMOD adapter preserves streaming drains; VM owns PCM and the backend owns playback
#![forbid(unsafe_code)]
use rudroid_common::{
    host_api::HostApiV0,
    jni_abi::{declare_field, declare_superclass},
    jni_env::{Arg, JObject, JniEnv, Method, Native},
};
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::Duration,
};
static HOST: OnceLock<HostApiV0> = OnceLock::new();
static ID: OnceLock<usize> = OnceLock::new();
static ACTIVE_MIXER: AtomicU64 = AtomicU64::new(0);
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Runner {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    owner: std::thread::ThreadId,
}
static RUNNERS: OnceLock<Mutex<HashMap<u64, Arc<Runner>>>> = OnceLock::new();
static START: Mutex<()> = Mutex::new(());
static CLOSING: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Default)]
struct MixerPolicy {
    clocked: bool,
    reuse_format: bool,
    silence: bool,
    diagnostic: bool,
}
static POLICY: OnceLock<MixerPolicy> = OnceLock::new();
type Format = (i32, i32, i32);
static LAST_FORMAT: Mutex<Option<Format>> = Mutex::new(None);
fn valid_format(format: Format) -> bool {
    (1..=384000).contains(&format.0)
        && (1..=16384).contains(&format.1)
        && (1..=8).contains(&format.2)
}
fn choose_format(requested: Format, cached: Option<Format>, reuse: bool) -> Option<(Format, bool)> {
    if valid_format(requested) {
        Some((requested, false))
    } else {
        cached
            .filter(|format| reuse && valid_format(*format))
            .map(|format| (format, true))
    }
}
fn runners() -> &'static Mutex<HashMap<u64, Arc<Runner>>> {
    RUNNERS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn id(env: JniEnv, object: JObject) -> u64 {
    env.get_long_field(object, *ID.get().unwrap() as *mut c_void) as u64
}
fn log(message: &str) {
    if let Some(api) = HOST.get() {
        api.log_line(c"FMOD", message);
    }
}
fn method(
    env: JniEnv,
    class: JObject,
    name: &std::ffi::CStr,
    sig: &std::ffi::CStr,
    static_method: bool,
) -> Option<Method> {
    if static_method {
        env.static_method(class, name, sig)
    } else {
        env.method(class, name, sig)
    }
}
extern "C" fn init(_env: JniEnv, _object: JObject) {}
extern "C" fn record(
    _env: JniEnv,
    _object: JObject,
    _rate: i32,
    _channels: i32,
    _frames: i32,
) -> i32 {
    log("microphone recording unavailable on this backend");
    0
}
extern "C" fn check_init(_env: JniEnv, _class: JObject) -> u8 {
    1
}
extern "C" fn fmod_init(_env: JniEnv, _class: JObject, _context: JObject) {}
extern "C" fn start(env: JniEnv, object: JObject) {
    let _serial = START.lock().unwrap();
    if CLOSING.load(Ordering::Acquire) {
        return;
    }
    let finished: Vec<_> = runners()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, runner)| !runner.active.load(Ordering::Acquire))
        .map(|(&id, r)| (id, Arc::clone(r)))
        .collect();
    for (id, runner) in finished {
        let mut join = runner.thread.lock().unwrap();
        if let Some(thread) = join.take() {
            let _ = thread.join();
        }
        drop(join);
        runners().lock().unwrap().remove(&id);
    }
    let mut serial = id(env, object);
    if runners()
        .lock()
        .unwrap()
        .get(&serial)
        .is_some_and(|r| r.active.load(Ordering::Acquire))
    {
        return;
    }
    if serial == 0 {
        serial = match NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
            id.checked_add(1).filter(|id| *id <= i64::MAX as u64)
        }) {
            Ok(id) => id,
            Err(_) => {
                log("audio device identity exhausted");
                return;
            }
        };
        env.set_long_field(object, *ID.get().unwrap() as *mut c_void, serial as i64);
    }
    if ACTIVE_MIXER
        .compare_exchange(0, serial, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let global = env.new_global_ref(object) as usize;
    if global == 0 || env.exception_check() {
        ACTIVE_MIXER.store(0, Ordering::Release);
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicBool::new(true));
    let worker_stop = Arc::clone(&stop);
    let worker_active = Arc::clone(&active);
    let vm = HOST.get().unwrap().java_vm();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("rudroid-fmod-mixer".into())
        .spawn(move || {
            let Some((vm, env)) = vm.and_then(|vm| Some((vm, vm.attach_current_thread()?))) else {
                worker_active.store(false, Ordering::Release);
                let _ = ready_tx.send(false);
                return;
            };
            let _ = ready_tx.send(true);
            produce(env, global as *mut c_void, &worker_stop);
            env.delete_global_ref(global as *mut c_void);
            worker_active.store(false, Ordering::Release);
            vm.detach_current_thread();
            ACTIVE_MIXER
                .compare_exchange(serial, 0, Ordering::AcqRel, Ordering::Acquire)
                .ok();
        });
    match spawned {
        Ok(thread) => {
            if ready_rx.recv().unwrap_or(false) {
                let owner = thread.thread().id();
                runners().lock().unwrap().insert(
                    serial,
                    Arc::new(Runner {
                        stop,
                        active,
                        thread: Mutex::new(Some(thread)),
                        owner,
                    }),
                );
            } else {
                let _ = thread.join();
                env.delete_global_ref(global as *mut c_void);
                ACTIVE_MIXER
                    .compare_exchange(serial, 0, Ordering::AcqRel, Ordering::Acquire)
                    .ok();
                log("mixer JNI attachment failed");
            }
        }
        Err(e) => {
            env.delete_global_ref(global as *mut c_void);
            ACTIVE_MIXER
                .compare_exchange(serial, 0, Ordering::AcqRel, Ordering::Acquire)
                .ok();
            log(&format!("mixer thread start failed: {e}"));
        }
    }
}
extern "C" fn stop(env: JniEnv, object: JObject) {
    let serial = id(env, object);
    let runner = {
        let _serial = START.lock().unwrap();
        runners().lock().unwrap().get(&serial).cloned()
    };
    if let Some(runner) = runner {
        runner.stop.store(true, Ordering::Release);
        if runner.owner == std::thread::current().id() {
            return;
        }
        // Every external close joins the same producer; keep the entry visible
        // until completion. Native reentrant self-stop never takes this lock.
        {
            let mut join = runner.thread.lock().unwrap();
            if let Some(thread) = join.take() {
                let _ = thread.join();
            }
        }
        let mut table = runners().lock().unwrap();
        if table
            .get(&serial)
            .is_some_and(|current| Arc::ptr_eq(current, &runner))
        {
            table.remove(&serial);
        }
        drop(table);
        log("mixer producer joined by device.stop");
    }
}
extern "C" fn quiesce(_env: JniEnv, _object: JObject) {
    {
        let _serial = START.lock().unwrap();
        CLOSING.store(true, Ordering::Release);
    }
    // Do not stop existing mixers here: native Sound releases can require
    // their progress. The native owner's stop callback joins before free.
    log("audio shutdown quiesced new starts; existing mixers can drain");
}
fn stop_all() {
    let all = {
        let _serial = START.lock().unwrap();
        CLOSING.store(true, Ordering::Release);
        runners()
            .lock()
            .unwrap()
            .iter()
            .map(|(&id, r)| (id, Arc::clone(r)))
            .collect::<HashMap<_, _>>()
    };
    for runner in all.values() {
        runner.stop.store(true, Ordering::Release);
    }
    for (serial, runner) in all {
        if runner.owner == std::thread::current().id() {
            continue;
        }
        let mut join = runner.thread.lock().unwrap();
        if let Some(thread) = join.take() {
            let _ = thread.join();
        }
        drop(join);
        let mut table = runners().lock().unwrap();
        if table.get(&serial).is_some_and(|r| Arc::ptr_eq(r, &runner)) {
            table.remove(&serial);
        }
    }
    log("all mixer producers joined");
}
extern "C" fn shutdown(_env: JniEnv, _object: JObject) {
    stop_all();
}
extern "C" fn running(env: JniEnv, object: JObject) -> u8 {
    u8::from(
        runners()
            .lock()
            .unwrap()
            .get(&id(env, object))
            .is_some_and(|r| r.active.load(Ordering::Acquire)),
    )
}
fn int(env: JniEnv, receiver: JObject, method: Option<Method>, args: &[Arg]) -> i32 {
    if env.push_local_frame(32) != 0 {
        return 0;
    }
    let result = method.map_or(0, |method| env.call_int(receiver, method, args));
    env.pop_local_frame(std::ptr::null_mut());
    result
}
fn produce(env: JniEnv, device: JObject, stop: &AtomicBool) {
    let device_class = env.get_object_class(device);
    let info = method(env, device_class, c"fmodGetInfo", c"(I)I", false);
    let mix = method(
        env,
        device_class,
        c"fmodProcess",
        c"(Ljava/nio/ByteBuffer;)I",
        false,
    );
    env.delete_local_ref(device_class);
    let output_class = env.find_class(c"org/rudroid/runtime/AudioOutput");
    let open = method(
        env,
        output_class,
        c"open",
        c"(III)Lorg/rudroid/runtime/AudioOutput;",
        true,
    );
    let write = method(
        env,
        output_class,
        c"write",
        c"(Ljava/nio/ByteBuffer;I)I",
        false,
    );
    let close = method(env, output_class, c"close", c"()V", false);
    let byte_class = env.find_class(c"java/nio/ByteBuffer");
    let zero = method(env, byte_class, c"_rudroidZeroOwned", c"()V", false);
    if zero.is_none() {
        env.delete_local_ref(byte_class);
        env.delete_local_ref(output_class);
        return;
    }
    let allocate = method(
        env,
        byte_class,
        c"allocateDirect",
        c"(I)Ljava/nio/ByteBuffer;",
        true,
    );
    let mut output: *mut c_void = std::ptr::null_mut();
    let mut buffer: *mut c_void = std::ptr::null_mut();
    let mut format = None;
    let mut warned_format = false;
    let policy = POLICY.get().copied().unwrap_or_default();
    let mut pcm = Vec::new();
    let mut next_tick = None;
    let mut process_count = 0u64;
    while !stop.load(Ordering::Acquire) && !env.exception_check() {
        let requested = (
            int(env, device, info, &[Arg::int(0)]),
            int(env, device, info, &[Arg::int(1)]),
            int(env, device, info, &[Arg::int(4)]),
        );
        let Some(((rate, frames, channels), using_cached)) =
            choose_format(requested, *LAST_FORMAT.lock().unwrap(), policy.reuse_format)
        else {
            if !warned_format {
                log(&format!("waiting for valid mixer format: {requested:?}"));
                warned_format = true;
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        if !using_cached {
            *LAST_FORMAT.lock().unwrap() = Some((rate, frames, channels));
        }
        if format != Some((rate, frames, channels)) {
            format = None;
            if !output.is_null() {
                if let Some(close) = close {
                    env.call_void(output, close, &[]);
                }
                env.delete_local_ref(output);
            }
            env.delete_local_ref(buffer);
            buffer = std::ptr::null_mut();
            output = open.map_or(std::ptr::null_mut(), |open| {
                env.call_static_object(
                    output_class,
                    open,
                    &[Arg::int(rate), Arg::int(channels), Arg::int(frames)],
                )
            });
            if output.is_null() || env.exception_check() {
                if env.exception_check() {
                    env.exception_describe();
                    env.exception_clear();
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            buffer = allocate.map_or(std::ptr::null_mut(), |allocate| {
                env.call_static_object(byte_class, allocate, &[Arg::int(frames * channels * 2)])
            });
            if buffer.is_null() || env.exception_check() {
                break;
            }
            warned_format = false;
            format = Some((rate, frames, channels));
            pcm.resize((frames * channels * 2) as usize, 0);
            next_tick = None;
            log(&format!(
                "mixer output opened: rate={rate} channels={channels} frames={frames}"
            ));
        }
        env.call_void(buffer, zero.unwrap(), &[]);
        if env.exception_check() {
            break;
        }
        let ready = int(env, device, info, &[Arg::int(3)]) == 1;
        if !policy.silence
            && (ready || using_cached)
            && int(env, device, mix, &[Arg::object(buffer)]) != 0
        {
            log("engine mixer failed to fill PCM");
            break;
        }
        if int(env, output, write, &[Arg::object(buffer), Arg::int(frames)]) != frames {
            break;
        }
        process_count = process_count.saturating_add(1);
        if policy.diagnostic
            && (process_count == 1 || process_count % 256 == 0)
            && env.read_direct_buffer(buffer, &mut pcm)
        {
            let peak = pcm
                .chunks_exact(2)
                .map(|b| i32::from(i16::from_le_bytes([b[0], b[1]])).abs())
                .max()
                .unwrap_or(0);
            log(&format!(
                "PCM pump: blocks={process_count} ready={ready} cached={using_cached} peak={peak}"
            ));
        }
        if policy.clocked {
            let now = std::time::Instant::now();
            let duration = Duration::from_nanos((frames as u64 * 1_000_000_000) / rate as u64);
            let deadline = next_tick.unwrap_or(now) + duration;
            if deadline > now {
                std::thread::sleep(deadline - now);
                next_tick = Some(deadline);
            } else {
                next_tick = Some(now);
            }
        }
    }
    if env.exception_check() {
        env.exception_describe();
        env.exception_clear();
        log("mixer stopped after JNI/backend failure");
    }
    if !output.is_null() {
        if let Some(close) = close {
            env.call_void(output, close, &[]);
        }
    }
    for reference in [output, buffer, byte_class, output_class] {
        env.delete_local_ref(reference);
    }
}
pub(super) fn register(api: &HostApiV0, env: JniEnv) -> i32 {
    HOST.get_or_init(|| *api);
    POLICY.get_or_init(|| MixerPolicy {
        clocked: api.config_bool(c"sdk_fmod.clocked_audio_pump", false),
        reuse_format: api.config_bool(c"sdk_fmod.reuse_last_valid_format", false),
        silence: api.config_bool(c"sdk_fmod.silence_audio_pump", false),
        diagnostic: api.config_bool(c"sdk_fmod.audio_diagnostic", false),
    });
    if declare_superclass(env, c"org/fmod/AudioDevice", c"org/fmod/FMODAudioDevice") != 0 {
        return -1;
    }
    if declare_field(env, c"org/fmod/FMODAudioDevice", c"runnerId", c"J", false) != 0 {
        return -1;
    }
    let class = env.find_class(c"org/fmod/FMODAudioDevice");
    let id = env.get_field_id(class, c"runnerId", c"J");
    env.delete_local_ref(class);
    if id.is_null() {
        return -1;
    }
    ID.get_or_init(|| id as usize);
    if env.register_class(
        c"org/fmod/FMODAudioDevice",
        &[
            Native::new0(c"<init>", c"()V", init),
            Native::new0(c"start", c"()V", start),
            Native::new0(c"stop", c"()V", stop),
            Native::new0(c"close", c"()V", stop),
            Native::new0(c"releaseAudioTrack", c"()V", stop),
            Native::new3(c"startAudioRecord", c"(III)I", record),
            Native::new0(c"stopAudioRecord", c"()V", init),
            Native::new0(c"isRunning", c"()Z", running),
        ],
    ) != 0
    {
        return -1;
    }
    if env.register_class(
        c"org/fmod/AudioDevice",
        &[Native::new0(c"<init>", c"()V", init)],
    ) != 0
    {
        return -1;
    }
    if env.register_class(
        c"org/fmod/FMOD",
        &[
            Native::new0(c"checkInit", c"()Z", check_init),
            Native::new1(c"init", c"(Landroid/content/Context;)V", fmod_init),
        ],
    ) != 0
    {
        return -1;
    }
    let lifecycle = env.find_class(c"org/rudroid/runtime/Lifecycle");
    for (name, phase, callback) in [
        (c"org/rudroid/fmod/Quiesce", c"registerShutdown", quiesce as extern "C" fn(JniEnv, JObject)),
        (c"org/rudroid/fmod/Shutdown", c"registerAfterNativeShutdown", shutdown),
    ] {
        if declare_superclass(env, name, c"java/lang/Runnable") != 0
            || env.register_class(name, &[Native::new0(c"run", c"()V", callback)]) != 0
        {
            env.delete_local_ref(lifecycle);
            return -1;
        }
        let class = env.find_class(name);
        let hook = env.alloc_object(class);
        env.delete_local_ref(class);
        let Some(register) = method(env, lifecycle, phase, c"(Ljava/lang/Runnable;)V", true) else {
            env.delete_local_ref(hook);
            env.delete_local_ref(lifecycle);
            return -1;
        };
        env.call_static_void(lifecycle, register, &[Arg::object(hook)]);
        env.delete_local_ref(hook);
    }
    env.delete_local_ref(lifecycle);
    if env.exception_check() {
        -1
    } else {
        0
    }
}
#[cfg(test)]
#[path = "../tests/unit/audio.rs"]
mod tests;
