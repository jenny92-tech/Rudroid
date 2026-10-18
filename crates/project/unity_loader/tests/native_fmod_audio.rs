//! INPUT:  production VM/framework/FM0D/SDL kit and real handheld audio driver
//! OUTPUT:  joined real-device PCM pump under normal/clocked/cached/silence policies
//! POS:    synthetic guest mixer verifies the native compatibility chain without changing game saves
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use rudroid_unity_loader::host_api;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
static MIXES: AtomicUsize = AtomicUsize::new(0);
static QUERIES: AtomicUsize = AtomicUsize::new(0);
extern "C" fn info(_env: JniEnv, _o: JObject, n: i32) -> i32 {
    let cached = std::env::var("RUDROID_AUDIO_MODE").unwrap() == "reuse";
    if n == 0 && cached && QUERIES.fetch_add(1, Ordering::Relaxed) >= 3 {
        return 0;
    }
    match n {
        0 => 48000,
        1 => 1024,
        2 => 4,
        3 => 1,
        4 => 2,
        _ => 0,
    }
}
extern "C" fn mix(env: JniEnv, _o: JObject, buffer: JObject) -> i32 {
    assert_eq!(env.get_direct_buffer_capacity(buffer), 4096);
    let address = env.get_direct_buffer_address(buffer).cast::<u8>();
    assert!(!address.is_null());
    // SAFETY: the producer owns this VM-allocated/rooted PCM block exclusively and invokes
    // the synthetic native mixer synchronously, before AudioOutput reads it.
    let bytes = unsafe { std::slice::from_raw_parts_mut(address, 4096) };
    assert!(
        bytes.iter().all(|b| *b == 0),
        "VM must zero the complete owned block before mixing"
    );
    for (i, frame) in bytes.chunks_exact_mut(4).enumerate() {
        let sample = ((i as f64 * 440.0 * std::f64::consts::TAU / 48000.0).sin() * 1000.0) as i16;
        let b = sample.to_le_bytes();
        frame.copy_from_slice(&[b[0], b[1], b[0], b[1]]);
    }
    MIXES.fetch_add(1, Ordering::Relaxed);
    0
}
#[test]
#[ignore = "run on handheld with production RUDROID_RESOURCE_KIT and RUDROID_AUDIO_MODE=normal/clocked/reuse/silence; requires real audio"]
fn real_fmod_pump_honors_policies_and_joins_before_return() {
    let kit = PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit"));
    let mode = std::env::var("RUDROID_AUDIO_MODE").expect("audio policy mode");
    assert!(matches!(
        mode.as_str(),
        "normal" | "clocked" | "reuse" | "silence"
    ));
    let config=rudroid_host_config::HostConfig::parse(&format!("[audio]\nlog_pcm_evidence=true\n[sdk_fmod]\nclocked_audio_pump={}\nreuse_last_valid_format={}\nsilence_audio_pump={}\naudio_diagnostic=true\n",mode=="clocked",mode=="reuse",mode=="silence")).unwrap();
    host_api::install_config(Some(config)).unwrap();
    let host = host_api::table();
    // SAFETY: trusted complete production kit; every DSO remains process-pinned for VM callbacks.
    let libraries: Vec<_> = unsafe {
        [
            "libplatform_log_stderr.so",
            "libreply_catalog.so",
            "libandroid_jni_vm.so",
            "libandroid_base.so",
            "libplatform_sdl_runtime.so",
            "libsdk_fmod.so",
        ]
        .into_iter()
        .map(|n| std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(n)).unwrap()))
        .collect()
    };
    for phase in [
        rudroid_unity_loader::phases::Phase::EarlyInit,
        rudroid_unity_loader::phases::Phase::RegisterJni,
        rudroid_unity_loader::phases::Phase::Init,
    ] {
        for l in &libraries {
            // SAFETY: exact phase ABI, libraries pinned through every registered callback.
            match unsafe { host_api::run_phase_entry(phase, l) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {}
                Err(e) => panic!("phase failed: {e:?}"),
            }
        }
    }
    let env = host.java_vm().unwrap().get_env(0x10006).unwrap();
    let name = c"org/fmod/FMODAudioDevice";
    assert_eq!(
        env.register_class(
            name,
            &[
                Native::new1(c"fmodGetInfo", c"(I)I", info),
                Native::new1(c"fmodProcess", c"(Ljava/nio/ByteBuffer;)I", mix)
            ]
        ),
        0
    );
    let class = env.find_class(name);
    let device = env.alloc_object(class);
    let start = env.method(class, c"start", c"()V").unwrap();
    let stop = env.method(class, c"stop", c"()V").unwrap();
    let running = env.method(class, c"isRunning", c"()Z").unwrap();
    env.call_void(device, start, &[]);
    assert!(env.call_boolean(device, running, &[]));
    std::thread::sleep(Duration::from_millis(600));
    env.call_void(device, stop, &[]);
    assert!(!env.call_boolean(device, running, &[]));
    let mixed = MIXES.load(Ordering::Relaxed);
    if mode == "silence" {
        assert_eq!(mixed, 0);
    } else {
        assert!(mixed > 3, "native mixer must keep pumping; count={mixed}");
        if mode == "clocked" {
            assert!(
                mixed <= 40,
                "clocked output must stay near its block cadence"
            );
        }
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        MIXES.load(Ordering::Relaxed),
        mixed,
        "stop must join the producer before returning"
    );
    assert!(!env.exception_check());
    env.delete_local_ref(device);
    env.delete_local_ref(class);
    let _ = Arg::int(0);
    eprintln!("real FMOD audio passed: mode={mode} blocks={mixed}, producer joined");
}
