//! Registration ABI and desktop vsync policy without a display.

use super::*;
use std::sync::atomic::AtomicI32;

static ENGINE_TARGET: AtomicI32 = AtomicI32::new(-1);
static ENGINE_VSYNC: AtomicI32 = AtomicI32::new(1);
static MANAGED_SETTER: OnceLock<Setter> = OnceLock::new();
static MANAGED_GETTER: OnceLock<Getter> = OnceLock::new();

extern "C" fn engine_set(value: c_int) {
    ENGINE_TARGET.store(value, Relaxed);
}
extern "C" fn engine_get() -> c_int {
    ENGINE_TARGET.load(Relaxed)
}
extern "C" fn engine_vsync() -> c_int {
    ENGINE_VSYNC.load(Relaxed)
}

extern "C" fn bridge_register(name: HostStr<'_>, address: *const c_void) {
    let Some(name) = name.to_cstr() else { return };
    match name.to_bytes() {
        b"UnityEngine.Application::set_targetFrameRate(System.Int32)" => {
            let _ = MANAGED_SETTER.set(unsafe { std::mem::transmute::<_, Setter>(address) });
        }
        b"UnityEngine.Application::get_targetFrameRate" => {
            let _ = MANAGED_GETTER.set(unsafe { std::mem::transmute::<_, Getter>(address) });
        }
        b"UnityEngine.QualitySettings::get_vSyncCount()"
        | b"UnityEngine.Application::get_targetFrameRateX" => {
            assert_eq!(address as usize, engine_vsync as *const () as usize);
        }
        _ => panic!("unexpected registration: {name:?}"),
    }
}

#[test]
fn desktop_translation_and_observation_preserve_managed_readback() {
    assert!(PACING
        .set(Pacing::new(Mode::Desktop(60), bridge_register))
        .is_ok());
    let register: Register = unsafe { std::mem::transmute(registration_wrapper().unwrap()) };
    register(
        HostStr::new(c"UnityEngine.Application::set_targetFrameRate(System.Int32)"),
        engine_set as *const c_void,
    );
    register(
        HostStr::new(c"UnityEngine.Application::get_targetFrameRate"),
        engine_get as *const c_void,
    );
    register(
        HostStr::new(c"UnityEngine.QualitySettings::get_vSyncCount()"),
        engine_vsync as *const c_void,
    );
    register(
        HostStr::new(c"UnityEngine.Application::get_targetFrameRateX"),
        engine_vsync as *const c_void,
    );
    let pacing = PACING.get().unwrap();
    let get = MANAGED_GETTER.get().unwrap();
    let set = MANAGED_SETTER.get().unwrap();
    assert_eq!(get(), -1);
    pacing.frame();
    assert_eq!((engine_get(), get()), (60, -1));
    set(37);
    pacing.frame();
    assert_eq!((engine_get(), get()), (60, 37));
    ENGINE_VSYNC.store(0, Relaxed);
    pacing.frame();
    assert_eq!((engine_get(), get()), (37, 37));
    set(144);
    pacing.frame();
    assert_eq!((engine_get(), get()), (144, 144));
    ENGINE_VSYNC.store(2, Relaxed);
    pacing.frame();
    assert_eq!((engine_get(), get()), (30, 144));
    ENGINE_VSYNC.store(0, Relaxed);
    set(-1);
    pacing.frame();
    assert_eq!((engine_get(), get()), (60, -1));
    pacing.stopped.store(true, Relaxed);
    set(29);
    pacing.frame();
    assert_eq!(engine_get(), 60);

    let observe = Pacing::new(Mode::Observe, bridge_register);
    let _ = observe.setter.set(engine_set);
    let _ = observe.getter.set(engine_get);
    let _ = observe.vsync.set(engine_vsync);
    observe.set_requested(35);
    ENGINE_VSYNC.store(1, Relaxed);
    observe.frame();
    assert_eq!((engine_get(), observe.get_requested()), (35, 35));
    assert_eq!(snapshot(&observe.engine_readback), Some(35));
}
