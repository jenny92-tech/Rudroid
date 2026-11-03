//! INPUT:  trusted production kit, real handheld SDL/Wayland/GLES and creator-thread SDL events
//! OUTPUT:  cached JNI device identity, axis/range/clone parity, release-before-removal and reentrant notifications
//! POS:    opt-in native input acceptance; synthetic hotplug events test the bridge, not physical unplugging
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Method, Native};
use rudroid_unity_loader::host_api;
use sdl2::{
    controller::{Axis, Button},
    event::Event,
};
use std::{
    ffi::{c_void, CStr},
    path::PathBuf,
    sync::Mutex,
};
static NOTICES: Mutex<Vec<(i32, i32)>> = Mutex::new(Vec::new());
fn method(env: JniEnv, obj: JObject, name: &CStr, sig: &CStr) -> Method {
    if env.exception_check() {
        env.exception_describe();
        panic!("pending JNI exception before {name:?}");
    }
    let class = env.get_object_class(obj);
    let m = env.method(class, name, sig);
    if m.is_none() {
        env.exception_describe();
        panic!("native method {name:?} {sig:?} on {obj:?}");
    }
    let m = m.unwrap();
    env.delete_local_ref(class);
    assert!(!env.exception_check());
    m
}
fn allocate(env: JniEnv, name: &CStr) -> JObject {
    let class = env.find_class(name);
    let o = env.alloc_object(class);
    env.delete_local_ref(class);
    assert!(!o.is_null());
    o
}
extern "C" fn added(_e: JniEnv, _o: JObject, id: i32) {
    NOTICES.lock().unwrap().push((0, id));
}
extern "C" fn removed(e: JniEnv, _o: JObject, id: i32) {
    let class = e.find_class(c"android/view/InputDevice");
    let get = e
        .static_method(class, c"getDevice", c"(I)Landroid/view/InputDevice;")
        .unwrap();
    assert!(
        e.call_static_object(class, get, &[Arg::int(id)]).is_null(),
        "removed device absent at callback"
    );
    e.delete_local_ref(class);
    NOTICES.lock().unwrap().push((1, id));
}
extern "C" fn changed(e: JniEnv, listener: JObject, id: i32) {
    NOTICES.lock().unwrap().push((2, id));
    // Re-enter listener registration from a callback: production must release its list lock first.
    let manager = allocate(e, c"android/hardware/input/InputManager");
    let m = method(
        e,
        manager,
        c"unregisterInputDeviceListener",
        c"(Landroid/hardware/input/InputManager$InputDeviceListener;)V",
    );
    e.call_void(manager, m, &[Arg::object(listener)]);
    e.delete_local_ref(manager);
}
fn int(e: JniEnv, o: JObject, name: &CStr) -> i32 {
    e.call_int(o, method(e, o, name, c"()I"), &[])
}
fn axis(e: JniEnv, o: JObject, n: i32) -> f32 {
    e.call_float(o, method(e, o, c"getAxisValue", c"(I)F"), &[Arg::int(n)])
}
fn next_event(e: JniEnv, source: JObject, poll: Method) -> JObject {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        let o = e.call_static_object(source, poll, &[]);
        assert!(!e.exception_check());
        if !o.is_null() {
            return o;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for SDL event"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn await_notice(e: JniEnv, source: JObject, poll: Method, count: usize) {
    for _ in 0..500 {
        let o = e.call_static_object(source, poll, &[]);
        e.delete_local_ref(o);
        if NOTICES.lock().unwrap().len() >= count {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("device notification timed out");
}
// Each ignored probe runs as its own process; libraries and the SDL session
// are deliberately pinned through all VM callbacks, matching the real loader.
fn input_env(hats: bool) -> JniEnv {
    input_env_identity(hats, false)
}
fn input_env_identity(hats: bool, xbox_identity: bool) -> JniEnv {
    let kit =
        PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit required"));
    let config=rudroid_host_config::HostConfig::parse(&format!("[device]\ndisplayWidth=960\ndisplayHeight=720\n[gpu]\nrenderWidth=640\nrenderHeight=480\n[input]\ncontroller=true\ndpad_synthesize_hat={hats}\nandroid_xbox360_identity={xbox_identity}\n")).unwrap();
    host_api::install_config(Some(config)).unwrap();
    let host = host_api::table();
    // SAFETY: trusted same-kit libraries, pinned through all registered callbacks and session owners.
    let libraries: Vec<_> = unsafe {
        [
            "libplatform_log_stderr.so",
            "libreply_catalog.so",
            "libandroid_jni_vm.so",
            "libandroid_base.so",
            "libplatform_sdl_runtime.so",
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
            // SAFETY: documented phase ABI on libraries pinned for this process.
            match unsafe { host_api::run_phase_entry(phase, l) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {}
                Err(e) => panic!("phase failed: {e:?}"),
            }
        }
    }
    // SAFETY: exact documented session ABI, all calls on this creator thread, no guest/render workers.
    unsafe {
        let l = &libraries[4];
        let create = l
            .get::<extern "C" fn(i64, i64, i32, i32) -> *mut c_void>(
                b"rudroid_sdl_session_create",
            )
            .unwrap();
        let handle = create(640, 480, 3, 2);
        assert!(!handle.is_null());
        let configure = l
            .get::<extern "C" fn(*mut c_void, i32, i32, i32) -> i32>(
                b"rudroid_sdl_render_configure",
            )
            .unwrap();
        assert_eq!(configure(handle, 640, 480, 1), 0);
        let activate = l
            .get::<unsafe extern "C" fn(*mut c_void, Option<extern "C" fn()>) -> i32>(
                b"rudroid_sdl_guest_graphics_activate",
            )
            .unwrap();
        assert_eq!(activate(handle, None), 0);
    }
    host.java_vm().unwrap().get_env(0x10006).unwrap()
}

#[test]
#[ignore = "run alone on real handheld with RUDROID_RESOURCE_KIT; optional RUDROID_INPUT_CACHE_GATE gates native allocation observation"]
fn real_jni_cached_device_queries_reuse_the_existing_owners() {
    let e = input_env(false);
    let class = e.find_class(c"android/view/InputDevice");
    let get = e.static_method(class, c"getDevice", c"(I)Landroid/view/InputDevice;").unwrap();
    let ids = e.call_static_object(class, e.static_method(class, c"getDeviceIds", c"()[I").unwrap(), &[]);
    let mut values = vec![0; e.get_array_length(ids) as usize];
    e.get_int_array_region(ids, 0, &mut values);
    e.delete_local_ref(ids);
    let id = *values.iter().find(|id| **id > 0).expect("mapped controller required");
    let pad = e.call_static_object(class, get, &[Arg::int(id)]);
    let keyboard = e.call_static_object(class, get, &[Arg::int(0)]);
    assert!(!pad.is_null() && !keyboard.is_null());
    assert!(e.call_static_object(class, get, &[Arg::int(-1)]).is_null());
    // Optional, bounded external gate keeps startup/teardown out of malloc observations.
    // Paths and owners are allocated before the observer can start.
    let gate = std::env::var_os("RUDROID_INPUT_CACHE_GATE").map(|dir| {
        let dir = PathBuf::from(dir);
        [dir.join("ready"), dir.join("run"), dir.join("done"), dir.join("release")]
    });
    let wait = |path: &std::path::Path| {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !path.exists() {
            assert!(std::time::Instant::now() < until, "input cache observation gate timed out");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    if let Some(paths) = &gate {
        std::fs::write(&paths[0], b"ready").unwrap();
        wait(&paths[1]);
    }
    let start = std::time::Instant::now();
    for (device_id, owner) in [(0, keyboard), (id, pad)] {
        for _ in 0..20_000 {
            let local = e.call_static_object(class, get, &[Arg::int(device_id)]);
            assert!(e.is_same_object(local, owner), "query must retain the existing device owner");
            e.delete_local_ref(local);
        }
    }
    let elapsed = start.elapsed();
    if let Some(paths) = &gate {
        std::fs::write(&paths[2], b"done").unwrap();
        wait(&paths[3]);
    }
    eprintln!("JNI_INPUT_CACHE queries=40000 elapsed_ns={} identities_preserved=true", elapsed.as_nanos());
    for local in [keyboard, pad, class] { e.delete_local_ref(local); }
    assert!(!e.exception_check());
}

#[test]
#[ignore = "run alone on real handheld with RUDROID_RESOURCE_KIT; requires a mapped physical controller"]
fn real_jni_normalized_identity_is_device_owned_and_excludes_keyboard() {
    let e = input_env_identity(false, true);
    let class = e.find_class(c"android/view/InputDevice");
    let get = e.static_method(class, c"getDevice", c"(I)Landroid/view/InputDevice;").unwrap();
    let ids = e.call_static_object(class, e.static_method(class, c"getDeviceIds", c"()[I").unwrap(), &[]);
    let mut values = vec![0; e.get_array_length(ids) as usize];
    e.get_int_array_region(ids, 0, &mut values);
    e.delete_local_ref(ids);
    let id = *values.iter().find(|id| **id > 0).expect("mapped controller required");
    let pad = e.call_static_object(class, get, &[Arg::int(id)]);
    assert_eq!(int(e, pad, c"getVendorId"), 0x045e);
    assert_eq!(int(e, pad, c"getProductId"), 0x028e);
    let name = e.call_object(pad, method(e, pad, c"getName", c"()Ljava/lang/String;"), &[]);
    assert_eq!(e.string_utf(name).unwrap().as_c_str(), c"Xbox 360 Controller");
    let again = e.call_static_object(class, get, &[Arg::int(id)]);
    assert!(e.is_same_object(pad, again), "repeat queries reuse the existing device owner");
    let keyboard = e.call_static_object(class, get, &[Arg::int(0)]);
    assert_eq!(int(e, keyboard, c"getVendorId"), 0);
    assert_eq!(int(e, keyboard, c"getProductId"), 0);
    let keyboard_name = e.call_object(keyboard, method(e, keyboard, c"getName", c"()Ljava/lang/String;"), &[]);
    assert_ne!(e.string_utf(keyboard_name).unwrap().as_c_str(), c"Xbox 360 Controller");
    for local in [keyboard_name, keyboard, again, name, pad, class] { e.delete_local_ref(local); }
    assert!(!e.exception_check());
}

#[test]
#[ignore = "run on real handheld with RUDROID_RESOURCE_KIT and idle game; requires a mapped physical controller"]
fn real_jni_button_dpad_omits_hat_axes_and_keeps_digital_events() {
    let e = input_env(false);
    let class = e.find_class(c"android/view/InputDevice");
    let ids = e.call_static_object(class, e.static_method(class,c"getDeviceIds",c"()[I").unwrap(), &[]);
    let mut values = vec![0; e.get_array_length(ids) as usize];
    e.get_int_array_region(ids,0,&mut values);e.delete_local_ref(ids);
    let id = *values.iter().find(|id| **id>0).expect("mapped controller required");
    let device = e.call_static_object(class,e.static_method(class,c"getDevice",c"(I)Landroid/view/InputDevice;").unwrap(),&[Arg::int(id)]);
    assert_eq!(int(e, device, c"getVendorId"), 0, "default identity remains unchanged");
    assert_eq!(int(e, device, c"getProductId"), 0);
    let ranges = e.call_object(device,method(e,device,c"getMotionRanges",c"()Ljava/util/List;"),&[]);
    assert_eq!(int(e,ranges,c"size"),8);
    for axis in [15,16] {
        assert!(e.call_object(device,method(e,device,c"getMotionRange",c"(I)Landroid/view/InputDevice$MotionRange;"),&[Arg::int(axis)]).is_null());
        assert!(e.call_object(device,method(e,device,c"getMotionRange",c"(II)Landroid/view/InputDevice$MotionRange;"),&[Arg::int(axis),Arg::int(0x01000010)]).is_null());
    }
    let sdl = sdl2::init().unwrap();let pads = sdl.game_controller().unwrap();
    let index = (0..pads.num_joysticks().unwrap()).find(|i|pads.is_game_controller(*i)).unwrap();
    let pad = pads.open(index).unwrap();let events = sdl.event().unwrap();
    let source = e.find_class(c"org/rudroid/runtime/InputSource");
    let poll = e.static_method(source,c"poll",c"()Ljava/lang/Object;").unwrap();
    loop {let event=e.call_static_object(source,poll,&[]);if event.is_null(){break;}e.delete_local_ref(event);}
    let mut down_time = 0;
    let system = e.find_class(c"java/lang/System");
    let nano_time = e.static_method(system,c"nanoTime",c"()J").unwrap();
    for down in [true,false] {
        let before = rudroid_common::clock::monotonic_nanos();
        let java_now = e.call_static_long(system,nano_time,&[]);
        assert!(java_now>=before && java_now<=rudroid_common::clock::monotonic_nanos());
        let event = if down {Event::ControllerButtonDown {timestamp:0,which:pad.instance_id(),button:Button::DPadDown}} else {Event::ControllerButtonUp {timestamp:0,which:pad.instance_id(),button:Button::DPadDown}};
        events.push_event(event).unwrap();
        let input = e.call_static_object(source,poll,&[]);assert!(!input.is_null());
        assert_eq!(int(e,input,c"getKeyCode"),20);assert_eq!(int(e,input,c"getAction"),i32::from(!down));
        let event_time = e.call_long(input,method(e,input,c"getEventTime",c"()J"),&[]);
        assert!(event_time>=before/1_000_000 && event_time<=rudroid_common::clock::monotonic_nanos()/1_000_000,
            "input time {event_time} must share native uptime domain, before={before}");
        let held_since = e.call_long(input,method(e,input,c"getDownTime",c"()J"),&[]);
        if down {down_time=event_time;}assert_eq!(held_since,down_time);
        e.delete_local_ref(input);
        assert!(e.call_static_object(source,poll,&[]).is_null(), "button mode must not emit a hat MotionEvent");
    }
    assert!(!e.exception_check());
    for local in [system,source,ranges,device,class] {e.delete_local_ref(local);}
    eprintln!("real JNI button dpad passed:8 ranges, no HAT declarations or events, digital press/release retained");
}

#[test]
#[ignore = "run on real handheld with RUDROID_RESOURCE_KIT and idle game; requires a mapped physical controller"]
fn real_jni_controller_axes_ranges_hotplug_and_listener_reentry() {
    let e = input_env(true);
    let source = e.find_class(c"org/rudroid/runtime/InputSource");
    let poll = e
        .static_method(source, c"poll", c"()Ljava/lang/Object;")
        .unwrap();
    let device_class = e.find_class(c"android/view/InputDevice");
    let get_device = e
        .static_method(device_class, c"getDevice", c"(I)Landroid/view/InputDevice;")
        .unwrap();
    let ids = e.call_static_object(
        device_class,
        e.static_method(device_class, c"getDeviceIds", c"()[I")
            .unwrap(),
        &[],
    );
    let mut id_values = vec![0; e.get_array_length(ids) as usize];
    e.get_int_array_region(ids, 0, &mut id_values);
    e.delete_local_ref(ids);
    let id = *id_values
        .iter()
        .find(|id| **id > 0)
        .expect("mapped controller required");
    let device = e.call_static_object(device_class, get_device, &[Arg::int(id)]);
    assert!(!device.is_null());
    let range = e.call_object(
        device,
        method(
            e,
            device,
            c"getMotionRange",
            c"(II)Landroid/view/InputDevice$MotionRange;",
        ),
        &[Arg::int(0), Arg::int(0x01000010)],
    );
    assert!(!range.is_null());
    assert_eq!(
        e.call_float(range, method(e, range, c"getMin", c"()F"), &[]),
        -1.0
    );
    assert_eq!(
        e.call_float(range, method(e, range, c"getMax", c"()F"), &[]),
        1.0
    );
    e.delete_local_ref(range);
    let ranges = e.call_object(
        device,
        method(e, device, c"getMotionRanges", c"()Ljava/util/List;"),
        &[],
    );
    assert_eq!(int(e, ranges, c"size"), 10);
    let first = e.call_object(
        ranges,
        method(e, ranges, c"get", c"(I)Ljava/lang/Object;"),
        &[Arg::int(0)],
    );
    assert_eq!(int(e, first, c"getAxis"), 0);
    e.delete_local_ref(first);
    e.delete_local_ref(ranges);
    let sdl = sdl2::init().unwrap();
    let subsystem = sdl.game_controller().unwrap();
    let index = (0..subsystem.num_joysticks().unwrap())
        .find(|i| subsystem.is_game_controller(*i))
        .unwrap();
    let pad = subsystem.open(index).unwrap();
    let which = pad.instance_id();
    let events = sdl.event().unwrap();
    for _ in 0..256 {
        let event = e.call_static_object(source, poll, &[]);
        if event.is_null() {
            break;
        }
        e.delete_local_ref(event);
    }
    events
        .push_event(Event::ControllerAxisMotion {
            timestamp: 0,
            which,
            axis: Axis::LeftX,
            value: i16::MIN,
        })
        .unwrap();
    let motion = next_event(e, source, poll);
    assert!(!motion.is_null());
    assert_eq!(int(e, motion, c"getDeviceId"), id);
    assert_eq!(axis(e, motion, 0), -1.0);
    let mc = e.find_class(c"android/view/MotionEvent");
    let clone = e.call_static_object(
        mc,
        e.static_method(
            mc,
            c"obtain",
            c"(Landroid/view/MotionEvent;)Landroid/view/MotionEvent;",
        )
        .unwrap(),
        &[Arg::object(motion)],
    );
    assert_eq!(axis(e, clone, 0), -1.0);
    e.delete_local_ref(clone);
    e.delete_local_ref(mc);
    e.delete_local_ref(motion);
    events
        .push_event(Event::ControllerAxisMotion {
            timestamp: 0,
            which,
            axis: Axis::TriggerLeft,
            value: i16::MAX,
        })
        .unwrap();
    let trigger = next_event(e, source, poll);
    assert_eq!(axis(e, trigger, 17), 1.0);
    assert_eq!(axis(e, trigger, 23), 1.0);
    e.delete_local_ref(trigger);
    let key = next_event(e, source, poll);
    assert_eq!(
        (int(e, key, c"getKeyCode"), int(e, key, c"getAction")),
        (104, 0)
    );
    e.delete_local_ref(key);
    let listener_class = c"org/rudroid/tests/InputListener";
    assert_eq!(
        e.register_class(
            listener_class,
            &[
                Native::new1(c"onInputDeviceAdded", c"(I)V", added),
                Native::new1(c"onInputDeviceRemoved", c"(I)V", removed),
                Native::new1(c"onInputDeviceChanged", c"(I)V", changed)
            ]
        ),
        0
    );
    let listener = allocate(e, listener_class);
    let manager = allocate(e, c"android/hardware/input/InputManager");
    let register = method(
        e,
        manager,
        c"registerInputDeviceListener",
        c"(Landroid/hardware/input/InputManager$InputDeviceListener;Landroid/os/Handler;)V",
    );
    for _ in 0..2 {
        e.call_void(
            manager,
            register,
            &[Arg::object(listener), Arg::object(std::ptr::null_mut())],
        );
    }
    events
        .push_event(Event::ControllerDeviceRemapped {
            timestamp: 0,
            which,
        })
        .unwrap();
    await_notice(e, source, poll, 1);
    assert_eq!(&*NOTICES.lock().unwrap(), &[(2, id)]);
    e.call_void(
        manager,
        register,
        &[Arg::object(listener), Arg::object(std::ptr::null_mut())],
    );
    events
        .push_event(Event::ControllerButtonDown {
            timestamp: 0,
            which,
            button: Button::A,
        })
        .unwrap();
    let event = next_event(e, source, poll);
    assert_eq!(int(e, event, c"getAction"), 0);
    e.delete_local_ref(event);
    events
        .push_event(Event::ControllerDeviceRemoved {
            timestamp: 0,
            which,
        })
        .unwrap();
    let mut releases = vec![];
    let mut zero = false;
    for _ in 0..3 {
        let event = next_event(e, source, poll);
        let cls = e.find_class(c"android/view/MotionEvent");
        let is_motion = e.is_instance_of(event, cls);
        e.delete_local_ref(cls);
        if is_motion {
            zero = axis(e, event, 0) == 0.0 && axis(e, event, 17) == 0.0;
        } else {
            assert_eq!(int(e, event, c"getAction"), 1);
            releases.push(int(e, event, c"getKeyCode"));
        }
        let still_present = e.call_static_object(device_class, get_device, &[Arg::int(id)]);
        assert!(
            !still_present.is_null(),
            "inventory remains until the last release is delivered"
        );
        e.delete_local_ref(still_present);
        e.delete_local_ref(event);
    }
    assert!(zero && releases.contains(&104) && releases.contains(&97));
    await_notice(e, source, poll, 2);
    assert_eq!(&*NOTICES.lock().unwrap(), &[(2, id), (1, id)]);
    events
        .push_event(Event::ControllerDeviceAdded {
            timestamp: 0,
            which: index,
        })
        .unwrap();
    await_notice(e, source, poll, 3);
    let notices = NOTICES.lock().unwrap();
    assert_eq!(notices.len(), 3);
    assert_eq!(notices[2].0, 0);
    assert!(notices[2].1 > id);
    drop(notices);
    assert!(!e.exception_check());
    for o in [manager, listener, device, device_class, source] {
        e.delete_local_ref(o);
    }
    eprintln!("real JNI input passed: calibrated axes/ranges, clone, shared-key release, removal ordering, monotonic ids and reentrant listener");
}
