//! INPUT: backend-neutral AudioOutput JNI requests and direct PCM16 buffers
//! OUTPUT: bounded real SDL playback with one producer and joined device teardown
//! POS: Linux output plugin; JNI stores integer identities, SDL devices stay on their producer thread
#![forbid(unsafe_code)]
use rudroid_common::{
    host_api::HostApiV0,
    jni_abi::declare_field,
    jni_env::{JObject, JniEnv, Native, JNI_VERSION_1_6},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::c_void,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::Duration,
};
static HOST: OnceLock<HostApiV0> = OnceLock::new();
static ID: OnceLock<usize> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Output {
    bridge: super::real_audio::AudioBridge,
    samples: Vec<f32>,
    bytes: Vec<u8>,
    channels: usize,
    weak: usize,
    env: JniEnv,
    observed: bool,
}
impl Drop for Output {
    fn drop(&mut self) {
        // AudioBridge closes/joins its callback before releasing callback state.
        // The weak handle owns no Java object, and the shared VM is process pinned.
        self.env.delete_weak_global_ref(self.weak as *mut c_void);
    }
}
thread_local! {static OUTPUTS:RefCell<HashMap<u64,Output>>=RefCell::new(HashMap::new());}
fn error(env: JniEnv, message: &str) {
    let class = env.find_class(c"java/lang/IllegalStateException");
    let text = std::ffi::CString::new(message.replace('\0', " ")).unwrap();
    env.throw_new(class, &text);
    env.delete_local_ref(class);
}
fn id(env: JniEnv, object: *mut c_void) -> u64 {
    env.get_long_field(object, *ID.get().unwrap() as *mut c_void) as u64
}
extern "C" fn open(env: JniEnv, _class: JObject, rate: i32, channels: i32, frames: i32) -> JObject {
    if !(1..=8).contains(&channels) || !(1..=16384).contains(&frames) {
        error(env, "invalid PCM output dimensions");
        return std::ptr::null_mut();
    }
    let bridge = match super::real_audio::AudioBridge::open_with_policy(
        rate,
        channels as u8,
        frames as u32,
        super::audio::output_policy(),
    ) {
        Ok(bridge) => bridge,
        Err(e) => {
            error(env, &format!("audio open failed: {e:?}"));
            return std::ptr::null_mut();
        }
    };
    let count = frames as usize * channels as usize;
    let (mut samples, mut bytes) = (Vec::new(), Vec::new());
    if samples.try_reserve_exact(count).is_err() || bytes.try_reserve_exact(2 * count).is_err() {
        error(env, "PCM conversion buffer allocation failed");
        return std::ptr::null_mut();
    }
    samples.resize(count, 0.0);
    bytes.resize(2 * count, 0);
    let serial = match NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
        id.checked_add(1).filter(|id| *id <= i64::MAX as u64)
    }) {
        Ok(id) => id,
        Err(_) => {
            error(env, "audio identity exhausted");
            return std::ptr::null_mut();
        }
    };
    let class = env.find_class(c"org/rudroid/runtime/AudioOutput");
    let object = env.alloc_object(class);
    env.delete_local_ref(class);
    env.set_long_field(object, *ID.get().unwrap() as *mut c_void, serial as i64);
    let weak = env.new_weak_global_ref(object) as usize;
    if weak == 0 || env.exception_check() {
        env.delete_local_ref(object);
        return std::ptr::null_mut();
    }
    OUTPUTS.with(|outputs| {
        let mut outputs = outputs.borrow_mut();
        outputs.retain(|_, output| {
            !env.is_same_object(output.weak as *mut c_void, std::ptr::null_mut())
        });
        outputs.insert(
            serial,
            Output {
                bridge,
                samples,
                bytes,
                channels: channels as usize,
                weak,
                env,
                observed: false,
            },
        );
    });
    object
}
extern "C" fn write(env: JniEnv, object: JObject, buffer: JObject, frames: i32) -> i32 {
    let id = id(env, object);
    OUTPUTS.with(|outputs| {
        let mut outputs=outputs.borrow_mut();let Some(output)=outputs.get_mut(&id)else{error(env,"audio output is closed or belongs to another producer thread");return -1;};
        let Some(count)=usize::try_from(frames).ok().and_then(|frames|frames.checked_mul(output.channels))else{error(env,"invalid PCM frame count");return -1;};
        if count>output.samples.len()||!env.read_direct_buffer(buffer,&mut output.bytes[..2*count]){error(env,"PCM buffer does not contain the requested frames");return -1;}
        for (sample,bytes) in output.samples[..count].iter_mut().zip(output.bytes.chunks_exact(2)){*sample=i16::from_le_bytes([bytes[0],bytes[1]]) as f32/32768.0;}
        match output.bridge.wait_for_space(frames as usize,Duration::from_secs(1)) {
            Ok(true)=>{},
            Ok(false)=>{error(env,"audio device stopped consuming PCM");return -1;},
            Err(e)=>{error(env,&format!("PCM wait failed: {e:?}"));return -1;},
        }
        if let Err(e)=output.bridge.submit(&output.samples[..count]){error(env,&format!("PCM submission failed: {e:?}"));return -1;}
        let stats=output.bridge.stats();let peak=f32::from_bits(stats.peak_abs_bits.load(Ordering::Acquire));
        if !output.observed&&peak>0.0 {
            output.observed=true;
            if let Some(host)=HOST.get(){if host.config_bool(c"audio.log_pcm_evidence",false){host.log_line(c"SDL",&format!("real PCM consumed: rate={} channels={} pulled_frames={} callbacks={} peak={peak}",output.bridge.spec_report().requested_rate,output.channels,stats.pulled_frames.load(Ordering::Acquire),stats.callbacks.load(Ordering::Acquire)));}}
        }
        frames
    })
}
extern "C" fn close(env: JniEnv, object: JObject) {
    let id = id(env, object);
    OUTPUTS.with(|outputs| {
        outputs.borrow_mut().remove(&id);
    });
}
pub(super) fn register(api: &HostApiV0) -> i32 {
    HOST.get_or_init(|| *api);
    let Some(vm) = api.java_vm() else {
        return 0;
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return -1;
    };
    if declare_field(
        env,
        c"org/rudroid/runtime/AudioOutput",
        c"outputId",
        c"J",
        false,
    ) != 0
    {
        return -1;
    }
    let class = env.find_class(c"org/rudroid/runtime/AudioOutput");
    let id = env.get_field_id(class, c"outputId", c"J");
    env.delete_local_ref(class);
    if id.is_null() {
        return -1;
    }
    ID.get_or_init(|| id as usize);
    env.register_class(
        c"org/rudroid/runtime/AudioOutput",
        &[
            Native::new3(c"open", c"(III)Lorg/rudroid/runtime/AudioOutput;", open),
            Native::new2(c"write", c"(Ljava/nio/ByteBuffer;I)I", write),
            Native::new0(c"close", c"()V", close),
        ],
    )
}
