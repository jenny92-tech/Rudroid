//! INPUT: Android window/display/resource/locale requests and authoritative Host Config
//! OUTPUT: cached JNI services, default single-display routing and explicit host-presentation compatibility control
//! POS: shared Android façade metadata; Linux rendering/audio devices remain owned by their backends
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{JObject, JniEnv, Native};
use std::ffi::{c_void, CStr};
use std::sync::{atomic::{AtomicBool, Ordering}, OnceLock};
static WINDOW: OnceLock<usize> = OnceLock::new();
static VIEW: OnceLock<usize> = OnceLock::new();
static RESOURCES: OnceLock<usize> = OnceLock::new();
static METRICS: OnceLock<usize> = OnceLock::new();
static DISPLAY: OnceLock<usize> = OnceLock::new();
static DISPLAY_MANAGER: OnceLock<usize> = OnceLock::new();
static WINDOW_MANAGER: OnceLock<usize> = OnceLock::new();
static INPUT_MANAGER: OnceLock<usize> = OnceLock::new();
static AUDIO_MANAGER: OnceLock<usize> = OnceLock::new();
static POWER_MANAGER: OnceLock<usize> = OnceLock::new();
static MEDIA_ROUTER: OnceLock<usize> = OnceLock::new();
static MEDIA_ROUTE: OnceLock<usize> = OnceLock::new();
// A virtual presentation route changes native Unity audio initialization.
// Enable it only through a version profile with measured compatibility evidence.
static HOST_PRESENTATION_ROUTE: AtomicBool = AtomicBool::new(false);
static LOCALE: OnceLock<usize> = OnceLock::new();
static LANGUAGE: OnceLock<usize> = OnceLock::new();
static COUNTRY: OnceLock<usize> = OnceLock::new();
static TAG: OnceLock<usize> = OnceLock::new();
static LOCALE_TEXT: OnceLock<usize> = OnceLock::new();
static LOCALE_CONFIG: OnceLock<String> = OnceLock::new();
static RESOLVER: OnceLock<usize> = OnceLock::new();
static MODE: OnceLock<usize> = OnceLock::new();
fn singleton(env: JniEnv, cache: &OnceLock<usize>, class: &CStr) -> *mut c_void {
    super::java::singleton(env, cache, || super::java::allocate(env, class))
}
extern "C" fn window(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &WINDOW, c"android/view/Window")
}
extern "C" fn view(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &VIEW, c"android/view/View")
}
extern "C" fn resources(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &RESOURCES, c"android/content/res/Resources")
}
extern "C" fn resolver(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &RESOLVER, c"android/content/ContentResolver")
}
extern "C" fn display(env: JniEnv, _object: JObject) -> JObject {
    singleton(env, &DISPLAY, c"android/view/Display")
}
extern "C" fn default_locale(env: JniEnv, _class: JObject) -> JObject {
    singleton(env, &LOCALE, c"java/util/Locale")
}
fn locale_config() -> &'static str {
    LOCALE_CONFIG.get_or_init(|| {
        let host = super::HOST.get().unwrap();
        host.config_string(c"locale.tag", 4096)
            .unwrap_or_else(|| "en-US".to_owned())
    })
}
fn locale_string(env: JniEnv, cache: &OnceLock<usize>, value: &str) -> *mut c_void {
    super::java::singleton(env, cache, || super::preferences::string(env, value))
}
extern "C" fn language(env: JniEnv, _locale: JObject) -> JObject {
    locale_string(
        env,
        &LANGUAGE,
        locale_config().split(['-', '_']).next().unwrap_or(""),
    )
}
extern "C" fn country(env: JniEnv, _locale: JObject) -> JObject {
    locale_string(
        env,
        &COUNTRY,
        locale_config()
            .split_once(['-', '_'])
            .map(|(_, country)| country)
            .unwrap_or(""),
    )
}
extern "C" fn locale_tag(env: JniEnv, _locale: JObject) -> JObject {
    locale_string(env, &TAG, locale_config())
}
extern "C" fn locale_text(env: JniEnv, _locale: JObject) -> JObject {
    super::java::singleton(env, &LOCALE_TEXT, || {
        super::preferences::string(env, &locale_config().replace('-', "_"))
    })
}
extern "C" fn fixed_policy(_env: JniEnv, _object: JObject, _value: i32) {}
extern "C" fn fixed_flags(_env: JniEnv, _object: JObject, _a: i32, _b: i32) {}
extern "C" fn permission(env: JniEnv, _context: JObject, name: JObject) -> i32 {
    match super::preferences::text(env, name).as_deref() {
        Some("android.permission.MODIFY_AUDIO_SETTINGS") => 0,
        _ => -1,
    }
}
extern "C" fn system_service(
    env: JniEnv,
    _context: JObject,
    name: JObject,
) -> JObject {
    match super::preferences::text(env, name).as_deref() {
        Some("display") => singleton(
            env,
            &DISPLAY_MANAGER,
            c"android/hardware/display/DisplayManager",
        ),
        Some("window") => singleton(env, &WINDOW_MANAGER, c"android/view/WindowManager"),
        Some("input") => singleton(env, &INPUT_MANAGER, c"android/hardware/input/InputManager"),
        Some("audio") => singleton(env, &AUDIO_MANAGER, c"android/media/AudioManager"),
        Some("power") => singleton(env, &POWER_MANAGER, c"android/os/PowerManager"),
        Some("media_router") => singleton(env, &MEDIA_ROUTER, c"android/media/MediaRouter"),
        _ => std::ptr::null_mut(),
    }
}
static DPI: OnceLock<i32> = OnceLock::new();
fn dpi() -> i32 {
    *DPI.get_or_init(|| {
        super::HOST.get().unwrap().config_i64(c"device.displayDpi", 100)
            .clamp(1, i32::MAX as i64) as i32
    })
}
fn dimension(width: bool) -> i32 {
    let host = super::HOST.get().unwrap();
    // SDL publishes the real drawable before guest preparation. Do not pin
    // a requested size if a service was queried during an earlier phase.
    rudroid_common::display_dimensions::RenderDimensions::from_host(host)
        .map(|size| if width { size.width } else { size.height }).unwrap_or(0)
}
extern "C" fn width(_env: JniEnv, _object: JObject) -> i32 {
    dimension(true)
}
extern "C" fn height(_env: JniEnv, _object: JObject) -> i32 {
    dimension(false)
}
fn fill_metrics(env: JniEnv, object: *mut c_void) {
    let class = env.get_object_class(object);
    for (name, value) in [
        (c"widthPixels", dimension(true)),
        (c"heightPixels", dimension(false)),
        (c"densityDpi", dpi()),
    ] {
        env.set_int_field(object, env.get_field_id(class, name, c"I"), value);
    }
    for (name, value) in [
        (c"density", dpi() as f32 / 160.0),
        (c"scaledDensity", dpi() as f32 / 160.0),
        (c"xdpi", dpi() as f32),
        (c"ydpi", dpi() as f32),
    ] {
        env.set_float_field(object, env.get_field_id(class, name, c"F"), value);
    }
    super::preferences::delete(env, class);
}
extern "C" fn metrics(env: JniEnv, _resources: JObject) -> JObject {
    super::java::singleton(env, &METRICS, || {
        let object = super::java::allocate(env, c"android/util/DisplayMetrics");
        fill_metrics(env, object);
        object
    })
}
extern "C" fn get_metrics(env: JniEnv, _display: JObject, metrics: JObject) {
    fill_metrics(env, metrics)
}
extern "C" fn supported_modes(env: JniEnv, _display: JObject) -> JObject {
    let mode = singleton(env, &MODE, c"android/view/Display$Mode");
    let class = env.find_class(c"android/view/Display$Mode");
    let result = env.new_object_array(1, class, mode);
    super::preferences::delete(env, class);
    super::preferences::delete(env, mode);
    result
}
extern "C" fn get_display(env: JniEnv, manager: JObject, id: i32) -> JObject {
    // Android's default lookup and the reference host presentation identity
    // describe the same cached display; neither creates another host window.
    if matches!(id, 0 | 1) {
        display(env, manager)
    } else {
        std::ptr::null_mut()
    }
}
extern "C" fn identifier(
    _env: JniEnv,
    _resource: JObject,
    _a: JObject,
    _b: JObject,
    _c: JObject,
) -> i32 {
    0
}
extern "C" fn timing_offset(_env: JniEnv, _object: JObject) -> i64 {
    0
}
extern "C" fn rotation(_env: JniEnv, _object: JObject) -> i32 {
    super::HOST.get().unwrap().config_i64(c"device.displayRotation", 0).clamp(0, 3) as i32
}
extern "C" fn requested_orientation(_env: JniEnv, _activity: JObject) -> i32 {
    // The host owns a fixed display, as in javastubs/android_misc.cpp.
    // Android ActivityInfo orientation constants are distinct from rotation.
    super::HOST.get().unwrap().config_i64(c"device.displayOrientation", 0)
        .clamp(i32::MIN as i64, i32::MAX as i64) as i32
}
extern "C" fn display_listener(
    _env: JniEnv,
    _manager: JObject,
    _listener: JObject,
    _handler: JObject,
) {
}
extern "C" fn audio_devices(
    env: JniEnv,
    _manager: JObject,
    _flags: i32,
) -> JObject {
    let class = env.find_class(c"android/media/AudioDeviceInfo");
    let result = env.new_object_array(0, class, std::ptr::null_mut());
    super::preferences::delete(env, class);
    result
}
extern "C" fn selected_route(
    env: JniEnv,
    _router: JObject,
    flags: i32,
) -> JObject {
    if flags & 2 != 0 && HOST_PRESENTATION_ROUTE.load(Ordering::Acquire) {
        singleton(env, &MEDIA_ROUTE, c"android/media/MediaRouter$RouteInfo")
    } else {
        std::ptr::null_mut()
    }
}
extern "C" fn configure_host_presentation(_env: JniEnv, _class: JObject, enabled: u8) {
    HOST_PRESENTATION_ROUTE.store(enabled != 0, Ordering::Release);
}
extern "C" fn presentation_display(env: JniEnv, object: JObject) -> JObject {
    if HOST_PRESENTATION_ROUTE.load(Ordering::Acquire) {
        display(env, object)
    } else {
        std::ptr::null_mut()
    }
}
extern "C" fn display_id(_env: JniEnv, _object: JObject) -> i32 {
    // Preserve the primary host display identity independently of whether a
    // version profile enables the virtual presentation compatibility route.
    1
}
extern "C" fn system_ui_visibility(_env: JniEnv, _object: JObject) -> i32 {
    0
}
extern "C" fn refresh_rate(_env: JniEnv, _object: JObject) -> f32 {
    static RATE: OnceLock<f32> = OnceLock::new();
    *RATE.get_or_init(|| {
        let rate = super::HOST.get().unwrap().config_f64(c"device.displayRefreshRate", 60.0);
        if rate.is_finite() && rate > 1.0 {
            rate.min(1000.0) as f32
        } else {
            60.0
        }
    })
}
extern "C" fn unsupported(_env: JniEnv, _object: JObject) -> u8 {
    0
}
extern "C" fn feature(_env: JniEnv, _object: JObject, _name: JObject) -> u8 {
    0
}
extern "C" fn metrics_init(env: JniEnv, object: JObject) {
    fill_metrics(env, object);
}
extern "C" fn listener(_env: JniEnv, _object: JObject, _listener: JObject) {}
extern "C" fn encode_uri(
    env: JniEnv,
    _class: JObject,
    input: JObject,
) -> JObject {
    let Some(input) = super::preferences::text(env, input) else {
        return std::ptr::null_mut();
    };
    let mut output = String::with_capacity(input.len());
    use std::fmt::Write;
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || b"_-!.~'()*".contains(&b) {
            output.push(b as char);
        } else {
            let _ = write!(output, "%{b:02X}");
        }
    }
    super::preferences::string(env, &output)
}
extern "C" fn audio_property(
    env: JniEnv,
    _object: JObject,
    name: JObject,
) -> JObject {
    let Some(name)=env.string_utf(name) else { return std::ptr::null_mut(); };
    match name.as_c_str().to_bytes() {
        b"android.media.property.OUTPUT_SAMPLE_RATE" => env.new_string_utf(c"48000"),
        b"android.media.property.OUTPUT_FRAMES_PER_BUFFER" => env.new_string_utf(c"256"),
        _ => std::ptr::null_mut(),
    }
}
extern "C" fn configuration(env: JniEnv, _resources: JObject) -> JObject {
    static CONFIG: OnceLock<usize> = OnceLock::new();
    super::java::singleton(env, &CONFIG, || {
        let object = super::java::allocate(env, c"android/content/res/Configuration");
        let class = env.find_class(c"android/content/res/Configuration");
        for (name, value) in [
            (
                c"orientation",
                if dimension(true) > dimension(false) {
                    2
                } else {
                    1
                },
            ),
            (c"keyboard", 1),
            (c"keyboardHidden", 2),
            (c"hardKeyboardHidden", 2),
            (c"navigation", 1),
            (c"navigationHidden", 2),
            (c"touchscreen", 1),
            (c"densityDpi", dpi()),
        ] {
            env.set_int_field(object, env.get_field_id(class, name, c"I"), value);
        }
        env.set_float_field(object, env.get_field_id(class, c"fontScale", c"F"), 1.0);
        let locale = default_locale(env, object);
        env.set_object_field(
            object,
            env.get_field_id(class, c"locale", c"Ljava/util/Locale;"),
            locale,
        );
        super::preferences::delete(env, locale);
        super::preferences::delete(env, class);
        object
    })
}
extern "C" fn locales(env: JniEnv, _configuration: JObject) -> JObject {
    static LIST: OnceLock<usize> = OnceLock::new();
    super::java::singleton(env, &LIST, || {
        let object = super::java::allocate(env, c"android/os/LocaleList");
        let class = env.find_class(c"android/os/LocaleList");
        let locale = default_locale(env, object);
        env.set_object_field(
            object,
            env.get_field_id(class, c"locale", c"Ljava/util/Locale;"),
            locale,
        );
        super::preferences::delete(env, class);
        super::preferences::delete(env, locale);
        object
    })
}
extern "C" fn locale_at(env: JniEnv, list: JObject, index: i32) -> JObject {
    if index != 0 {
        let class = env.find_class(c"java/lang/IndexOutOfBoundsException");
        env.throw_new(class, c"LocaleList index");
        super::preferences::delete(env, class);
        return std::ptr::null_mut();
    }
    let class = env.find_class(c"android/os/LocaleList");
    let result = env.get_object_field(
        list,
        env.get_field_id(class, c"locale", c"Ljava/util/Locale;"),
    );
    super::preferences::delete(env, class);
    result
}
extern "C" fn locale_count(_: JniEnv, _: JObject) -> i32 {
    1
}
extern "C" fn unavailable_setting(
    _: JniEnv,
    _: JObject,
    _: JObject,
    _: JObject,
) -> JObject {
    std::ptr::null_mut()
}
extern "C" fn empty_trace(
    env: JniEnv,
    _class: JObject,
    _throwable: JObject,
) -> JObject {
    super::preferences::string(
        env,
        "Rudroid: Java exception (native diagnostics contain details)",
    )
}

