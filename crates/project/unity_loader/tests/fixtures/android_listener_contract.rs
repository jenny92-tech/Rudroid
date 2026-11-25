//! INPUT: production framework/profile DSOs through the real shared JNI table
//! OUTPUT: Android listener lookup, exact mixed native ABI and both Unity proxy routes
//! POS: synthetic callback arguments only; no host hardware or game state is created
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use std::{ffi::CStr, sync::atomic::{AtomicUsize, Ordering}};

const HANDLE: i64 = 0x1234_7654;
const BOUNDS: [i32; 8] = [i32::MIN, -71, 0, 29, 16384, -200000, 0x12345678, i32::MAX];
static CALLS: AtomicUsize = AtomicUsize::new(0);
static DIRECT_CALLS: AtomicUsize = AtomicUsize::new(0);
static REFLECT_CALLS: AtomicUsize = AtomicUsize::new(0);
static VIEW: AtomicUsize = AtomicUsize::new(0);
static EXTRAS: AtomicUsize = AtomicUsize::new(0);
const INTERFACES: [&CStr; 5] = [
    c"android/location/LocationListener",
    c"android/view/View$OnSystemUiVisibilityChangeListener",
    c"android/view/View$OnLayoutChangeListener",
    c"android/hardware/display/DisplayManager$DisplayListener",
    c"android/hardware/input/InputManager$InputDeviceListener",
];
fn integer(env: JniEnv, object: JObject) -> i32 {
    let class = env.find_class(c"java/lang/Integer");
    assert!(env.is_instance_of(object, class));
    let value = env.call_int(object, env.method(class, c"intValue", c"()I").unwrap(), &[]);
    env.delete_local_ref(class);
    value
}
fn delivered(env: JniEnv, name: &CStr, args: JObject) {
    let expected = match name.to_bytes() {
        b"onStatusChanged" => 3,
        b"onLayoutChange" => 9,
        _ => 1,
    };
    assert_eq!(env.get_array_length(args), expected);
    for index in 0..expected {
        let value = env.get_object_array_element(args, index);
        match name.to_bytes() {
            b"onLocationChanged" => assert!(value.is_null()),
            b"onProviderEnabled" | b"onProviderDisabled" => {
                assert_eq!(env.string_utf(value).unwrap().as_c_str(), c"synthetic-provider");
            }
            b"onStatusChanged" => match index {
                0 => assert_eq!(env.string_utf(value).unwrap().as_c_str(), c"synthetic-provider"),
                1 => assert_eq!(integer(env, value), -7),
                _ => assert!(env.is_same_object(value, EXTRAS.load(Ordering::SeqCst) as JObject)),
            },
            b"onLayoutChange" => {
                if index == 0 {
                    assert!(env.is_same_object(value, VIEW.load(Ordering::SeqCst) as JObject));
                } else { assert_eq!(integer(env, value), BOUNDS[(index - 1) as usize]); }
            }
            b"onSystemUiVisibilityChange" | b"onDisplayAdded" | b"onDisplayRemoved" |
            b"onDisplayChanged" | b"onInputDeviceAdded" | b"onInputDeviceRemoved" |
            b"onInputDeviceChanged" => assert_eq!(integer(env, value), 37),
            _ => panic!("unexpected callback {name:?}"),
        }
        env.delete_local_ref(value);
    }
    assert!(!env.exception_check());
    CALLS.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn invoke(env: JniEnv, _: JObject, handle: i64, class: JObject,
    method: JObject, args: JObject) -> JObject {
    assert_eq!(handle, HANDLE);
    let meta = env.find_class(c"java/lang/reflect/Method");
    let declaring = env.call_object(method, env.method(meta, c"getDeclaringClass", c"()Ljava/lang/Class;").unwrap(), &[]);
    assert!(env.is_same_object(declaring, class));
    let name = env.call_object(method, env.method(meta, c"getName", c"()Ljava/lang/String;").unwrap(), &[]);
    let text = env.string_utf(name).unwrap();
    delivered(env, text.as_c_str(), args);
    DIRECT_CALLS.fetch_add(1, Ordering::SeqCst);
    drop(text);
    for local in [declaring, name, meta] { env.delete_local_ref(local); }
    std::ptr::null_mut()
}
extern "C" fn reflect_invoke(env: JniEnv, _: JObject, handle: i64, name: JObject, args: JObject) -> JObject {
    assert_eq!(handle, HANDLE);
    let text = env.string_utf(name).unwrap();
    delivered(env, text.as_c_str(), args);
    REFLECT_CALLS.fetch_add(1, Ordering::SeqCst);
    std::ptr::null_mut()
}
pub fn check(env: JniEnv, player: JObject) {
    assert_eq!(env.register_class(c"bitter/jnibridge/JNIBridge", &[Native::new4(c"invoke",
        c"(JLjava/lang/Class;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;", invoke)]), 0);
    assert_eq!(env.register_class(c"com/unity3d/player/ReflectionHelper", &[Native::new3(c"nativeProxyInvoke",
        c"(JLjava/lang/String;[Ljava/lang/Object;)Ljava/lang/Object;", reflect_invoke)]), 0);
    let classes = INTERFACES.map(|name| env.find_class(name));
    assert!(classes.iter().all(|class| !class.is_null()));
    let class_type = env.find_class(c"java/lang/Class");
    let interfaces = env.new_object_array(5, class_type, std::ptr::null_mut());
    for (index, class) in classes.iter().enumerate() { env.set_object_array_element(interfaces, index as i32, *class); }
    let view_type = env.find_class(c"android/view/View");
    let view = env.alloc_object(view_type);
    let extras_type = env.find_class(c"android/os/Bundle");
    let extras = env.alloc_object(extras_type);
    let provider = env.new_string_utf(c"synthetic-provider");
    // These locals remain rooted until both synchronous callback routes finish.
    VIEW.store(view as usize, Ordering::SeqCst);
    EXTRAS.store(extras as usize, Ordering::SeqCst);
    let mut layout = vec![Arg::object(view)];
    layout.extend(BOUNDS.map(Arg::int));
    let callbacks = [
        (0, c"onLocationChanged", c"(Landroid/location/Location;)V", vec![Arg::object(std::ptr::null_mut())]),
        (0, c"onStatusChanged", c"(Ljava/lang/String;ILandroid/os/Bundle;)V", vec![Arg::object(provider), Arg::int(-7), Arg::object(extras)]),
        (0, c"onProviderEnabled", c"(Ljava/lang/String;)V", vec![Arg::object(provider)]),
        (0, c"onProviderDisabled", c"(Ljava/lang/String;)V", vec![Arg::object(provider)]),
        (1, c"onSystemUiVisibilityChange", c"(I)V", vec![Arg::int(37)]),
        (2, c"onLayoutChange", c"(Landroid/view/View;IIIIIIII)V", layout),
        (3, c"onDisplayAdded", c"(I)V", vec![Arg::int(37)]),
        (3, c"onDisplayRemoved", c"(I)V", vec![Arg::int(37)]),
        (3, c"onDisplayChanged", c"(I)V", vec![Arg::int(37)]),
        (4, c"onInputDeviceAdded", c"(I)V", vec![Arg::int(37)]),
        (4, c"onInputDeviceRemoved", c"(I)V", vec![Arg::int(37)]),
        (4, c"onInputDeviceChanged", c"(I)V", vec![Arg::int(37)]),
    ];
    let bridge = env.find_class(c"bitter/jnibridge/JNIBridge");
    let reflection = env.find_class(c"com/unity3d/player/ReflectionHelper");
    for reflected in [false, true] {
        let object = if reflected {
            env.call_static_object(reflection, env.static_method(reflection, c"newProxyInstance",
                c"(Lcom/unity3d/player/UnityPlayer;J[Ljava/lang/Class;)Ljava/lang/Object;").unwrap(),
                &[Arg::object(player), Arg::long(HANDLE), Arg::object(interfaces)])
        } else {
            env.call_static_object(bridge, env.static_method(bridge, c"newInterfaceProxy",
                c"(J[Ljava/lang/Class;)Ljava/lang/Object;").unwrap(), &[Arg::long(HANDLE), Arg::object(interfaces)])
        };
        assert!(!object.is_null() && !env.exception_check());
        let actual = env.get_object_class(object);
        for (index, name, sig, args) in &callbacks {
            assert!(env.is_instance_of(object, classes[*index]));
            // JNI callers may retain either the interface method ID or look up
            // on the dynamic proxy class. Both must select the same contract.
            for owner in [classes[*index], actual] {
                env.call_void(object, env.method(owner, name, sig).unwrap(), args);
                assert!(!env.exception_check(), "{name:?} left an exception");
            }
        }
        let before = CALLS.load(Ordering::SeqCst);
        env.call_static_void(bridge, env.static_method(bridge, c"disableInterfaceProxy", c"(Ljava/lang/Object;)V").unwrap(), &[Arg::object(object)]);
        env.call_void(object, env.method(actual, c"onLayoutChange", c"(Landroid/view/View;IIIIIIII)V").unwrap(), &callbacks[5].3);
        assert_eq!(CALLS.load(Ordering::SeqCst), before, "disabled callback must not use a stale native handle");
        assert!(!env.exception_check());
        for local in [actual, object] { env.delete_local_ref(local); }
    }
    assert_eq!(CALLS.load(Ordering::SeqCst), callbacks.len() * 4);
    assert_eq!(DIRECT_CALLS.load(Ordering::SeqCst), callbacks.len() * 2);
    assert_eq!(REFLECT_CALLS.load(Ordering::SeqCst), callbacks.len() * 2,
        "the reflection Class[] factory must preserve its own native invocation route");
    let bare = env.alloc_object(classes[4]);
    env.call_void(bare, env.method(classes[4], c"onInputDeviceAdded", c"(I)V").unwrap(), &[Arg::int(37)]);
    assert!(env.exception_check(), "a bare interface cannot report callback success");
    env.exception_clear();
    for local in [bare, bridge, reflection, view, extras, provider, view_type, extras_type, interfaces, class_type] {
        env.delete_local_ref(local);
    }
    for local in classes { env.delete_local_ref(local); }
    VIEW.store(0, Ordering::SeqCst);
    EXTRAS.store(0, Ordering::SeqCst);
    println!("Android listener contracts: 12 callbacks, mixed ABI, reference identity and both native proxy routes passed");
}
