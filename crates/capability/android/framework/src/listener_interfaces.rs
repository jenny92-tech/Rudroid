//! INPUT: actual Android listener calls and shared-VM interface proxies
//! OUTPUT: declared listener contracts with exact boxed callback forwarding
//! POS: callback metadata only; no location service, display events or device data are invented
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::invoke_proxy_arguments;
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use std::ffi::CStr;

const LOCATION: &CStr = c"android/location/LocationListener";
const UI: &CStr = c"android/view/View$OnSystemUiVisibilityChangeListener";
const LAYOUT: &CStr = c"android/view/View$OnLayoutChangeListener";
const DISPLAY: &CStr = c"android/hardware/display/DisplayManager$DisplayListener";
const INPUT: &CStr = c"android/hardware/input/InputManager$InputDeviceListener";

fn forward(env: JniEnv, object: JObject, interface: &CStr, name: &CStr,
           signature: &CStr, raw: &[Arg], boxed: &[JObject]) {
    // A concrete Java implementation overrides this declaration. The shared
    // engine proxy supplies its real native target through JavaProxy.invoke;
    // invoking the bare contract must neither recurse nor silently succeed.
    let proxy = env.find_class(c"org/rudroid/runtime/JavaProxy");
    let is_proxy = env.is_instance_of(object, proxy);
    env.delete_local_ref(proxy);
    if is_proxy {
        invoke_proxy_arguments(env, object, interface, name, signature, raw, boxed);
    } else if !env.exception_check() {
        let error = env.find_class(c"java/lang/UnsupportedOperationException");
        env.throw_new(error, c"listener requires a concrete callback implementation");
        env.delete_local_ref(error);
    }
}
fn integer(env: JniEnv, value: i32) -> JObject {
    let class = env.find_class(c"java/lang/Integer");
    let boxed = env.method(class, c"<init>", c"(I)V").map_or(std::ptr::null_mut(),
        |ctor| env.new_object(class, ctor, &[Arg::int(value)]));
    env.delete_local_ref(class);
    boxed
}
extern "C" fn location(env: JniEnv, object: JObject, location: JObject) {
    forward(env, object, LOCATION, c"onLocationChanged", c"(Landroid/location/Location;)V",
        &[Arg::object(location)], &[location]);
}
extern "C" fn status(env: JniEnv, object: JObject, provider: JObject, status: i32, extras: JObject) {
    let number = integer(env, status);
    forward(env, object, LOCATION, c"onStatusChanged", c"(Ljava/lang/String;ILandroid/os/Bundle;)V",
        &[Arg::object(provider), Arg::int(status), Arg::object(extras)], &[provider, number, extras]);
    env.delete_local_ref(number);
}
extern "C" fn enabled(env: JniEnv, object: JObject, provider: JObject) {
    forward(env, object, LOCATION, c"onProviderEnabled", c"(Ljava/lang/String;)V",
        &[Arg::object(provider)], &[provider]);
}
extern "C" fn disabled(env: JniEnv, object: JObject, provider: JObject) {
    forward(env, object, LOCATION, c"onProviderDisabled", c"(Ljava/lang/String;)V",
        &[Arg::object(provider)], &[provider]);
}
macro_rules! int_callback {
    ($body:ident, $interface:ident, $name:literal) => {
        extern "C" fn $body(env: JniEnv, object: JObject, value: i32) {
            let number = integer(env, value);
            forward(env, object, $interface, $name, c"(I)V", &[Arg::int(value)], &[number]);
            env.delete_local_ref(number);
        }
    };
}
int_callback!(visibility, UI, c"onSystemUiVisibilityChange");
int_callback!(display_added, DISPLAY, c"onDisplayAdded");
int_callback!(display_removed, DISPLAY, c"onDisplayRemoved");
int_callback!(display_changed, DISPLAY, c"onDisplayChanged");
int_callback!(input_added, INPUT, c"onInputDeviceAdded");
int_callback!(input_removed, INPUT, c"onInputDeviceRemoved");
int_callback!(input_changed, INPUT, c"onInputDeviceChanged");

extern "C" fn layout(env: JniEnv, object: JObject, view: JObject,
    left: i32, top: i32, right: i32, bottom: i32,
    old_left: i32, old_top: i32, old_right: i32, old_bottom: i32) {
    let bounds = [left, top, right, bottom, old_left, old_top, old_right, old_bottom];
    let numbers = bounds.map(|value| integer(env, value));
    let mut raw = vec![Arg::object(view)];
    raw.extend(bounds.map(Arg::int));
    let mut boxed = vec![view];
    boxed.extend(numbers);
    forward(env, object, LAYOUT, c"onLayoutChange", c"(Landroid/view/View;IIIIIIII)V", &raw, &boxed);
    for number in numbers { env.delete_local_ref(number); }
}

pub(super) fn register(env: JniEnv) -> i32 {
    for (class, methods) in [
        (LOCATION, vec![
            Native::new1(c"onLocationChanged", c"(Landroid/location/Location;)V", location),
            Native::new3(c"onStatusChanged", c"(Ljava/lang/String;ILandroid/os/Bundle;)V", status),
            Native::new1(c"onProviderEnabled", c"(Ljava/lang/String;)V", enabled),
            Native::new1(c"onProviderDisabled", c"(Ljava/lang/String;)V", disabled),
        ]),
        (UI, vec![Native::new1(c"onSystemUiVisibilityChange", c"(I)V", visibility)]),
        (LAYOUT, vec![Native::new9(c"onLayoutChange", c"(Landroid/view/View;IIIIIIII)V", layout)]),
        (DISPLAY, vec![
            Native::new1(c"onDisplayAdded", c"(I)V", display_added),
            Native::new1(c"onDisplayRemoved", c"(I)V", display_removed),
            Native::new1(c"onDisplayChanged", c"(I)V", display_changed),
        ]),
        (INPUT, vec![
            Native::new1(c"onInputDeviceAdded", c"(I)V", input_added),
            Native::new1(c"onInputDeviceRemoved", c"(I)V", input_removed),
            Native::new1(c"onInputDeviceChanged", c"(I)V", input_changed),
        ]),
    ] {
        if env.register_class(class, &methods) != 0 { return -1; }
    }
    0
}