pub(super) fn register(env: JniEnv) -> i32 {
    if declare_field(
        env,
        c"android/os/LocaleList",
        c"locale",
        c"Ljava/util/Locale;",
        false,
    ) != 0
    {
        return -1;
    }
    for (child, parent) in [
        (
            c"android/view/ContextThemeWrapper",
            c"android/content/ContextWrapper",
        ),
        (
            c"android/content/ContextWrapper",
            c"android/content/Context",
        ),
    ] {
        if declare_superclass(env, child, parent) != 0 {
            return -1;
        }
    }
    for (class, fields, static_field) in [
        (
            c"android/content/Context",
            &[
                (c"DISPLAY_SERVICE", c"Ljava/lang/String;"),
                (c"WINDOW_SERVICE", c"Ljava/lang/String;"),
                (c"AUDIO_SERVICE", c"Ljava/lang/String;"),
                (c"POWER_SERVICE", c"Ljava/lang/String;"),
                (c"MEDIA_ROUTER_SERVICE", c"Ljava/lang/String;"),
                (c"LOCATION_SERVICE", c"Ljava/lang/String;"),
                (c"INPUT_SERVICE", c"Ljava/lang/String;"),
            ] as &[(&CStr, &CStr)],
            true,
        ),
        (
            c"android/content/pm/ActivityInfo",
            &[
                (c"SCREEN_ORIENTATION_UNSPECIFIED", c"I"),
                (c"SCREEN_ORIENTATION_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_REVERSE_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_LANDSCAPE", c"I"),
                (c"SCREEN_ORIENTATION_REVERSE_LANDSCAPE", c"I"),
                (c"SCREEN_ORIENTATION_FULL_USER", c"I"),
                (c"SCREEN_ORIENTATION_FULL_SENSOR", c"I"),
                (c"SCREEN_ORIENTATION_SENSOR", c"I"),
                (c"SCREEN_ORIENTATION_USER_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_USER_LANDSCAPE", c"I"),
                (c"SCREEN_ORIENTATION_SENSOR_PORTRAIT", c"I"),
                (c"SCREEN_ORIENTATION_SENSOR_LANDSCAPE", c"I"),
            ],
            true,
        ),
        (
            c"android/view/View",
            &[
                (c"SYSTEM_UI_FLAG_IMMERSIVE_STICKY", c"I"),
                (c"SYSTEM_UI_FLAG_LAYOUT_STABLE", c"I"),
                (c"SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN", c"I"),
                (c"SYSTEM_UI_FLAG_FULLSCREEN", c"I"),
                (c"SYSTEM_UI_FLAG_HIDE_NAVIGATION", c"I"),
                (c"SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION", c"I"),
            ],
            true,
        ),
        (
            c"android/media/AudioManager",
            &[
                (c"STREAM_MUSIC", c"I"),
                (c"GET_DEVICES_OUTPUTS", c"I"),
                (c"PROPERTY_OUTPUT_SAMPLE_RATE", c"Ljava/lang/String;"),
                (c"PROPERTY_OUTPUT_FRAMES_PER_BUFFER", c"Ljava/lang/String;"),
            ],
            true,
        ),
        (
            c"android/content/pm/PackageManager",
            &[
                (c"FEATURE_AUDIO_LOW_LATENCY", c"Ljava/lang/String;"),
                (c"PERMISSION_GRANTED", c"I"),
                (c"GET_META_DATA", c"I"),
                (c"GET_INTENT_FILTERS", c"I"),
            ],
            true,
        ),
        (
            c"android/content/Intent",
            &[(c"ACTION_MAIN", c"Ljava/lang/String;")],
            true,
        ),
        (
            c"android/media/MediaRouter",
            &[(c"ROUTE_TYPE_LIVE_VIDEO", c"I")],
            true,
        ),
        (
            c"android/content/res/Configuration",
            &[
                (c"KEYBOARD_QWERTY", c"I"),
                (c"HARDKEYBOARDHIDDEN_YES", c"I"),
            ],
            true,
        ),
        (
            c"android/content/res/Configuration",
            &[
                (c"orientation", c"I"),
                (c"keyboard", c"I"),
                (c"keyboardHidden", c"I"),
                (c"hardKeyboardHidden", c"I"),
                (c"navigation", c"I"),
                (c"navigationHidden", c"I"),
                (c"touchscreen", c"I"),
                (c"colorMode", c"I"),
                (c"densityDpi", c"I"),
                (c"fontScale", c"F"),
                (c"locale", c"Ljava/util/Locale;"),
                (c"mcc", c"I"),
                (c"mnc", c"I"),
                (c"screenLayout", c"I"),
                (c"uiMode", c"I"),
                (c"screenWidthDp", c"I"),
                (c"screenHeightDp", c"I"),
                (c"smallestScreenWidthDp", c"I"),
            ],
            false,
        ),
        (
            c"android/util/DisplayMetrics",
            &[
                (c"widthPixels", c"I"),
                (c"heightPixels", c"I"),
                (c"densityDpi", c"I"),
                (c"density", c"F"),
                (c"scaledDensity", c"F"),
                (c"xdpi", c"F"),
                (c"ydpi", c"F"),
            ],
            false,
        ),
    ] {
        for (name, sig) in fields {
            if declare_field(env, class, name, sig, static_field) != 0 {
                return -1;
            }
        }
    }
    let configuration_class = env.find_class(c"android/content/res/Configuration");
    for name in [c"KEYBOARD_QWERTY", c"HARDKEYBOARDHIDDEN_YES"] {
        env.set_static_int_field(
            configuration_class,
            env.get_static_field_id(configuration_class, name, c"I"),
            2,
        );
    }
    super::preferences::delete(env, configuration_class);
    for (class, name, value) in [
        (c"android/content/Context", c"DISPLAY_SERVICE", "display"),
        (c"android/content/Context", c"WINDOW_SERVICE", "window"),
        (c"android/content/Context", c"AUDIO_SERVICE", "audio"),
        (c"android/content/Context", c"POWER_SERVICE", "power"),
        (c"android/content/Context", c"LOCATION_SERVICE", "location"),
        (c"android/content/Context", c"INPUT_SERVICE", "input"),
        (c"android/content/Intent", c"ACTION_MAIN", "android.intent.action.MAIN"),
        (
            c"android/content/Context",
            c"MEDIA_ROUTER_SERVICE",
            "media_router",
        ),
        (
            c"android/media/AudioManager",
            c"PROPERTY_OUTPUT_SAMPLE_RATE",
            "android.media.property.OUTPUT_SAMPLE_RATE",
        ),
        (
            c"android/media/AudioManager",
            c"PROPERTY_OUTPUT_FRAMES_PER_BUFFER",
            "android.media.property.OUTPUT_FRAMES_PER_BUFFER",
        ),
        (
            c"android/content/pm/PackageManager",
            c"FEATURE_AUDIO_LOW_LATENCY",
            "android.hardware.audio.low_latency",
        ),
    ] {
        let class = env.find_class(class);
        let value = super::preferences::string(env, value);
        env.set_static_object_field(
            class,
            env.get_static_field_id(class, name, c"Ljava/lang/String;"),
            value,
        );
        super::preferences::delete(env, value);
        super::preferences::delete(env, class);
    }
    for (class, name, value) in [
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_UNSPECIFIED",
            -1,
        ),
        (
            c"android/view/View",
            c"SYSTEM_UI_FLAG_IMMERSIVE_STICKY",
            4096,
        ),
        (c"android/media/AudioManager", c"STREAM_MUSIC", 3),
        (c"android/media/AudioManager", c"GET_DEVICES_OUTPUTS", 2),
        (c"android/content/pm/PackageManager", c"GET_META_DATA", 128),
        (c"android/content/pm/PackageManager", c"GET_INTENT_FILTERS", 32),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_FULL_SENSOR", 10),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_SENSOR", 4),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_USER_PORTRAIT", 12),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_USER_LANDSCAPE", 11),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_SENSOR_PORTRAIT", 7),
        (c"android/content/pm/ActivityInfo", c"SCREEN_ORIENTATION_SENSOR_LANDSCAPE", 6),
        (
            c"android/content/pm/PackageManager",
            c"PERMISSION_GRANTED",
            0,
        ),
        (c"android/media/MediaRouter", c"ROUTE_TYPE_LIVE_VIDEO", 2),
        (c"android/view/View", c"SYSTEM_UI_FLAG_LAYOUT_STABLE", 256),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_PORTRAIT",
            1,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_REVERSE_PORTRAIT",
            9,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_LANDSCAPE",
            0,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_REVERSE_LANDSCAPE",
            8,
        ),
        (
            c"android/content/pm/ActivityInfo",
            c"SCREEN_ORIENTATION_FULL_USER",
            13,
        ),
        (
            c"android/view/View",
            c"SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN",
            1024,
        ),
        (c"android/view/View", c"SYSTEM_UI_FLAG_FULLSCREEN", 4),
        (c"android/view/View", c"SYSTEM_UI_FLAG_HIDE_NAVIGATION", 2),
        (
            c"android/view/View",
            c"SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION",
            512,
        ),
    ] {
        let class = env.find_class(class);
        env.set_static_int_field(class, env.get_static_field_id(class, name, c"I"), value);
        super::preferences::delete(env, class);
    }
    for (class, methods) in [
        (
            c"android/content/Context",
            vec![
                Native::new0(
                    c"getResources",
                    c"()Landroid/content/res/Resources;",
                    resources
                ),
                Native::new0(
                    c"getContentResolver",
                    c"()Landroid/content/ContentResolver;",
                    resolver
                ),
                Native::new1(
                    c"getSystemService",
                    c"(Ljava/lang/String;)Ljava/lang/Object;",
                    system_service
                ),
                Native::new1(
                    c"checkCallingOrSelfPermission",
                    c"(Ljava/lang/String;)I",
                    permission
                ),
            ],
        ),
        (
            c"android/app/Activity",
            vec![
                Native::new0(c"getWindow", c"()Landroid/view/Window;", window),
                Native::new1(c"setRequestedOrientation", c"(I)V", fixed_policy),
                Native::new0(c"getRequestedOrientation", c"()I", requested_orientation),
            ],
        ),
        (
            c"android/view/Window",
            vec![
                Native::new0(c"getDecorView", c"()Landroid/view/View;", view),
                Native::new1(c"addFlags", c"(I)V", fixed_policy),
                Native::new2(c"setFlags", c"(II)V", fixed_flags),
            ],
        ),
        (
            c"android/view/View",
            vec![
                Native::new1(c"setSystemUiVisibility", c"(I)V", fixed_policy),
                Native::new0(c"getSystemUiVisibility", c"()I", system_ui_visibility),
                Native::new1(c"removeOnLayoutChangeListener",c"(Landroid/view/View$OnLayoutChangeListener;)V",listener),
                Native::new1(c"setOnSystemUiVisibilityChangeListener", c"(Landroid/view/View$OnSystemUiVisibilityChangeListener;)V", listener),
                Native::new0(c"getWidth", c"()I", width),
                Native::new0(c"getHeight", c"()I", height),
            ],
        ),
        (
            c"android/view/WindowManager",
            vec![Native::new0(
                c"getDefaultDisplay",
                c"()Landroid/view/Display;",
                display
            )],
        ),
        (
            c"android/view/Display",
            vec![
                Native::new0(c"getDisplayId", c"()I", display_id),
                Native::new0(c"getRotation", c"()I", rotation),
                Native::new0(c"getAppVsyncOffsetNanos", c"()J", timing_offset),
                Native::new0(c"getPresentationDeadlineNanos", c"()J", timing_offset),
                Native::new0(c"isWideColorGamut", c"()Z", unsupported),
                Native::new0(c"isHdr", c"()Z", unsupported),
                Native::new0(c"getRefreshRate", c"()F", refresh_rate),
                Native::new0(c"getWidth", c"()I", width),
                Native::new0(c"getHeight", c"()I", height),
                Native::new1(
                    c"getMetrics",
                    c"(Landroid/util/DisplayMetrics;)V",
                    get_metrics
                ),
                Native::new1(
                    c"getRealMetrics",
                    c"(Landroid/util/DisplayMetrics;)V",
                    get_metrics
                ),
                Native::new0(
                    c"getSupportedModes",
                    c"()[Landroid/view/Display$Mode;",
                    supported_modes
                ),
            ],
        ),
        (
            c"android/hardware/display/DisplayManager",
            vec![Native::new1(c"unregisterDisplayListener",c"(Landroid/hardware/display/DisplayManager$DisplayListener;)V",listener),Native::new2(c"registerDisplayListener", c"(Landroid/hardware/display/DisplayManager$DisplayListener;Landroid/os/Handler;)V", display_listener), Native::new1(
                c"getDisplay",
                c"(I)Landroid/view/Display;",
                get_display
            )],
        ),
        (
            c"android/view/Display$Mode",
            vec![
                Native::new0(c"getRefreshRate", c"()F", refresh_rate),
                Native::new0(c"getPhysicalWidth", c"()I", width),
                Native::new0(c"getPhysicalHeight", c"()I", height),
            ],
        ),
        (
            c"android/content/res/Resources",
            vec![
                Native::new0(c"getConfiguration", c"()Landroid/content/res/Configuration;", configuration),
                Native::new0(
                    c"getDisplayMetrics",
                    c"()Landroid/util/DisplayMetrics;",
                    metrics
                ),
                Native::new3(
                    c"getIdentifier",
                    c"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                    identifier
                ),
            ],
        ),
        (c"android/content/res/Configuration",vec![Native::new0(c"getLocales",c"()Landroid/os/LocaleList;",locales)]),
        (c"android/os/LocaleList",vec![Native::new1(c"get",c"(I)Ljava/util/Locale;",locale_at),Native::new0(c"size",c"()I",locale_count),Native::new0(c"isEmpty",c"()Z",unsupported)]),
        (c"android/provider/Settings$Secure",vec![Native::new2(c"getString",c"(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",unavailable_setting)]),
        (
            c"java/util/Locale",
            vec![
                Native::new0(c"getDefault", c"()Ljava/util/Locale;", default_locale),
                Native::new0(c"getLanguage", c"()Ljava/lang/String;", language),
                Native::new0(c"getCountry", c"()Ljava/lang/String;", country),
                Native::new0(c"toLanguageTag", c"()Ljava/lang/String;", locale_tag),
                Native::new0(c"toString", c"()Ljava/lang/String;", locale_text),
            ],
        ),
        (c"android/util/DisplayMetrics", vec![Native::new0(c"<init>", c"()V", metrics_init)]),
        (c"android/os/PowerManager", vec![Native::new0(c"isSustainedPerformanceModeSupported", c"()Z", unsupported)]),
        (c"android/content/pm/PackageManager", vec![Native::new1(c"hasSystemFeature", c"(Ljava/lang/String;)Z", feature)]),
        (c"android/media/AudioManager", vec![Native::new1(c"getProperty", c"(Ljava/lang/String;)Ljava/lang/String;", audio_property), Native::new1(c"getDevices", c"(I)[Landroid/media/AudioDeviceInfo;", audio_devices)]),
        (c"android/media/MediaRouter", vec![
            Native::new1(c"getSelectedRoute", c"(I)Landroid/media/MediaRouter$RouteInfo;", selected_route),
            Native::new1(c"_rudroidSetHostPresentationRoute", c"(Z)V", configure_host_presentation),
        ]),
        (c"android/media/MediaRouter$RouteInfo", vec![Native::new0(c"getPresentationDisplay", c"()Landroid/view/Display;", presentation_display)]),
        (c"android/net/Uri", vec![
            Native::new1(c"encode", c"(Ljava/lang/String;)Ljava/lang/String;", encode_uri),
            Native::new1(c"decode", c"(Ljava/lang/String;)Ljava/lang/String;", super::uri::decode),
        ]),
        (
            c"android/util/Log",
            vec![Native::new1(
                c"getStackTraceString",
                c"(Ljava/lang/Throwable;)Ljava/lang/String;",
                empty_trace
            )],
        ),
    ] {
        if env.register_class(class, &methods) != 0 {
            return -1;
        }
    }
    if super::threads::register(env) != 0
        || super::input::register(env) != 0
        || super::lifecycle::register(env) != 0
        || super::trust_store::register(env) != 0
    {
        return -1;
    }
    super::choreographer::register(env)
}
